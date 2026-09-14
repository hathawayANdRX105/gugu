//! OneBot v11 forward-WS client transport.
//!
//! gugu acts as the WS *client*; an external protocol end (NapCat / Lagrange)
//! runs the actual QQ login and forwards events over `ws://`. The background
//! thread owns the socket: it parses events, sends action frames, and
//! reconnects with backoff. Callers talk to it through [`Handle`] (actions in)
//! and the `on_event` callback (events out); bridging into Slint
//! (`invoke_from_event_loop`) is the caller's job.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

/// Reconnect delays: 1s → 2s → 5s, then capped at 10s.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
];

/// Connection settings (loaded from `data/config.toml`).
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Forward-WS endpoint of the OneBot implementation, e.g. `ws://127.0.0.1:3001`.
    pub ws_url: String,
    /// `access_token`; appended to `ws_url` as a query parameter. Empty = no auth.
    pub access_token: String,
}

impl Config {
    /// Load `{ws_url, access_token}` from a toml file.
    /// Missing file, unreadable file, or missing field → `Err(message)`.
    pub fn load(path: &Path) -> Result<Config, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// One chat target: friend or group.
#[derive(Debug, Clone)]
pub struct Peer {
    /// `user_id` for friends, `group_id` for groups.
    pub id: i64,
    /// Friend: non-empty `remark`, else `nickname`; group: `group_name`.
    pub name: String,
    pub is_group: bool,
}

/// Event delivered to the UI layer on the background thread.
#[derive(Debug)]
pub enum Event {
    /// Reply to the automatic `get_login_info` after (re)connecting.
    Login {
        nickname: String,
        #[allow(dead_code)] // read once T5 sidebar wiring uses it; remove then
        uin: String,
    },
    /// WS established (including after a reconnect).
    Connected,
    /// Private (`message_type == "private"`) message.
    PrivateMessage {
        #[allow(dead_code)] // read once T5 sidebar wiring uses it; remove then
        sender_id: i64,
        sender_name: String,
        text: String,
        time: i64,
    },
    /// Group (`message_type == "group"`) message.
    GroupMessage {
        #[allow(dead_code)] // read once T5 sidebar wiring uses it; remove then
        group_id: i64,
        #[allow(dead_code)] // read once T5 sidebar wiring uses it; remove then
        sender_id: i64,
        sender_name: String,
        text: String,
        time: i64,
    },
    /// Merged `get_friend_list` + `get_group_list` reply, pulled
    /// automatically after each (re)connection. Friends first, groups after.
    Roster { peers: Vec<Peer> },
}

/// Action accepted on [`Handle::send`]; serialized to a OneBot v11 frame.
pub enum Action {
    /// `send_private_msg`
    SendPrivate { user_id: i64, text: String },
    /// `send_group_msg`
    SendGroup { group_id: i64, text: String },
    /// `get_login_info`
    GetLoginInfo,
    /// expands to a `get_friend_list` + `get_group_list` frame pair; the
    /// merged reply becomes [`Event::Roster`].
    GetRoster,
}

/// Sender side of the action queue; cheap to clone out of the UI thread.
pub struct Handle {
    tx: Sender<Action>,
}

impl Handle {
    /// Queue an action. Dropped if the transport thread is gone (it shouldn't
    /// be: the thread survives disconnects via the reconnect loop).
    pub fn send(&self, action: Action) {
        let _ = self.tx.send(action);
    }
}

/// Reply slots tracked per connection: which echo belongs to which request,
/// plus the roster halves parsed so far. [`Event::Roster`] fires once both
/// the friends and groups replies arrive; a half that never answers leaves
/// the slot pending until the next reconnect re-requests it.
/// ponytail: fixed slots for the three request kinds; upgrade to
/// HashMap<echo, oneshot> when action variety outgrows this.
#[derive(Default)]
struct ReplySlots {
    login: Option<String>,
    friends: Option<String>,
    groups: Option<String>,
    roster_friends: Option<Vec<Peer>>,
    roster_groups: Option<Vec<Peer>>,
}

/// Spawn the transport thread (std::thread + current-thread tokio runtime).
///
/// `on_event` runs on that thread — bridge into Slint with
/// `slint::invoke_from_event_loop` before touching UI state. The thread
/// connects, and on every successful (re)connection emits [`Event::Connected`]
/// and sends `get_login_info` then [`Action::GetRoster`]. Disconnects retry
/// with [`BACKOFF`] delays.
/// No client-side heartbeat: tungstenite ping/pong plus error/close-triggered
/// reconnect is enough; add a read timeout only if a half-open NapCat shows up.
pub fn spawn(config: Config, on_event: Box<dyn Fn(Event) + Send>) -> Handle {
    let (tx, rx) = std::sync::mpsc::channel::<Action>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("onebot: failed to build tokio runtime");
        rt.block_on(run(config, rx, on_event));
    });
    Handle { tx }
}

