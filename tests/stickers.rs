//! Behaviour tests for `src/stickers.rs`, seen as a consumer would.
//!
//! gugu has no lib target yet and `stickers` is not registered in `main.rs`,
//! so the module is compiled straight into this test crate via `#[path]`
//! instead of `use gugu::...`. Fixtures are synthesized into a unique temp
//! directory; nothing is written into the repository and only that directory
//! is ever deleted.

#[path = "../src/stickers.rs"]
mod stickers;

use image::{Delay, Frame};
use image::codecs::gif::GifEncoder;
use image::RgbaImage;
use stickers::{load_gif, scan_packs, StickerError};
use std::fs;
use std::path::{Path, PathBuf};

/// Fresh scratch directory under the system temp dir; the only place tests
/// are allowed to create (and later delete) files.
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "gugu-stickers-{tag}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Encode a 4x4 solid-colour GIF with the given per-frame delays (ms).
fn write_gif(path: &Path, delays_ms: &[u32]) {
    let frames = delays_ms.iter().enumerate().map(|(i, &d)| {
        let img = RgbaImage::from_pixel(4, 4, image::Rgba([i as u8 * 60, 90, 30, 255]));
        Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(d, 1))
    });
    let file = fs::File::create(path).unwrap();
    GifEncoder::new(file).encode_frames(frames).unwrap();
}

/// Write a tiny 3x3 static PNG.
fn write_png(path: &Path) {
    RgbaImage::from_pixel(3, 3, image::Rgba([7, 7, 7, 255]))
        .save(path)
        .unwrap();
}

#[test]
fn multi_frame_gif_yields_all_frames_with_clamped_delays() {
    let dir = temp_dir("anim");
    let gif = dir.join("anim.gif");
    // 0 ms clamps up to 20; 30 ms passes through; 600 s clamps down to 5000.
    write_gif(&gif, &[0, 30, 600_000]);

    let sticker = load_gif(&gif).expect("synthesized gif must decode");
    assert_eq!(sticker.frames.len(), 3);
    assert_eq!(sticker.delays, vec![20, 30, 5000]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn garbage_or_missing_gif_yields_typed_error_not_panic() {
    let dir = temp_dir("garbage");
    let bad = dir.join("broken.gif");
    fs::write(&bad, b"GIF89a\xff\x00not really a gif at all").unwrap();
    let err = load_gif(&bad).expect_err("garbage must not decode");
    assert!(
        matches!(err, StickerError::Decode { .. } | StickerError::Empty { .. }),
        "unexpected variant: {err:?}"
    );
    assert!(err.to_string().contains("broken.gif"), "error should name the file");

    let missing = dir.join("nope.gif");
    let err = load_gif(&missing).expect_err("missing file must error");
    assert!(matches!(err, StickerError::Io { .. }), "unexpected: {err:?}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_packs_skips_non_directories_and_broken_entries() {
    let dir = temp_dir("scan");
    fs::create_dir_all(dir.join("alpha")).unwrap();
    fs::create_dir_all(dir.join("beta")).unwrap();
    write_gif(&dir.join("alpha").join("a1.gif"), &[40, 40]);
    fs::write(dir.join("alpha").join("broken.gif"), b"junk").unwrap();
    write_png(&dir.join("beta").join("b1.png"));
    // Non-directory entries at the root must not become packs.
    fs::write(dir.join("root-note.txt"), b"x").unwrap();
    fs::write(dir.join("loose.gif"), b"y").unwrap();

    let packs = scan_packs(&dir);
    let names: Vec<&str> = packs.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"]);

    assert_eq!(packs[0].stickers.len(), 1, "broken gif must be skipped");
    assert_eq!(packs[0].stickers[0].0, "a1.gif");
    assert_eq!(packs[0].stickers[0].1.frames.len(), 2);

    // A static PNG is a valid one-frame sticker of the right size.
    assert_eq!(packs[1].stickers[0].0, "b1.png");
    let size = packs[1].stickers[0].1.frames[0].size();
    assert_eq!((size.width, size.height), (3, 3));

    // Unreadable root: empty scan, no panic.
    assert!(scan_packs(&dir.join("does-not-exist")).is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_packs_ordering_is_deterministic() {
    let dir = temp_dir("order");
    // Create packs and files in scrambled order so filesystem order differs
    // from the expected sorted output.
    fs::create_dir_all(dir.join("beta")).unwrap();
    fs::create_dir_all(dir.join("alpha")).unwrap();
    write_gif(&dir.join("beta").join("z.gif"), &[50]);
    write_gif(&dir.join("beta").join("a.gif"), &[50]);
    write_gif(&dir.join("alpha").join("m.gif"), &[50]);
    write_gif(&dir.join("alpha").join("b.gif"), &[50]);

    let shape = |packs: &Vec<stickers::StickerPack>| {
        packs
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    p.stickers.iter().map(|s| s.0.clone()).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };

    let once = scan_packs(&dir);
    let twice = scan_packs(&dir);
    assert_eq!(shape(&once), shape(&twice));
    assert_eq!(
        shape(&once),
        vec![
            ("alpha".to_string(), vec!["b.gif".to_string(), "m.gif".to_string()]),
            ("beta".to_string(), vec!["a.gif".to_string(), "z.gif".to_string()]),
        ]
    );

    let _ = fs::remove_dir_all(&dir);
}
