// 咕咕 gugu — lightweight QQ chat client (Slint software renderer)
//
// Animated stickers: Slint has no native GIF animation (upstream #2081).
// We decode all frames once with `image`, then a single `slint::Timer`
// advances one animated row per tick (round-robin) and publishes its frame
// through the `StickerAnim` global. Rows pick the frame up by uid, so the
// message model is never rewritten per frame — rewriting a row marked its
// layout dirty, which re-laid-out the whole scroll pane every tick.

use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod convo;
mod onebot;
mod stickers;
use stickers::{load_gif, scan_packs, StickerFrames, StickerPack};

slint::include_modules!();

// GIF decoding lives in `stickers` — it also backs the sticker-pack scanner.

/// Avatar letter: first char uppercased. `.slint` has no substring/charAt,
/// so every text derivation happens here.
fn initial(name: &str) -> SharedString {
    name.chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .collect::<String>()
        .into()
}

/// Stable avatar palette index (0..7) for a name.
fn color_of(name: &str) -> i32 {
    (name.chars().map(|c| c as u32).sum::<u32>() % 8) as i32
}

/// Minutes since midnight (UTC); mock-clock granularity, one minute.
fn now_minutes() -> i64 {
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60
        % 1440) as i64
}

/// Format minutes since midnight as "HH:MM".
fn fmt_time(min: i64) -> SharedString {
    format!("{:02}:{:02}", (min / 60) % 24, min % 60).into()
}

/// Columns in the sticker picker nine-grid.
const GRID_COLS: usize = 3;
/// One grid cell edge in px; must match the `.slint` cell size.
const CELL_PX: f32 = 72.0;

/// Rebuild the picker grid for one pack: first-frame thumbnails (the grid
/// itself never animates) laid out row-major, `GRID_COLS` per row.
/// A missing pack index yields an empty grid — the panel then shows its
/// "没有表情包" placeholder instead.
fn show_pack(ui: &MainWindow, packs: &[StickerPack], pack: usize) {
    let cells: Vec<StickerCell> = packs
        .get(pack)
        .map(|p| {
            p.stickers
                .iter()
                .enumerate()
                .map(|(i, (_, f))| StickerCell {
                    image: f.frames.first().cloned().unwrap_or_default(),
                    pack: pack as i32,
                    index: i as i32,
                    row: (i / GRID_COLS) as i32,
                    col: (i % GRID_COLS) as i32,
                })
                .collect()
        })
        .unwrap_or_default();
    let rows = cells.len().div_ceil(GRID_COLS);
    ui.set_grid_height((rows as f32) * CELL_PX);
    ui.set_sticker_cells(Rc::new(VecModel::from(cells)).into());
}

/// Stable row identity: assigned once at insert and never re-assigned, so
/// the animation loop can address a row without ever rewriting the model.
static NEXT_UID: AtomicI32 = AtomicI32::new(0);

fn next_uid() -> i32 {
    NEXT_UID.fetch_add(1, Ordering::Relaxed)
}

/// One registered sticker animation: the row's `uid`, its decoded frames,
/// and the index of the frame currently on display. Rows are append-only,
/// so entries are never removed.
struct AnimRow {
    uid: i32,
    frames: Rc<StickerFrames>,
    idx: usize,
}

/// Advance one animated row per tick, round-robin, and publish its current
/// frame through the `StickerAnim` global. Because the message model is
/// never touched, no row's layout is re-dirtied by the animation — only the
/// active row's `Image` repaints. The interval is re-armed each tick to the
/// frame's own delay.
fn start_anim_loop(
    ui: Weak<MainWindow>,
    anims: Rc<RefCell<Vec<AnimRow>>>,
    cursor: Rc<Cell<usize>>,
    timer: Rc<Timer>,
) {
    let timer_c = timer.clone();
    // Last published (uid, frame index): Slint property sets mark the window
    // dirty even when the value is identical, and each needless dirty spins
    // the render loop (measured 97% CPU on a single-frame sticker) — so the
    // tick below publishes only when the displayed frame genuinely changes.
    let last = Rc::new(Cell::new((-1i32, 0usize)));
    timer.start(
        TimerMode::Repeated,
        Duration::from_millis(20), // first tick is immediate-ish; the loop re-arms per frame
        move || {
            let Some(ui) = ui.upgrade() else { return };
            let globals = ui.global::<StickerAnim>();
            let mut rows = anims.borrow_mut();
            // ponytail: rows are append-only, so this stays cold; stopping a
            // Repeated timer from its own callback would leave the loop unable
            // to restart on a later registration, so it just idles instead.
            if rows.is_empty() {
                if last.get().0 != -1 {
                    globals.set_active_uid(-1);
                    last.set((-1, 0));
                }
                return;
            }
            let i = cursor.get() % rows.len();
            let row = &mut rows[i];
            let next = (row.idx + 1) % row.frames.frames.len();
            if (row.uid, next) != last.get() {
                globals.set_active_uid(row.uid);
                globals.set_frame(row.frames.frames[next].clone());
                last.set((row.uid, next));
            }
            row.idx = next;
            timer_c.set_interval(Duration::from_millis(row.frames.delays[next]));
            cursor.set((i + 1) % rows.len());
        },
    );
}