/// Reconnect loop: connect → run one session → repeat after backoff.
async fn run(config: Config, rx: Receiver<Action>, on_event: Box<dyn Fn(Event) + Send>) {
    // Bridge the blocking std channel into the async session loop.
    let (tx, mut arx) = tokio::sync::mpsc::unbounded_channel::<Action>();
    tokio::task::spawn_blocking(move || {
        while let Ok(a) = rx.recv() {
            if tx.send(a).is_err() {
                break;
            }
        }
    });

    let url = authed_url(&config);
    let mut attempt = 0usize;
    let mut echo: i64 = 0;
    loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => {
                attempt = 0;
                on_event(Event::Connected);
                let (sink, stream) = ws.split();
                session(sink, stream, &mut arx, on_event.as_ref(), &mut echo).await;
            }
            Err(e) => eprintln!("onebot: connect {url} failed: {e}"),
        }
        let delay = BACKOFF[attempt.min(BACKOFF.len() - 1)];
        attempt += 1;
        tokio::time::sleep(delay).await;
    }
}

/// The socket pair after `connect_async` + `split`.
type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One connected session: pump frames both ways until socket error/close.
async fn session(
    mut sink: futures_util::stream::SplitSink<Ws, Message>,
    mut stream: futures_util::stream::SplitStream<Ws>,
    arx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>,
    on_event: &(dyn Fn(Event) + Send),
    echo: &mut i64,
) {
    let mut slots = ReplySlots::default();
    // The connect path always asks who we are, then pulls the roster.
    send_action(&mut sink, Action::GetLoginInfo, echo, &mut slots).await;
    send_action(&mut sink, Action::GetRoster, echo, &mut slots).await;
    loop {
        tokio::select! {
            item = stream.next() => {
                let msg = match item {
                    Some(Ok(m)) => m,
                    _ => return, // close, error, or stream end
                };
                if !matches!(msg, Message::Text(_)) {
                    continue; // ping/pong handled by tungstenite; OneBot WS is JSON text
                }
                let txt = msg.to_string();
                let Ok(v) = serde_json::from_str::<Value>(&txt) else {
                    eprintln!("onebot: bad JSON frame: {}", txt.chars().take(120).collect::<String>());
                    continue;
                };
                let ev = if v.get("post_type").is_some() {
                    parse_event(&v)
                } else if v.get("echo").is_some() {
                    parse_reply(&v, &mut slots)
                } else {
                    None // meta_event / heartbeat etc.
                };
                if let Some(ev) = ev {
                    on_event(ev);
                }
            }
            action = arx.recv() => {
                let Some(action) = action else { return }; // sender dropped
                send_action(&mut sink, action, echo, &mut slots).await;
            }
        }
    }
}

/// Write one action frame with a fresh echo (per-connection string of the
/// monotonic counter) and return it, so callers can record reply matching.
async fn send_one(
    sink: &mut futures_util::stream::SplitSink<Ws, Message>,
    name: &str,
    params: Value,
    echo: &mut i64,
) -> String {
    *echo += 1;
    let id = echo.to_string();
    let frame = json!({ "action": name, "params": params, "echo": id.clone() });
    if let Err(e) = sink.send(Message::Text(frame.to_string().into())).await {
        eprintln!("onebot: send {name} failed: {e}");
    }
    id
}

