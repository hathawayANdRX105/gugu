// 咕咕 gugu — lightweight QQ chat client (Slint software renderer)
//
// Animated stickers: Slint has no native GIF animation (upstream #2081).
// We decode all GIF frames once with `image`, then a slint::Timer swaps
// the current frame into the model row. ponytail: one timer per animated
// message row; if that ever measures hot, collapse to one shared ticker
// driving all (row, frame-index) pairs.

use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Drive one message row's sticker animation with per-frame delays, exactly
/// the M0 frame-player pattern. Returns a running `Timer` the caller must
/// keep alive (dropping it stops the animation).
fn spawn_sticker_timer(
    model: Rc<VecModel<Message>>,
    row: usize,
    frames: Rc<StickerFrames>,
) -> Rc<Timer> {
    let timer = Rc::new(Timer::default());
    let state = Rc::new(RefCell::new(0usize)); // current frame index
    let (frames_c, state_c, model_c, timer_c) =
        (frames.clone(), state.clone(), model.clone(), timer.clone());
    timer.start(
        TimerMode::Repeated,
        Duration::from_millis(frames.delays[0]),
        move || {
            let gif = frames_c.clone();
            let i = *state_c.borrow();

            let Some(mut m) = model_c.row_data(row) else {
                return;
            };
            m.sticker = gif.frames[i].clone();
            model_c.set_row_data(row, m);
            let next = (i + 1) % gif.frames.len();
            *state_c.borrow_mut() = next;
            timer_c.set_interval(Duration::from_millis(gif.delays[next]));
        },
    );
    timer
}

// Model + grouping state stashed for the onebot bridge. `Rc` is `!Send`,
// so the transport thread's closure can't carry it; the
// `invoke_from_event_loop` callback runs back on this (UI) thread and
// picks the handles up here.
type BridgeState = (Rc<VecModel<Message>>, Rc<RefCell<Option<(String, i64)>>>);

thread_local! {
    static BRIDGE: RefCell<Option<BridgeState>> = const { RefCell::new(None) };
}

