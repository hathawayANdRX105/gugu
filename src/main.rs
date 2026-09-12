// 咕咕 gugu — lightweight QQ chat client (Slint software renderer)
//
// Animated stickers: Slint has no native GIF animation (upstream #2081).
// We decode all GIF frames once with `image`, then a slint::Timer swaps
// the current frame into the model row. ponytail: one timer for the demo;
// batch per-sticker timers when real chat has many gifs on screen.

use slint::{
    ComponentHandle, Image as SlintImage, Model, ModelRc, SharedPixelBuffer, Timer, TimerMode,
    VecModel,
};
use image::AnimationDecoder; // brings into_frames() into scope
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

slint::include_modules!();

/// All decoded frames of one animated GIF + per-frame delays (ms).
struct GifFrames {
    frames: Vec<SlintImage>,
    delays: Vec<u64>,
}

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

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;

    // ---- mock data (ricq protocol layer replaces this later) ----
    let channels: Vec<Channel> = [
        "#general", "#random", "#memes", "#stuffs", "#dev", "#art", "#music", "#gaming",
        "#announce", "#offtopic",
    ]
    .iter()
    .map(|n| Channel { name: (*n).into() })
    .collect();
    ui.set_channels(Rc::new(VecModel::from(channels)).into());

    let names = ["alice", "bob", "carol", "dave", "erin", "frank", "grace", "heidi", "ivan", "judy"];
    let bodies = [
        "hello everyone", "how's the build going?", "did you see that PR?", "lgtm", "ship it",
        "anyone tried the new feature?", "works on my machine", "lol", "+1", "interesting",
    ];
    let mut msgs: Vec<Message> = (0..40)
        .map(|i| Message {
            author: names[i % 10].into(),
            initial: names[i % 10][..1].to_uppercase().into(),
            body: bodies[i % 10].into(),
            time: format!("09:{i:02}").into(),
            color: (i % 10) as i32,
            sticker: SlintImage::default(),
        })
        .collect();
    msgs.insert(0, Message {
        author: "bob".into(),
        initial: "B".into(),
        body: "".into(),
        time: "now".into(),
        color: 1,
        sticker: SlintImage::default(),
    });
    let sticker_row = 0usize;
    let model = Rc::new(VecModel::from(msgs));
    ui.set_messages(ModelRc::from(model.clone()));

    // ---- send ----
    let ui_weak = ui.as_weak();
    let model_send = model.clone();
    ui.on_send(move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        let draft = ui.get_draft().to_string();
        if draft.trim().is_empty() {
            return;
        }
        model_send.push(Message {
            author: "you".into(),
            initial: "Y".into(),
            body: draft.into(),
            time: "now".into(),
            color: 4,
            sticker: SlintImage::default(),
        });
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
    let gif = Rc::new(load_gif("assets/sticker.gif"));
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