/// Serialize one action and write it: [`Action::GetRoster`] expands to the
/// two list requests; echoes whose replies are consumed land in `slots`.
async fn send_action(
    sink: &mut futures_util::stream::SplitSink<Ws, Message>,
    action: Action,
    echo: &mut i64,
    slots: &mut ReplySlots,
) {
    match action {
        Action::SendPrivate { user_id, text } => {
            let params = json!({ "user_id": user_id, "message": text });
            send_one(sink, "send_private_msg", params, echo).await;
        }
        Action::SendGroup { group_id, text } => {
            let params = json!({ "group_id": group_id, "message": text });
            send_one(sink, "send_group_msg", params, echo).await;
        }
        Action::GetLoginInfo => {
            slots.login = Some(send_one(sink, "get_login_info", json!({}), echo).await);
        }
        Action::GetRoster => {
            slots.friends = Some(send_one(sink, "get_friend_list", json!({}), echo).await);
            slots.groups = Some(send_one(sink, "get_group_list", json!({}), echo).await);
        }
    }
}

/// Append `?access_token=` to the configured URL (empty token = untouched).
fn authed_url(config: &Config) -> String {
    if config.access_token.is_empty() {
        return config.ws_url.clone();
    }
    let sep = if config.ws_url.contains('?') {
        '&'
    } else {
        '?'
    };
    format!(
        "{}{}access_token={}",
        config.ws_url, sep, config.access_token
    )
}

