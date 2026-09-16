//! Mock OneBot v11 protocol endpoint for smoke-testing gugu without QQ.
//!
//! Self-contained on purpose: gugu is a bin-only crate, so an example cannot
//! import its modules. Run this, point `data/config.toml`'s `ws_url` at it,
//! start gugu — it answers `get_login_info`, `get_friend_list` and
//! `get_group_list` with a mock roster, acks `send_private_msg` /
//! `send_group_msg` (logging target and text), pushes a private message
//! from a roster friend every 5s, and echoes every action to stderr.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

/// Identity the mock reports to `get_login_info`.
const BOT: (u64, &str) = (10001, "MockBot");
/// Roster reported to `get_friend_list`: (user_id, nickname, remark).
/// Pushed private messages rotate senders through this list.
const FRIENDS: [(u64, &str, &str); 3] = [
    (20001, "MockFriend", "老咕咕"),
    (20002, "咕咕鸡", ""),
    (20003, "TestPal", ""),
];
/// Roster reported to `get_group_list`: (group_id, group_name).
const GROUPS: [(u64, &str); 2] = [(30001, "咕咕交流群"), (30002, "Mock群")];
/// Rotated message texts.
const LINES: [&str; 3] = ["咕咕咕（mock）", "吃了吗？（mock）", "测试测试 123（mock）"];

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let fail_first: usize = std::env::var("GUGU_SMOKE_SEND_FAIL_FIRST")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Counts send_* attempts across every connection: a fresh gugu run reconnects,
    // and retry/final-failure scenarios must see a stable failure budget.
    let fails = Arc::new(AtomicUsize::new(0));
    if fail_first > 0 {
        eprintln!("mock_onebot: injecting failure into the first {fail_first} send_* actions");
    }
    let listener = TcpListener::bind("127.0.0.1:3001")
        .await
        .expect("mock_onebot: cannot bind 127.0.0.1:3001");
    eprintln!("mock_onebot: listening on ws://127.0.0.1:3001 (Ctrl-C to stop)");
    loop {
        let (stream, peer) = listener.accept().await.expect("accept failed");
        eprintln!("mock_onebot: {peer} connected");
        tokio::spawn(serve(stream, fails.clone(), fail_first));
    }
}

/// Drive one client connection until it goes away. `fails` counts send_*
/// attempts across connections so the injected failure budget survives
/// reconnects; the first `fail_first` sends answer retcode 1200.
async fn serve(stream: TcpStream, fails: Arc<AtomicUsize>, fail_first: usize) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        eprintln!("mock_onebot: handshake failed");
        return;
    };
    let (mut sink, mut src) = ws.split();
    let mut push = tokio::time::interval(Duration::from_secs(5));
    let mut line = 0usize;
    let mut mid = 0u64;
    loop {
        tokio::select! {
            _ = push.tick() => {
                let (fid, fname, _) = FRIENDS[line % FRIENDS.len()];
                let text = LINES[line % LINES.len()];
                line += 1;
                let event = json!({
                    "time": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64),
                    "self_id": BOT.0,
                    "post_type": "message",
                    "message_type": "private",
                    "sub_type": "friend",
                    "message_id": line,
                    "font": 14,
                    "sender": { "user_id": fid, "nickname": fname, "card": "", "sex": "unknown", "age": 0, "level": "5" },
                    "message": text,
                    "raw_message": text,
                });
                if sink.send(Message::Text(event.to_string().into())).await.is_err() {
                    eprintln!("mock_onebot: client gone, push failed");
                    return;
                }
            }
            item = src.next() => {
                let Some(Ok(msg)) = item else { eprintln!("mock_onebot: client left"); return };
                let Ok(frame) = serde_json::from_str::<Value>(&msg.to_string()) else {
                    eprintln!("mock_onebot: non-JSON frame ignored");
                    continue;
                };
                let action = frame["action"].as_str().unwrap_or("?");
                eprintln!("mock_onebot <- action: {action} params: {}", frame["params"]);
                match action {
                    "get_login_info" => {
                        let data = json!({ "user_id": BOT.0, "nickname": BOT.1, "sex": "male", "age": 0, "level": 1 });
                        if sink.send(Message::Text(ok(&frame, data).into())).await.is_err() {
                            return;
                        }
                    }
                    "get_friend_list" => {
                        let data: Vec<Value> = FRIENDS
                            .iter()
                            .map(|(id, nick, remark)| json!({ "user_id": id, "nickname": nick, "remark": remark }))
                            .collect();
                        if sink.send(Message::Text(ok(&frame, Value::Array(data)).into())).await.is_err() {
                            return;
                        }
                    }
                    "get_group_list" => {
                        let data: Vec<Value> = GROUPS
                            .iter()
                            .map(|(id, name)| json!({ "group_id": id, "group_name": name }))
                            .collect();
                        if sink.send(Message::Text(ok(&frame, Value::Array(data)).into())).await.is_err() {
                            return;
                        }
                    }
                    "send_private_msg" | "send_group_msg" => {
                        let target = if action == "send_private_msg" {
                            frame["params"]["user_id"].clone()
                        } else {
                            frame["params"]["group_id"].clone()
                        };
                        let text = frame["params"]["message"].as_str().unwrap_or("?");
                        let count = fails.fetch_add(1, Ordering::Relaxed) + 1;
                        if count <= fail_first {
                            eprintln!("mock_onebot -> {action} to {target}: INJECTED-FAIL ({count}/{fail_first})");
                            let body = json!({
                                "status": "failed",
                                "retcode": 1200,
                                "wording": "mock injected failure",
                                "echo": frame["echo"],
                            });
                            if sink.send(Message::Text(body.to_string().into())).await.is_err() {
                                return;
                            }
                        } else {
                            mid += 1;
                            eprintln!("mock_onebot -> {action} to {target}: {text} (message_id {mid})");
                            if sink.send(Message::Text(ok(&frame, json!({ "message_id": mid })).into())).await.is_err() {
                                return;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// A retcode-0 reply frame echoing the request's `echo`, carrying `data`.
fn ok(frame: &Value, data: Value) -> String {
    json!({ "status": "ok", "retcode": 0, "echo": frame["echo"], "data": data }).to_string()
}
