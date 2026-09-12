// 咕咕 gugu — lightweight QQ chat client (Slint software renderer)
//
// Animated stickers: Slint has no native GIF animation (upstream #2081).
// We decode all GIF frames once with `image`, then a slint::Timer swaps
// the current frame into the model row. ponytail: one timer for the demo;
// batch per-sticker timers when real chat has many gifs on screen.

use image::AnimationDecoder; // brings into_frames() into scope
use slint::{
    ComponentHandle, Image as SlintImage, Model, ModelRc, SharedPixelBuffer, SharedString, Timer,
    TimerMode, VecModel,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

slint::include_modules!();

/// All decoded frames of one animated GIF + per-frame delays (ms).
struct GifFrames {
    frames: Vec<SlintImage>,
    delays: Vec<u64>,
}

/// Decode every frame of a GIF file into Slint images (RGBA, straight alpha).
fn load_gif(path: &str) -> GifFrames {
    let data = std::fs::read(path).expect(path);
    let reader = std::io::Cursor::new(&data);
    let decoder = image::codecs::gif::GifDecoder::new(reader).expect("gif decode");
    let mut frames = Vec::new();
    let mut delays = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.expect("gif frame");
        let ms = Duration::from(frame.delay()).as_millis().clamp(20, 5000) as u64;
        let buf = frame.into_buffer(); // RgbaImage (straight alpha)
        let shared =
            SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(buf.as_raw(), buf.width(), buf.height());
        frames.push(SlintImage::from_rgba8(shared));
        delays.push(ms);
    }
    assert!(!frames.is_empty(), "gif has no frames");
    GifFrames { frames, delays }
}

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
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / 60 % 1440) as i64
}

/// Format minutes since midnight as "HH:MM".
fn fmt_time(min: i64) -> SharedString {
    format!("{:02}:{:02}", (min / 60) % 24, min % 60).into()
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
        ("alice", "同一个人隔了 20 分钟，重开一组", 20 * 60 + 20, false),
        ("bob", "看这个贴纸", 20 * 60 + 24, true),
        ("dave", "哈哈哈有点可爱", 20 * 60 + 25, false),
        ("erin", "下午三点会议室过一版", 20 * 60 + 29, false),
        ("erin", "记得带周报", 20 * 60 + 30, false),
        ("grace", "收到", 20 * 60 + 31, false),
    ];

    let gif = Rc::new(load_gif("assets/sticker.gif"));

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
                SlintImage::default()
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
    ui.on_send(move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        let draft = ui.get_draft().to_string();
        if draft.trim().is_empty() {
            return;
        }
        let min = now_minutes();
        let grouped = last_send.borrow().is_some_and(|(a, m)| a == "you" && min - m <= 5);
        model_send.push(Message {
            author: "you".into(),
            initial: initial("you"),
            body: draft.into(),
            time: fmt_time(min),
            color: color_of("you"),
            sticker: SlintImage::default(),
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

    // ---- animated sticker loop ----
    let state = Rc::new(RefCell::new(0usize)); // current frame index
    let timer = Rc::new(Timer::default());
    let gif_c = gif.clone();
    let state_c = state.clone();
    let model_c = model.clone();
    let timer_c = timer.clone();
    timer.start(TimerMode::Repeated, Duration::from_millis(gif.delays[0]), move || {
        let gif = gif_c.clone();
        let i = *state_c.borrow();
        let mut m = model_c.row_data(sticker_row).unwrap();
        m.sticker = gif.frames[i].clone();
        model_c.set_row_data(sticker_row, m);
        let next = (i + 1) % gif.frames.len();
        *state_c.borrow_mut() = next;
        timer_c.set_interval(Duration::from_millis(gif.delays[next]));
    });

    ui.run()
}