/// Flatten a OneBot `message` field: plain string, or segments joined by
/// their `text` parts (non-text segments — image/face/at — are dropped).
fn message_text(m: &Value) -> String {
    match m {
        Value::String(s) => s.clone(),
        Value::Array(segs) => segs
            .iter()
            .filter(|s| s["type"] == "text")
            .filter_map(|s| s["data"]["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

/// Parse an event frame; `None` for non-message events and unreadable fields.
fn parse_event(v: &Value) -> Option<Event> {
    if v["post_type"] != "message" {
        return None;
    }
    let sender_id = v["sender"]["user_id"].as_i64()?;
    // Group members show their card; fall back to nickname when empty.
    let card = v["sender"]["card"].as_str().unwrap_or("");
    let sender_name = if card.is_empty() {
        v["sender"]["nickname"].as_str().unwrap_or("").to_string()
    } else {
        card.to_string()
    };
    let text = message_text(&v["message"]);
    let time = v["time"].as_i64().unwrap_or(0);
    match v["message_type"].as_str()? {
        "private" => Some(Event::PrivateMessage {
            sender_id,
            sender_name,
            text,
            time,
        }),
        "group" => Some(Event::GroupMessage {
            group_id: v["group_id"].as_i64()?,
            sender_id,
            sender_name,
            text,
            time,
        }),
        _ => None,
    }
}

/// Parse one roster entry; `None` when its id is missing. Friend name
/// prefers a non-empty `remark`; group name is `group_name` (kept even when
/// empty — the peer still exists and is addressable).
fn parse_peer(e: &Value, is_group: bool) -> Option<Peer> {
    let id = if is_group {
        e["group_id"].as_i64()?
    } else {
        e["user_id"].as_i64()?
    };
    let name = if is_group {
        e["group_name"].as_str().unwrap_or("")
    } else {
        let remark = e["remark"].as_str().unwrap_or("");
        if remark.is_empty() {
            e["nickname"].as_str().unwrap_or("")
        } else {
            remark
        }
    };
    Some(Peer {
        id,
        name: name.to_string(),
        is_group,
    })
}

/// Parse an API reply frame. `retcode != 0` → log and drop; echo matching a
/// [`ReplySlots`] slot → [`Event::Login`], or [`Event::Roster`] once both
/// roster halves have arrived; anything else → `None`.
fn parse_reply(v: &Value, slots: &mut ReplySlots) -> Option<Event> {
    let retcode = v["retcode"].as_i64().unwrap_or(-1);
    if retcode != 0 {
        eprintln!(
            "onebot: action failed retcode={retcode} wording={}",
            v["wording"].as_str().unwrap_or("")
        );
        return None;
    }
    let echo = v["echo"].as_str()?;
    if Some(echo) == slots.login.as_deref() {
        slots.login = None;
        return Some(Event::Login {
            nickname: v["data"]["nickname"].as_str()?.to_string(),
            uin: v["data"]["user_id"].as_i64()?.to_string(),
        });
    }
    if Some(echo) == slots.friends.as_deref() {
        slots.friends = None;
        slots.roster_friends = Some(
            v["data"]
                .as_array()?
                .iter()
                .filter_map(|e| parse_peer(e, false))
                .collect(),
        );
    } else if Some(echo) == slots.groups.as_deref() {
        slots.groups = None;
        slots.roster_groups = Some(
            v["data"]
                .as_array()?
                .iter()
                .filter_map(|e| parse_peer(e, true))
                .collect(),
        );
    } else {
        return None;
    }
    // Emit one merged Roster only after both halves are in; friends first.
    if slots.roster_friends.is_some() && slots.roster_groups.is_some() {
        let mut peers = slots.roster_friends.take().unwrap();
        peers.extend(slots.roster_groups.take().unwrap());
        Some(Event::Roster { peers })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tokio::time::timeout;

    fn tmp_toml(content: &str, tag: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("gugu-ob-{}-{}.toml", tag, std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn config_load_ok() {
        let p = tmp_toml(
            "ws_url = \"ws://127.0.0.1:3001\"\naccess_token = \"secret\"\n",
            "ok",
        );
        let c = Config::load(&p).unwrap();
        assert_eq!(c.ws_url, "ws://127.0.0.1:3001");
        assert_eq!(c.access_token, "secret");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn config_load_missing_field() {
        let p = tmp_toml("ws_url = \"ws://x\"\n", "partial");
        assert!(Config::load(&p).is_err());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn config_load_missing_file() {
        assert!(Config::load(Path::new("/nonexistent/gugu-ob-config.toml")).is_err());
    }

    #[test]
    fn event_string_message() {
        let v: Value = serde_json::json!({
            "post_type": "message", "message_type": "private", "time": 1u64,
            "sender": { "user_id": 10, "nickname": "alice", "card": "" },
            "message": "hello"
        });
        match parse_event(&v).unwrap() {
            Event::PrivateMessage {
                sender_id,
                sender_name,
                text,
                ..
            } => {
                assert_eq!(
                    (sender_id, sender_name.as_str(), text.as_str()),
                    (10, "alice", "hello")
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn event_segment_array_message() {
        let v: Value = serde_json::json!({
            "post_type": "message", "message_type": "group", "group_id": 42, "time": 2,
            "sender": { "user_id": 7, "nickname": "bob", "card": "Bobie" },
            "message": [
                { "type": "image", "data": { "file": "x.png" } },
                { "type": "text", "data": { "text": "hi " } },
                { "type": "at", "data": { "qq": 1 } },
                { "type": "text", "data": { "text": "there" } }
            ]
        });
        match parse_event(&v).unwrap() {
            Event::GroupMessage {
                group_id,
                sender_name,
                text,
                ..
            } => {
                assert_eq!(
                    (group_id, sender_name.as_str(), text.as_str()),
                    (42, "Bobie", "hi there")
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn reply_login_info() {
        let mut slots = ReplySlots {
            login: Some("7".to_string()),
            ..Default::default()
        };
        let v: Value = serde_json::json!({
            "status": "ok", "retcode": 0, "echo": "7",
            "data": { "user_id": 123456, "nickname": "gugu" }
        });
        match parse_reply(&v, &mut slots).unwrap() {
            Event::Login { nickname, uin } => {
                assert_eq!((nickname.as_str(), uin.as_str()), ("gugu", "123456"))
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(slots.login, None); // consumed
    }

    #[test]
    fn reply_error_is_dropped() {
        let mut slots = ReplySlots {
            login: Some("9".to_string()),
            ..Default::default()
        };
        let v: Value = serde_json::json!({
            "status": "failed", "retcode": 100, "wording": "bad param", "echo": "9"
        });
        assert!(parse_reply(&v, &mut slots).is_none());
        assert_eq!(slots.login, Some("9".to_string())); // still pending: failure ≠ login
    }

    /// Read one text frame as JSON from the mock endpoint; panic on timeout.
    async fn next_frame<W>(src: &mut W) -> Value
    where
        W: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        const WAIT: Duration = Duration::from_secs(10);
        let msg = timeout(WAIT, src.next())
            .await
            .expect("mock endpoint frame timed out")
            .expect("websocket error")
            .expect("frame");
        match msg {
            Message::Text(t) => serde_json::from_str(&t).expect("frame is not valid JSON"),
            other => panic!("expected text frame, got {other:?}"),
        }
    }

    /// Start the real transport against an in-process WS server and accept
    /// one connection. Returns the action handle, the server-side socket
    /// halves, and the event callback receiver. The listener is dropped
    /// after accept; later reconnects fail fast, which is fine per-test.
    async fn loopback() -> (
        Handle,
        futures_util::stream::SplitSink<Ws, Message>,
        futures_util::stream::SplitStream<Ws>,
        std::sync::mpsc::Receiver<Event>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Event>();
        let handle = spawn(
            Config {
                ws_url: format!("ws://{addr}/onebot"),
                access_token: "sekret".into(),
            },
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        let (stream, _peer) = timeout(Duration::from_secs(10), listener.accept())
            .await
            .expect("client did not connect")
            .unwrap();
        let ws = timeout(
            Duration::from_secs(10),
            tokio_tungstenite::accept_async(tokio_tungstenite::MaybeTlsStream::Plain(stream)),
        )
        .await
        .expect("handshake timed out")
        .unwrap();
        let (sink, src) = ws.split();
        (handle, sink, src, rx)
    }

    /// Full loopback: real WS server in-process, real transport thread.
    /// Connect → Connected event → auto get_login_info → Login event →
    /// auto get_friend_list/get_group_list request frames → queued
    /// SendPrivate arrives as a send_private_msg frame.
    #[tokio::test]
    async fn roundtrip_connect_login_send() {
        let (handle, mut sink, mut src, rx) = loopback().await;

        // On connect the transport must identify itself…
        let f = next_frame(&mut src).await;
        assert_eq!(f["action"], "get_login_info");
        assert!(f["echo"].is_string());
        // …and immediately pull the roster. Both list replies stay
        // unanswered here, so no Roster event fires; this pins frame order.
        assert_eq!(next_frame(&mut src).await["action"], "get_friend_list");
        assert_eq!(next_frame(&mut src).await["action"], "get_group_list");
        let reply = json!({
            "status": "ok", "retcode": 0, "echo": f["echo"],
            "data": { "user_id": 10001, "nickname": "smoke" }
        });
        sink.send(Message::Text(reply.to_string().into()))
            .await
            .unwrap();

        // …and surface Connected, then Login, on the callback.
        let e = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("no Connected");
        assert!(matches!(e, Event::Connected), "{e:?}");
        let e = rx.recv_timeout(Duration::from_secs(10)).expect("no Login");
        match e {
            Event::Login { nickname, uin } => {
                assert_eq!((nickname.as_str(), uin.as_str()), ("smoke", "10001"));
            }
            other => panic!("expected Login, got {other:?}"),
        }

        // Actions queued on the Handle come out as OneBot frames.
        handle.send(Action::SendPrivate {
            user_id: 42,
            text: "hi".into(),
        });
        let f = next_frame(&mut src).await;
        assert_eq!(f["action"], "send_private_msg");
        assert_eq!(f["params"]["user_id"], 42);
        assert_eq!(f["params"]["message"], "hi");

        // Dropping sink/listener here ends the session; the transport thread
        // retries the dead port until the test process exits — harmless.
    }

    /// A friends-only reply must not fire an event: the answered slot is
    /// consumed, the pending groups slot survives, half data is retained.
    #[test]
    fn roster_waits_for_the_second_half() {
        let mut slots = ReplySlots {
            friends: Some("1".to_string()),
            groups: Some("2".to_string()),
            ..Default::default()
        };
        let v: Value = json!({
            "retcode": 0, "echo": "1",
            "data": [{ "user_id": 11, "nickname": "a", "remark": "" }]
        });
        assert!(
            parse_reply(&v, &mut slots).is_none(),
            "one half must not fire"
        );
        assert_eq!(slots.friends, None, "answered slot must be consumed");
        assert_eq!(
            slots.groups,
            Some("2".to_string()),
            "pending slot must stay"
        );
        assert!(slots.roster_friends.is_some());
        assert!(slots.roster_groups.is_none());
    }

    /// Both halves merge into one Roster: friends first, non-empty remark
    /// over nickname, group entries named by `group_name`.
    #[test]
    fn roster_merges_friends_then_groups() {
        let mut slots = ReplySlots {
            friends: Some("1".to_string()),
            groups: Some("2".to_string()),
            ..Default::default()
        };
        let friends: Value = json!({
            "retcode": 0, "echo": "1",
            "data": [
                { "user_id": 11, "nickname": "nick", "remark": "备注" },
                { "user_id": 12, "nickname": "plain", "remark": "" }
            ]
        });
        assert!(parse_reply(&friends, &mut slots).is_none());
        let groups: Value = json!({
            "retcode": 0, "echo": "2",
            "data": [{ "group_id": 99, "group_name": "群名" }]
        });
        match parse_reply(&groups, &mut slots).unwrap() {
            Event::Roster { peers } => assert_eq!(
                peers
                    .iter()
                    .map(|p| (p.id, p.name.as_str(), p.is_group))
                    .collect::<Vec<_>>(),
                [
                    (11, "备注", false),
                    (12, "plain", false),
                    (99, "群名", true)
                ]
            ),
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(slots.roster_friends.is_none() && slots.roster_groups.is_none());
    }

    /// Entries missing their id field are skipped; a missing `remark` key
    /// falls back to `nickname` without dropping the entry.
    #[test]
    fn roster_skips_entries_without_id() {
        let mut slots = ReplySlots {
            friends: Some("1".to_string()),
            groups: Some("2".to_string()),
            ..Default::default()
        };
        let friends: Value = json!({
            "retcode": 0, "echo": "1",
            "data": [{ "nickname": "no-id" }, { "user_id": 12, "nickname": "ok" }]
        });
        assert!(parse_reply(&friends, &mut slots).is_none());
        let groups: Value = json!({
            "retcode": 0, "echo": "2",
            "data": [{ "group_name": "no-id" }, { "group_id": 99, "group_name": "g" }]
        });
        match parse_reply(&groups, &mut slots).unwrap() {
            Event::Roster { peers } => {
                assert_eq!(peers.iter().map(|p| p.id).collect::<Vec<_>>(), [12i64, 99])
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// A failed roster reply is dropped before slot matching: the pending
    /// echo and the merged halves stay untouched (clean retry on reconnect).
    #[test]
    fn roster_error_reply_keeps_slots_clean() {
        let mut slots = ReplySlots {
            friends: Some("1".to_string()),
            groups: Some("2".to_string()),
            ..Default::default()
        };
        let bad: Value = json!({ "retcode": 100, "wording": "boom", "echo": "1" });
        assert!(parse_reply(&bad, &mut slots).is_none());
        assert_eq!(slots.friends, Some("1".to_string()));
        assert!(slots.roster_friends.is_none());
    }

    /// End-to-end Roster over the real thread: answer all three startup
    /// requests with wire-shaped data; exactly one merged event fires,
    /// friends (remark over nickname) first, then groups.
    #[tokio::test]
    async fn roundtrip_roster_event() {
        let (_handle, mut sink, mut src, rx) = loopback().await;
        let f = next_frame(&mut src).await;
        assert_eq!(f["action"], "get_login_info");
        let login_echo = f["echo"].as_str().unwrap().to_string();
        let f = next_frame(&mut src).await;
        assert_eq!(f["action"], "get_friend_list");
        let friends_echo = f["echo"].as_str().unwrap().to_string();
        let f = next_frame(&mut src).await;
        assert_eq!(f["action"], "get_group_list");
        let groups_echo = f["echo"].as_str().unwrap().to_string();

        for (echo, data) in [
            (login_echo, json!({ "user_id": 1, "nickname": "bot" })),
            (
                friends_echo,
                json!([
                    { "user_id": 21, "nickname": "fr", "remark": "老remark" },
                    { "user_id": 22, "nickname": "fr2", "remark": "" }
                ]),
            ),
            (groups_echo, json!([{ "group_id": 31, "group_name": "g1" }])),
        ] {
            let reply = json!({ "status": "ok", "retcode": 0, "echo": echo, "data": data });
            sink.send(Message::Text(reply.to_string().into()))
                .await
                .unwrap();
        }

        let e = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("no Connected");
        assert!(matches!(e, Event::Connected), "{e:?}");
        let e = rx.recv_timeout(Duration::from_secs(10)).expect("no Login");
        assert!(matches!(e, Event::Login { .. }), "{e:?}");
        let e = rx.recv_timeout(Duration::from_secs(10)).expect("no Roster");
        match e {
            Event::Roster { peers } => assert_eq!(
                peers
                    .iter()
                    .map(|p| (p.id, p.name.as_str(), p.is_group))
                    .collect::<Vec<_>>(),
                [
                    (21, "老remark", false),
                    (22, "fr2", false),
                    (31, "g1", true)
                ]
            ),
            other => panic!("expected Roster, got {other:?}"),
        }
        // Exactly one Roster: the halves were consumed by the merge.
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