/// Register one row's animation. The loop is started lazily on the first
/// registration and then runs for the window's lifetime; the round-robin
/// cursor picks newly registered rows up on the next tick.
///
/// Single-frame stickers are skipped: `msg.sticker` already renders their
/// only frame, and animating them would re-publish identical frames forever.
fn register_anim(
    ui: &Weak<MainWindow>,
    anims: &Rc<RefCell<Vec<AnimRow>>>,
    cursor: &Rc<Cell<usize>>,
    timer: &Rc<Timer>,
    uid: i32,
    frames: Rc<StickerFrames>,
) {
    if frames.frames.len() <= 1 {
        return;
    }
    anims.borrow_mut().push(AnimRow {
        uid,
        frames,
        idx: 0,
    });
    if !timer.running() {
        start_anim_loop(ui.clone(), anims.clone(), cursor.clone(), timer.clone());
    }
}

// Message model + conversation store stashed for the onebot bridge.
// `Rc` is `!Send`, so the transport thread's closure can't carry it; the
// `invoke_from_event_loop` callback runs back on this (UI) thread and
// picks the handles up here.
type BridgeState = (Rc<VecModel<Message>>, Rc<RefCell<convo::Store<Message>>>);

thread_local! {
    static BRIDGE: RefCell<Option<BridgeState>> = const { RefCell::new(None) };
}

/// Append one incoming OneBot message to the active conversation's model
/// (or its unread counter, if the user has since switched away). Grouping
/// is per-conversation via [`convo::Store`]; demo-mode rows share the
/// [`convo::ConvKey::DEMO`] bucket and behave exactly as before routing
/// existed.
fn push_incoming(key: convo::ConvKey, author: &str, body: &str, unix: i64) {
    // ponytail: UTC minute-of-day like now_minutes(); TZ handling is M4's.
    let min = unix / 60 % 1440;
    BRIDGE.with(|b| {
        let borrowed = b.borrow();
        let Some((model, store)) = borrowed.as_ref() else {
            return;
        };
        let mut row = Message {
            author: author.into(),
            initial: initial(author),
            uid: next_uid(),
            body: body.into(),
            time: fmt_time(min),
            color: color_of(author),
            sticker: slint::Image::default(),
            grouped: false,
            // Incoming rows are never send-tracked.
            client_id: 0,
            status: SendStatus::Sent,
        };
        let landed = store.borrow_mut().push(key, &mut row, author, min);
        if landed {
            model.push(row);
        }
    });
}

/// Flip the send status of the row whose `uid == client_id` in the
/// conversation store — across all conversations, because the outcome may
/// arrive after the user switched away. If that row is in the active
/// conversation's displayed model, mirror the flip there too. One
/// `set_row_data` per transition (at most two per row's lifetime); the
/// animation path never touches the model, so this never feeds the
/// per-frame redraw pump. No-op when the row is gone (e.g. after restart).
fn set_send_status(client_id: i64, status: SendStatus) {
    BRIDGE.with(|b| {
        let borrowed = b.borrow();
        let Some((model, store)) = borrowed.as_ref() else {
            return;
        };
        let mut found = false;
        {
            let mut store = store.borrow_mut();
            for row in store.rows_mut() {
                if row.uid as i64 == client_id {
                    row.status = status;
                    found = true;
                }
            }
        }
        if !found {
            return;
        }
        // The displayed model holds the active conversation's rows only; if
        // the row is visible there, mirror the flip (bounded: one set_row_data).
        let mut i = 0;
        while i < model.row_count() {
            match model.row_data(i) {
                Some(mut m) if m.uid as i64 == client_id => {
                    m.status = status;
                    model.set_row_data(i, m);
                    return;
                }
                Some(_) => i += 1,
                None => return,
            }
        }
    });
}
impl convo::HasRowMeta for Message {
    fn set_grouped(&mut self, grouped: bool) {
        self.grouped = grouped;
    }
}

