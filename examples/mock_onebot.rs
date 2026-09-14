//! Mock OneBot v11 protocol endpoint for smoke-testing gugu without QQ.
//!
//! Self-contained on purpose: gugu is a bin-only crate, so an example cannot
//! import its modules. Run this, point `data/config.toml`'s `ws_url` at it,
//! start gugu — it replies to `get_login_info`, pushes one private message
//! every 5s, and echoes every action frame it receives to stderr.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

/// Identity the mock reports to `get_login_info`.
const BOT: (u64, &str) = (10001, "MockBot");
/// Sender of the pushed private messages.
const FRIEND: (u64, &str) = (20001, "MockFriend");
/// Rotated message texts.
const LINES: [&str; 3] = ["咕咕咕（mock）", "吃了吗？（mock）", "测试测试 123（mock）"];

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:3001")
        .await
        .expect("mock_onebot: cannot bind 127.0.0.1:3001");
    eprintln!("mock_onebot: listening on ws://127.0.0.1:3001 (Ctrl-C to stop)");
    loop {
        let (stream, peer) = listener.accept().await.expect("accept failed");
        eprintln!("mock_onebot: {peer} connected");
        tokio::spawn(serve(stream));
    }
}

/// Drive one client connection until it goes away.
async fn serve(stream: TcpStream) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        eprintln!("mock_onebot: handshake failed");
        return;
    };
    let (mut sink, mut src) = ws.split();
    let mut push = tokio::time::interval(Duration::from_secs(5));
    let mut line = 0usize;
    loop {
        tokio::select! {
            _ = push.tick() => {
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
                    "sender": { "user_id": FRIEND.0, "nickname": FRIEND.1, "card": "", "sex": "unknown", "age": 0, "level": "5" },
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
                if action == "get_login_info" {
                    let reply = json!({
                        "status": "ok", "retcode": 0, "echo": frame["echo"],
                        "data": { "user_id": BOT.0, "nickname": BOT.1, "sex": "male", "age": 0, "level": 1 }
                    });
                    if sink.send(Message::Text(reply.to_string().into())).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}
