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
    let mut last: Option<(&str, i64)> = None;
    for (author, body, min, sticker) in mock {
        let grouped = last.is_some_and(|(a, m)| a == *author && *min - m <= 5);
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
        last = Some((author, *min));
    }
    let model = Rc::new(VecModel::from(msgs));
    ui.set_messages(ModelRc::from(model.clone()));

    // ---- send ----
    let ui_weak = ui.as_weak();
    let model_send = model.clone();
    let last_send = Rc::new(RefCell::new(last));
    // Text-send and sticker-pick share one grouping state.
    let last_sticker = last_send.clone();
    ui.on_send(move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        let draft = ui.get_draft().to_string();
        if draft.trim().is_empty() {
            return;
        }
        let min = now_minutes();
        let grouped = last_send
            .borrow()
            .is_some_and(|(a, m)| a == "you" && min - m <= 5);
        model_send.push(Message {
            author: "you".into(),
            initial: initial("you"),
            body: draft.into(),
            time: fmt_time(min),
            color: color_of("you"),
            sticker: slint::Image::default(),
            grouped,
        });
        *last_send.borrow_mut() = Some(("you", min));
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
            .is_some_and(|(a, m)| a == "you" && min - m <= 5);
        model_pick.push(Message {
            author: "you".into(),
            initial: initial("you"),
            body: "".into(),
            time: fmt_time(min),
            color: color_of("you"),
            sticker: frames.frames.first().cloned().unwrap_or_default(),
            grouped,
        });
        *last_pick.borrow_mut() = Some(("you", min));
        let row = model_pick.row_count() - 1;
        if frames.frames.len() > 1 {
            let timer = spawn_sticker_timer(model_pick.clone(), row, frames);
            timers_pick.borrow_mut().push(timer);
        }
    });

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

    ui.run()
}