/// Conversation key for a sidebar row: the row's peer id, or
/// [`convo::ConvKey::DEMO`] for header/demo rows that carry none.
fn channel_key(ch: &Channel) -> convo::ConvKey {
    match ch.peer_id.parse::<i64>() {
        Ok(id) => convo::ConvKey {
            id,
            is_group: ch.is_group,
        },
        Err(_) => convo::ConvKey::DEMO,
    }
}
/// Switch the displayed conversation: activate `idx`'s key in the store,
/// clear its unread counter, and reflow the message model in place from
/// the store's transcript. Row uids come from the store, so the animation
/// loop's uid routing stays stable across the switch.
fn select_conversation(
    ui: &MainWindow,
    model: &Rc<VecModel<Message>>,
    store: &Rc<RefCell<convo::Store<Message>>>,
    idx: i32,
) {
    let key = ui
        .get_channels()
        .row_data(idx.max(0) as usize)
        .as_ref()
        .map(channel_key)
        .unwrap_or(convo::ConvKey::DEMO);
    let rows: Vec<Message> = {
        let mut st = store.borrow_mut();
        st.set_active(key);
        st.clear_unread(key);
        st.entries(key).to_vec()
    };
    // ponytail: set_vec resets the model in one shot; uids come from the
    // store, so the animation loop's row routing survives the switch.
    model.set_vec(rows);
}

