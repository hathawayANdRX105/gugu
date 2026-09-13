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

/// Event delivered to the UI layer on the background thread.
#[derive(Debug)]
pub enum Event {
    /// Reply to the automatic `get_login_info` after (re)connecting.
    Login {
        nickname: String,
        #[allow(dead_code)] // read once the friend-list PR wires it; remove then
        uin: String,
    },
    /// WS established (including after a reconnect).
    Connected,
    /// Private (`message_type == "private"`) message.
    PrivateMessage {
        #[allow(dead_code)] // read once the friend-list PR wires it; remove then
        sender_id: i64,
        sender_name: String,
        text: String,
        time: i64,
    },
    /// Group (`message_type == "group"`) message.
    GroupMessage {
        #[allow(dead_code)] // read once the friend-list PR wires it; remove then
        group_id: i64,
        #[allow(dead_code)] // read once the friend-list PR wires it; remove then
        sender_id: i64,
        sender_name: String,
        text: String,
        time: i64,
    },
}

/// Action accepted on [`Handle::send`]; serialized to a OneBot v11 frame.
pub enum Action {
    /// `send_private_msg`
    #[allow(dead_code)] // sent once the friend-list PR wires it; remove then
    SendPrivate { user_id: i64, text: String },
    /// `send_group_msg`
    #[allow(dead_code)] // sent once the friend-list PR wires it; remove then
    SendGroup { group_id: i64, text: String },
    /// `get_login_info`
    GetLoginInfo,
}

/// Sender side of the action queue; cheap to clone out of the UI thread.
#[allow(dead_code)] // kept by the UI once the friend-list PR wires it; remove then
pub struct Handle {
    tx: Sender<Action>,
}

impl Handle {
    /// Queue an action. Dropped if the transport thread is gone (it shouldn't
    /// be: the thread survives disconnects via the reconnect loop).
    #[allow(dead_code)] // called once the friend-list PR wires it; remove then
    pub fn send(&self, action: Action) {
        let _ = self.tx.send(action);
    }
}

/// Spawn the transport thread (std::thread + current-thread tokio runtime).
///
/// `on_event` runs on that thread — bridge into Slint with
/// `slint::invoke_from_event_loop` before touching UI state. The thread
/// connects, and on every successful (re)connection emits [`Event::Connected`]
/// and sends `get_login_info`. Disconnects retry with [`BACKOFF`] delays.
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
    let mut login_echo: Option<String> = None;
    // The connect path always asks who we are; remember its echo.
    send_action(&mut sink, Action::GetLoginInfo, echo, &mut login_echo).await;
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
                    parse_reply(&v, &mut login_echo)
                } else {
                    None // meta_event / heartbeat etc.
                };
                if let Some(ev) = ev {
                    on_event(ev);
                }
            }
            action = arx.recv() => {
                let Some(action) = action else { return }; // sender dropped
                send_action(&mut sink, action, echo, &mut login_echo).await;
            }
        }
    }
}

/// Serialize one action to a frame and write it. `echo` is a per-connection
/// string of the monotonic counter; `get_login_info` records its echo so the
/// matching reply becomes [`Event::Login`].
async fn send_action(
    sink: &mut futures_util::stream::SplitSink<Ws, Message>,
    action: Action,
    echo: &mut i64,
    login_echo: &mut Option<String>,
) {
    *echo += 1;
    let id = echo.to_string();
    let (name, params) = match action {
        Action::SendPrivate { user_id, text } => (
            "send_private_msg",
            json!({ "user_id": user_id, "message": text }),
        ),
        Action::SendGroup { group_id, text } => (
            "send_group_msg",
            json!({ "group_id": group_id, "message": text }),
        ),
        Action::GetLoginInfo => {
            *login_echo = Some(id.clone());
            ("get_login_info", json!({}))
        }
    };
    let frame = json!({ "action": name, "params": params, "echo": id });
    if let Err(e) = sink.send(Message::Text(frame.to_string().into())).await {
        eprintln!("onebot: send {name} failed: {e}");
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

/// Parse an API reply frame. `retcode != 0` → log and drop; echo matching the
/// pending `get_login_info` → [`Event::Login`]; anything else → `None`.
fn parse_reply(v: &Value, login_echo: &mut Option<String>) -> Option<Event> {
    let retcode = v["retcode"].as_i64().unwrap_or(-1);
    if retcode != 0 {
        eprintln!(
            "onebot: action failed retcode={retcode} wording={}",
            v["wording"].as_str().unwrap_or("")
        );
        return None;
    }
    if v["echo"].as_str()? != login_echo.as_deref()? {
        return None;
    }
    *login_echo = None;
    Some(Event::Login {
        nickname: v["data"]["nickname"].as_str()?.to_string(),
        uin: v["data"]["user_id"].as_i64()?.to_string(),
    })
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
        let mut pending = Some("7".to_string());
        let v: Value = serde_json::json!({
            "status": "ok", "retcode": 0, "echo": "7",
            "data": { "user_id": 123456, "nickname": "gugu" }
        });
        match parse_reply(&v, &mut pending).unwrap() {
            Event::Login { nickname, uin } => {
                assert_eq!((nickname.as_str(), uin.as_str()), ("gugu", "123456"))
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(pending, None); // consumed
    }

    #[test]
    fn reply_error_is_dropped() {
        let mut pending = Some("9".to_string());
        let v: Value = serde_json::json!({
            "status": "failed", "retcode": 100, "wording": "bad param", "echo": "9"
        });
        assert!(parse_reply(&v, &mut pending).is_none());
        assert_eq!(pending, Some("9".to_string())); // still pending: failure ≠ login
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

    /// Full loopback: real WS server in-process, real transport thread.
    /// Connect → Connected event → auto get_login_info → Login event →
    /// queued SendPrivate arrives as a send_private_msg frame.
    #[tokio::test]
    async fn roundtrip_connect_login_send() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
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
            tokio_tungstenite::accept_async(stream),
        )
        .await
        .expect("handshake timed out")
        .unwrap();
        let (mut sink, mut src) = ws.split();

        // On connect the transport must identify itself…
        let f = next_frame(&mut src).await;
        assert_eq!(f["action"], "get_login_info");
        assert!(f["echo"].is_string());
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
}