/// Append one incoming OneBot message to the chat model. Same 5-minute
/// same-author grouping rule as local sends; shares the `last` state via
/// [`BRIDGE`] so incoming and outgoing rows merge consistently.
fn push_incoming(author: &str, body: &str, unix: i64) {
    // ponytail: UTC minute-of-day like now_minutes(); TZ handling is M4's.
    let min = unix / 60 % 1440;
    BRIDGE.with(|b| {
        let borrowed = b.borrow();
        let Some((model, last)) = borrowed.as_ref() else {
            return;
        };
        let grouped = last
            .borrow()
            .as_ref()
            .is_some_and(|(a, m)| a == author && (0..=5).contains(&(min - *m)));
        model.push(Message {
            author: author.into(),
            initial: initial(author),
            body: body.into(),
            time: fmt_time(min),
            color: color_of(author),
            sticker: slint::Image::default(),
            grouped,
        });
        *last.borrow_mut() = Some((author.to_string(), min));
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

    // Group rule (Discord/Revolt): same author AND gap <= 5 min merges;
    // anything else reopens the group. Computed here, `.slint` only reads `grouped`.
    let mut msgs: Vec<Message> = Vec::new();
    let mut sticker_row = 0usize;
    let mut last: Option<(String, i64)> = None;
    for (author, body, min, sticker) in mock {
        let grouped = last
            .as_ref()
            .is_some_and(|(a, m)| a == *author && *min - m <= 5);
        if *sticker {
            sticker_row = msgs.len();
        }
        msgs.push(Message {
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
            grouped,
        });
        last = Some(((*author).to_string(), *min));
    }
    let model = Rc::new(VecModel::from(msgs));
    ui.set_messages(ModelRc::from(model.clone()));

    // ---- send ----
    let ui_weak = ui.as_weak();
    let model_send = model.clone();
    let last_send = Rc::new(RefCell::new(last));
    // Text-send, sticker-pick and onebot events share one grouping state.
    let last_sticker = last_send.clone();
    let last_bridge = last_send.clone();

    // ---- onebot (M2): data/config.toml present → live events; absent → demo ----
    // Spawned before `on_send` so sends can address the selected peer. The
    // transport thread survives disconnects (reconnect loop), so `Some` here
    // means "configured"; actions queued while down drain on the next session.
    let ob_handle = Rc::new(match onebot::Config::load(Path::new("data/config.toml")) {
        Ok(config) => {
            BRIDGE.with(|b| *b.borrow_mut() = Some((model.clone(), last_bridge)));
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
                                sender_name,
                                text,
                                time,
                                ..
                            }
                            | onebot::Event::GroupMessage {
                                sender_name,
                                text,
                                time,
                                ..
                            } => {
                                // ponytail: still one shared transcript; per-conversation
                                // routing by sender/group id is T6's.
                                push_incoming(&sender_name, &text, time);
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
                                    });
                                    rows.extend(seg.into_iter().map(|p| Channel {
                                        name: p.name.as_str().into(),
                                        header: false,
                                        peer_id: p.id.to_string().into(),
                                        is_group: p.is_group,
                                    }));
                                }
                                let first = rows.iter().position(|r| !r.header).unwrap_or(0) as i32;
                                ui.set_channels(Rc::new(VecModel::from(rows)).into());
                                ui.set_selected(first);
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
        // T5 addressing: the selected sidebar row carries the peer id. Header
        // rows (peer_id ""), unparseable ids and demo mode echo locally only.
        let target = ui
            .get_channels()
            .row_data(ui.get_selected().max(0) as usize);
        if let (Some(ob), Some(ch)) = (ob_send.as_ref(), target) {
            if let Ok(peer_id) = ch.peer_id.parse::<i64>() {
                ob.send(if ch.is_group {
                    onebot::Action::SendGroup {
                        group_id: peer_id,
                        text: draft.clone(),
                    }
                } else {
                    onebot::Action::SendPrivate {
                        user_id: peer_id,
                        text: draft.clone(),
                    }
                });
            }
        }
        let min = now_minutes();
        let grouped = last_send
            .borrow()
            .as_ref()
            .is_some_and(|(a, m)| a == "you" && min - *m <= 5);
        model_send.push(Message {
            author: "you".into(),
            initial: initial("you"),
            body: draft.into(),
            time: fmt_time(min),
            color: color_of("you"),
            sticker: slint::Image::default(),
            grouped,
        });
        *last_send.borrow_mut() = Some(("you".to_string(), min));
        ui.set_draft("".into());
    });

    // ---- channel select ----
    let ui_weak2 = ui.as_weak();
    ui.on_select_channel(move |idx| {
        if let Some(ui) = ui_weak2.upgrade() {
            ui.set_selected(idx);
        }
    });

    // ---- sticker animation timers ----
    // One timer per animated row; kept alive for the window's lifetime.
    let timers = Rc::new(RefCell::new(Vec::<Rc<Timer>>::new()));
    timers
        .borrow_mut()
        .push(spawn_sticker_timer(model.clone(), sticker_row, gif.clone()));

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
    let packs_pick = packs.clone();
    let timers_pick = timers.clone();
    let last_pick = last_sticker;
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
        let grouped = last_pick
            .borrow()
            .as_ref()
            .is_some_and(|(a, m)| a == "you" && min - *m <= 5);
        model_pick.push(Message {
            author: "you".into(),
            initial: initial("you"),
            body: "".into(),
            time: fmt_time(min),
            color: color_of("you"),
            sticker: frames.frames.first().cloned().unwrap_or_default(),
            grouped,
        });
        *last_pick.borrow_mut() = Some(("you".to_string(), min));
        let row = model_pick.row_count() - 1;
        if frames.frames.len() > 1 {
            let timer = spawn_sticker_timer(model_pick.clone(), row, frames);
            timers_pick.borrow_mut().push(timer);
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
        push_incoming("smoke", "onebot 桥接自检消息", secs);
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