/// Republish unread counts on every sidebar row. Header rows stay at 0.
fn refresh_unread(ui: &MainWindow, store: &Rc<RefCell<convo::Store<Message>>>) {
    let channels = ui.get_channels();
    for i in 0..channels.row_count() {
        if let Some(mut ch) = channels.row_data(i) {
            if !ch.header {
                ch.unread = store.borrow().unread(channel_key(&ch)) as i32;
                channels.set_row_data(i, ch);
            }
        }
    }
}
/// [`refresh_unread`] for callers that only have the UI handle at hand:
/// pulls the store out of the thread-local [`BRIDGE`]. No-op in demo mode,
/// where the bridge is unregistered and there is no roster either.
fn refresh_unread_via_bridge(ui: &MainWindow) {
    BRIDGE.with(|b| {
        if let Some((_, store)) = b.borrow().as_ref() {
            refresh_unread(ui, store);
        }
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;

    // ---- mock data (ricq protocol layer replaces this later) ----
    let servers: Vec<Server> = ["咕咕", "喵星", "像素社", "开发组", "摸鱼群"]
        .iter()
        .map(|n| Server {
            name: (*n).into(),
            initial: initial(n),
            color: color_of(n),
        })
        .collect();
    ui.set_servers(Rc::new(VecModel::from(servers)).into());

    let channels: Vec<Channel> = [
        ("频道", true),
        ("#general", false),
        ("#random", false),
        ("#memes", false),
        ("#dev", false),
        ("私信", true),
        ("alice", false),
        ("bob", false),
    ]
    .iter()
    .map(|(n, h)| Channel {
        name: (*n).into(),
        header: *h,
        // Demo rows carry no peer id → sends stay local echo (see on_send).
        peer_id: "".into(),
        is_group: false,
        unread: 0,
    })
    .collect();
    ui.set_channels(Rc::new(VecModel::from(channels)).into());
    ui.set_selected(1); // first real channel, not the section header

    let members: Vec<Member> = [
        "alice", "bob", "carol", "dave", "erin", "frank", "grace", "heidi",
    ]
    .iter()
    .map(|n| Member {
        name: (*n).into(),
        initial: initial(n),
        color: color_of(n),
    })
    .collect();
    ui.set_members(Rc::new(VecModel::from(members)).into());

    // (author, body, minute-of-day, has sticker)
    let mock: &[(&str, &str, i64, bool)] = &[
        ("alice", "早上好，各位", 20 * 60, false),
        ("alice", "昨晚那版构建结果怎么样？", 20 * 60 + 1, false),
        ("alice", "CI 全绿了", 20 * 60 + 2, false),
        ("bob", "牛啊", 20 * 60 + 4, false),
        ("bob", "我本地跑了一下午没复现", 20 * 60 + 5, false),
        ("carol", "能发了吗", 20 * 60 + 7, false),
        (
            "alice",
            "同一个人隔了 20 分钟，重开一组",
            20 * 60 + 20,
            false,
        ),
        ("bob", "看这个贴纸", 20 * 60 + 24, true),
        ("dave", "哈哈哈有点可爱", 20 * 60 + 25, false),
        ("erin", "下午三点会议室过一版", 20 * 60 + 29, false),
        ("erin", "记得带周报", 20 * 60 + 30, false),
        ("grace", "收到", 20 * 60 + 31, false),
    ];

    let gif: Rc<StickerFrames> = Rc::new(
        load_gif(Path::new("assets/sticker.gif")).expect("bundled demo sticker must decode"),
    );

    // Demo transcript: one DEMO bucket in the store; grouping computed per
    // conversation (here the whole transcript). Rows land in the model via
    // the store, so the active/switch machinery below shares this data.
    let store = Rc::new(RefCell::new(convo::Store::new()));
    store.borrow_mut().set_active(convo::ConvKey::DEMO);
    let mut msgs: Vec<Message> = Vec::new();
    let mut sticker_uid = -1;
    for (author, body, min, sticker) in mock {
        let uid = next_uid();
        if *sticker {
            sticker_uid = uid;
        }
        let mut row = Message {
            author: (*author).into(),
            initial: initial(author),
            body: (*body).into(),
            time: fmt_time(*min),
            color: color_of(author),
            sticker: if *sticker {
                gif.frames[0].clone()
            } else {
                slint::Image::default()
            },
            grouped: false,
            uid,
            // Demo rows are never send-tracked.
            client_id: 0,
            status: SendStatus::Sent,
        };
        // Active DEMO bucket: push lands in-transcript; model gets the row too.
        store
            .borrow_mut()
            .push(convo::ConvKey::DEMO, &mut row, author, *min);
        msgs.push(row);
    }
    let model = Rc::new(VecModel::from(msgs));
    ui.set_messages(ModelRc::from(model.clone()));

    // ---- send ----
    let ui_weak = ui.as_weak();
    let model_send = model.clone();
    let store_send = store.clone();
    let store_bridge = store.clone();

    // ---- onebot (M2): data/config.toml present → live events; absent → demo ----
    // Spawned before `on_send` so sends can address the selected peer. The
    // transport thread survives disconnects (reconnect loop), so `Some` here
    // means "configured"; actions queued while down drain on the next session.
    let ob_handle = Rc::new(match onebot::Config::load(Path::new("data/config.toml")) {
        Ok(config) => {
            BRIDGE.with(|b| *b.borrow_mut() = Some((model.clone(), store_bridge)));
            let weak_ob = ui.as_weak();
            // on_event runs on the transport thread; only Send values may be
            // captured, so the event itself hops into the UI event loop and
            // the Rc handles come back out of the thread-local BRIDGE.
            Some(onebot::spawn(
                config,
                Box::new(move |ev| {
                    let weak = weak_ob.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = weak.upgrade() else { return };
                        match ev {
                            onebot::Event::Connected => ui.set_conn_label("已连接".into()),
                            onebot::Event::Login { nickname, .. } => {
                                ui.set_conn_label(format!("已连接 · {nickname}").into());
                            }
                            onebot::Event::PrivateMessage {
                                sender_id,
                                sender_name,
                                text,
                                time,
                                ..
                            } => {
                                let key = convo::ConvKey {
                                    id: sender_id,
                                    is_group: false,
                                };
                                push_incoming(key, &sender_name, &text, time);
                                refresh_unread_via_bridge(&ui);
                            }
                            onebot::Event::GroupMessage {
                                group_id,
                                sender_name,
                                text,
                                time,
                                ..
                            } => {
                                let key = convo::ConvKey {
                                    id: group_id,
                                    is_group: true,
                                };
                                push_incoming(key, &sender_name, &text, time);
                                refresh_unread_via_bridge(&ui);
                            }
                            // T5: real friends/groups replace the mock sidebar. Two
                            // segments, header row first; QQ ids travel as strings
                            // (slint `int` is i32 and cannot hold them).
                            onebot::Event::Roster { peers } => {
                                let mut rows: Vec<Channel> = Vec::new();
                                for (label, group) in [("私信", false), ("群聊", true)] {
                                    let seg: Vec<&onebot::Peer> =
                                        peers.iter().filter(|p| p.is_group == group).collect();
                                    if seg.is_empty() {
                                        continue;
                                    }
                                    rows.push(Channel {
                                        name: label.into(),
                                        header: true,
                                        peer_id: "".into(),
                                        is_group: false,
                                        unread: 0,
                                    });
                                    rows.extend(seg.into_iter().map(|p| Channel {
                                        name: p.name.as_str().into(),
                                        header: false,
                                        peer_id: p.id.to_string().into(),
                                        is_group: p.is_group,
                                        unread: 0,
                                    }));
                                }
                                let first = rows.iter().position(|r| !r.header).unwrap_or(0) as i32;
                                ui.set_channels(Rc::new(VecModel::from(rows)).into());
                                ui.set_selected(first);
                                BRIDGE.with(|b| {
                                    if let Some((model, store)) = b.borrow().as_ref() {
                                        select_conversation(&ui, model, store, first);
                                    }
                                });
                            }
                            // Send retry bookkeeping: route the outcome back
                            // to the row that sent it (uid == client_id).
                            onebot::Event::SendOk {
                                client_id,
                                message_id,
                            } => {
                                eprintln!(
                                    "gugu: send ok for row {client_id} (message_id {message_id})"
                                );
                                set_send_status(client_id, SendStatus::Sent);
                            }
                            onebot::Event::SendFailed { client_id } => {
                                set_send_status(client_id, SendStatus::Failed);
                            }
                        }
                    });
                }),
            ))
        }
        Err(e) => {
            eprintln!("未找到 data/config.toml，运行在演示模式: {e}");
            None
        }
    });
    let ob_send = ob_handle.clone();
    ui.on_send(move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        let draft = ui.get_draft().to_string();
        if draft.trim().is_empty() {
            return;
        }
        // One uid per outgoing row doubles as the retry machinery's
        // client_id, so SendOk/SendFailed route straight back to this row.
        let send_uid = next_uid();
        // T5 addressing: the selected sidebar row carries the peer id. Header
        // rows (peer_id ""), unparseable ids and demo mode echo locally only.
        let target = ui
            .get_channels()
            .row_data(ui.get_selected().max(0) as usize);
        let key = target
            .as_ref()
            .map(channel_key)
            .unwrap_or(convo::ConvKey::DEMO);
        if let (Some(ob), Some(ch)) = (ob_send.as_ref(), target) {
            if let Ok(peer_id) = ch.peer_id.parse::<i64>() {
                ob.send(if ch.is_group {
                    onebot::Action::SendGroup {
                        client_id: send_uid as i64,
                        group_id: peer_id,
                        text: draft.clone(),
                    }
                } else {
                    onebot::Action::SendPrivate {
                        client_id: send_uid as i64,
                        user_id: peer_id,
                        text: draft.clone(),
                    }
                });
            }
        }
        let min = now_minutes();
        let mut row = Message {
            author: "you".into(),
            initial: initial("you"),
            body: draft.into(),
            time: fmt_time(min),
            color: color_of("you"),
            sticker: slint::Image::default(),
            grouped: false,
            uid: send_uid,
            client_id: send_uid,
            status: SendStatus::Sent,
        };
        // Outbound rows land in the selected conversation: active → the
        // displayed model too; the store owns the grouping decision.
        let landed = store_send.borrow_mut().push(key, &mut row, "you", min);
        if landed {
            model_send.push(row);
        }
        ui.set_draft("".into());
    });

    // ---- channel select ----
    let ui_weak2 = ui.as_weak();
    let model_sel = model.clone();
    let store_sel = store.clone();
    ui.on_select_channel(move |idx| {
        if let Some(ui) = ui_weak2.upgrade() {
            ui.set_selected(idx);
            select_conversation(&ui, &model_sel, &store_sel, idx);
        }
    });

    // ---- sticker animation ----
    // One timer drives every animated row (round-robin over the registry
    // below); kept alive for the window's lifetime. Frames are published
    // through the StickerAnim global, never through the model.
    let anims: Rc<RefCell<Vec<AnimRow>>> = Rc::new(RefCell::new(Vec::new()));
    let anim_cursor = Rc::new(Cell::new(0usize));
    let anim_timer = Rc::new(Timer::default());
    register_anim(
        &ui.as_weak(),
        &anims,
        &anim_cursor,
        &anim_timer,
        sticker_uid,
        gif.clone(),
    );

    // ---- sticker picker (M3): scan_packs at startup, no hardcoded list ----
    // Memory budget (AGENTS #1): packs decode once at startup; demo assets
    // are a few KB. M4's media-cache LRU bounds real user packs.
    let packs = Rc::new(scan_packs(Path::new("assets/stickers")));
    let pack_names: Vec<SharedString> = packs.iter().map(|p| p.name.as_str().into()).collect();
    ui.set_pack_names(Rc::new(VecModel::from(pack_names)).into());
    show_pack(&ui, &packs, 0);

    let ui_weak3 = ui.as_weak();
    let packs_tab = packs.clone();
    ui.on_select_pack(move |idx| {
        if let Some(ui) = ui_weak3.upgrade() {
            show_pack(&ui, &packs_tab, idx as usize);
        }
    });
    let model_pick = model.clone();
    let store_pick = store.clone();
    let packs_pick = packs.clone();
    let anims_pick = anims.clone();
    let cursor_pick = anim_cursor.clone();
    let timer_pick = anim_timer.clone();
    let ui_weak_pick = ui.as_weak();
    ui.on_pick_sticker(move |pack, index| {
        // scan_packs never fails; a bad index just drops the click.
        let Some((_, decoded)) = packs_pick
            .get(pack as usize)
            .and_then(|p| p.stickers.get(index as usize))
        else {
            return;
        };
        let frames = Rc::new(decoded.clone());

        let min = now_minutes();
        let uid = next_uid();
        let mut row = Message {
            author: "you".into(),
            initial: initial("you"),
            body: "".into(),
            time: fmt_time(min),
            color: color_of("you"),
            sticker: frames.frames.first().cloned().unwrap_or_default(),
            grouped: false,
            uid,
            // Local sticker picks are never send-tracked (image channel is
            // batch seven; once it ships, picked stickers go through it).
            client_id: 0,
            status: SendStatus::Sent,
        };
        // Picked stickers belong to the active conversation; the store
        // decides grouping, and the landed row joins the displayed model.
        let active = store_pick.borrow().active().unwrap_or(convo::ConvKey::DEMO);
        let landed = store_pick.borrow_mut().push(active, &mut row, "you", min);
        if landed {
            model_pick.push(row);
        }
        // A static (single-frame) sticker has nothing to animate.
        if frames.frames.len() > 1 {
            register_anim(
                &ui_weak_pick,
                &anims_pick,
                &cursor_pick,
                &timer_pick,
                uid,
                frames,
            );
        }
    });

    // Smoke seam (same GUGU_* convention): GUGU_SMOKE_ONEBOT pushes one
    // synthetic event through the bridge — verifies the message/conn-label
    // display path without a live WS peer. No-op unless config loaded too.
    if std::env::var_os("GUGU_SMOKE_ONEBOT").is_some() && ob_handle.is_some() {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        ui.set_conn_label("已连接 · smoke".into());
        push_incoming(convo::ConvKey::DEMO, "smoke", "onebot 桥接自检消息", secs);
    }

    // Smoke seam (AGENTS 测试分层: 启动 + grim + 帧差): GUGU_SMOKE opens the
    // picker and sends one animated sticker through the real handlers, so the
    // M3 flow is verifiable without synthetic pointer input. No-op otherwise.
    if std::env::var_os("GUGU_SMOKE").is_some() {
        ui.set_show_stickers(true);
        ui.set_current_pack(1);
        ui.invoke_select_pack(1);
        let weak = ui.as_weak();
        Timer::single_shot(Duration::from_millis(600), move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_pick_sticker(1, 0);
            }
        });
    }

    // Smoke seam: GUGU_SMOKE_SEND exercises the real send addressing — pick
    // the first non-header roster row, set a fixed draft, invoke_send(), so
    // on_send's peer_id/is_group routing runs without synthetic pointer
    // input; the log line is diffed against the mock's stderr. The roster
    // lands asynchronously (WS handshake + get_friend_list/get_group_list
    // replies rebuild the sidebar), hence the 3 s delay. No-op otherwise.
    if std::env::var_os("GUGU_SMOKE_SEND").is_some() && ob_handle.is_some() {
        let weak = ui.as_weak();
        Timer::single_shot(Duration::from_millis(3000), move || {
            let Some(ui) = weak.upgrade() else { return };
            let channels = ui.get_channels();
            let Some((idx, ch)) = (0..channels.row_count())
                .filter_map(|i| Some((i, channels.row_data(i)?)))
                .find(|(_, c)| !c.header)
            else {
                eprintln!("gugu: smoke send skipped — roster not in sidebar");
                return;
            };
            ui.set_selected(idx as i32);
            ui.set_draft("smoke send probe".into());
            ui.invoke_send();
            eprintln!(
                "gugu: smoke send -> row={} peer_id={} is_group={}",
                ch.name, ch.peer_id, ch.is_group
            );
        });
    }

    ui.run()
}
