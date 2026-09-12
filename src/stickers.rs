//! Local sticker-pack layer (ROADMAP M3).
//!
//! Slint has no native GIF animation (upstream #2081), so the M0-validated
//! frame-player pattern stays: decode frames with `image`, then the UI swaps
//! them with a `slint::Timer`. This module is the pure-decode half: it only
//! reads and decodes files on whatever thread the caller is on and returns
//! owned `slint::Image` data — it never touches any Slint component state,
//! which keeps the "no Slint off the UI thread" rule intact for the caller.
//!
//! On-disk layout for [`scan_packs`]: every sub-directory of the root is a
//! pack, every `*.gif` / `*.png` file inside it is one sticker, e.g.
//! `assets/stickers/pepe/wave.gif`. A PNG (or any single-frame file) is a
//! perfectly valid static sticker.
//!
//! Not yet registered in `main.rs`; the maintainer adds `mod stickers;`
//! after landing this module.

use std::fmt;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::AnimationDecoder as _; // brings into_frames() into scope for GifDecoder

/// Lower clamp for per-frame delay (ms): a shorter timer tick would just spin
/// the event loop. GIFs on the wild (and QQ stickers specifically) love 0 ms.
const MIN_DELAY_MS: u64 = 20;

/// Upper clamp for per-frame delay (ms): beyond this the animation is treated
/// as broken/frozen rather than slow.
const MAX_DELAY_MS: u64 = 5000;

/// Nominal delay reported for a static single-frame sticker. Consumers must
/// not start an animation timer when `frames.len() == 1`, so this value is
/// never actually displayed; it exists so `delays.len() == frames.len()`
/// always holds.
const STATIC_DELAY_MS: u64 = 100;

/// Failure to load one sticker file.
#[derive(Debug)]
pub enum StickerError {
    /// The file could not be opened or read (missing, denied, I/O failure).
    Io {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The file was readable but is not a decodable image (bad header,
    /// truncated stream, unsupported format).
    Decode {
        /// File that could not be decoded.
        path: PathBuf,
        /// Underlying decoder error.
        source: image::ImageError,
    },
    /// The container parsed but yielded zero frames.
    Empty {
        /// File that contained no frames.
        path: PathBuf,
    },
}

impl fmt::Display for StickerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "cannot read sticker {}: {source}", path.display())
            }
            Self::Decode { path, source } => {
                write!(f, "cannot decode sticker {}: {source}", path.display())
            }
            Self::Empty { path } => {
                write!(f, "sticker {} contains no frames", path.display())
            }
        }
    }
}

impl std::error::Error for StickerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Empty { .. } => None,
        }
    }
}

/// All decoded frames of one sticker, ready for the UI's frame player.
///
/// `frames` and `delays` are the same length; `delays[i]` is how long
/// `frames[i]` should be displayed, already clamped to 20..=5000 ms.
/// A static sticker is one frame — not an error.
#[derive(Debug, Clone)]
pub struct StickerFrames {
    /// Frames in playback order (RGBA, straight alpha).
    pub frames: Vec<slint::Image>,
    /// Per-frame display duration in ms, clamped to 20..=5000.
    pub delays: Vec<u64>,
}

/// Decode every frame of an animated GIF into Slint images.
///
/// Compositing/disposal is handled by the `image` crate's frame iterator, so
/// each frame is a complete RGBA buffer. A single-frame GIF yields one frame
/// (a valid static sticker).
///
/// Errors: [`StickerError::Io`] when the file cannot be read,
/// [`StickerError::Decode`] on a broken or truncated GIF,
/// [`StickerError::Empty`] when it parses but contains zero frames.
pub fn load_gif(path: &Path) -> Result<StickerFrames, StickerError> {
    let as_io = |source: std::io::Error| StickerError::Io {
        path: path.to_path_buf(),
        source,
    };
    let as_decode = |source: image::ImageError| StickerError::Decode {
        path: path.to_path_buf(),
        source,
    };
    let file = fs::File::open(path).map_err(as_io)?;
    let decoder = image::codecs::gif::GifDecoder::new(BufReader::new(file)).map_err(as_decode)?;
    let mut out = StickerFrames {
        frames: Vec::new(),
        delays: Vec::new(),
    };
    for frame in decoder.into_frames() {
        let frame = frame.map_err(as_decode)?;
        let ms = Duration::from(frame.delay()).as_millis() as u64;
        out.frames.push(to_slint_image(frame.into_buffer()));
        out.delays.push(ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS));
    }
    if out.frames.is_empty() {
        return Err(StickerError::Empty {
            path: path.to_path_buf(),
        });
    }
    Ok(out)
}

/// Load any sticker file: `*.gif` goes through [`load_gif`] (animated),
/// everything else is decoded as a static single-frame image (PNG support
/// follows `Cargo.toml` features).
///
/// Errors as per [`load_gif`], plus [`StickerError::Decode`] for unknown or
/// unsupported formats.
pub fn load_sticker(path: &Path) -> Result<StickerFrames, StickerError> {
    if has_extension(path, "gif") {
        load_gif(path)
    } else {
        load_still(path)
    }
}

/// A named directory of stickers, i.e. one sub-directory of the scan root.
#[derive(Debug, Clone)]
pub struct StickerPack {
    /// Pack name: the directory's file name, e.g. `"pepe"`.
    pub name: String,
    /// Stickers sorted by file name; each key is the file name, e.g.
    /// `"wave.gif"`.
    pub stickers: Vec<(String, StickerFrames)>,
}

/// Scan a sticker root directory: each sub-directory is one pack, each
/// `*.gif` / `*.png` file inside it one sticker.
///
/// The scan never fails: an unreadable root yields an empty `Vec`, and
/// non-directories under the root, non-image files, and individual broken
/// images are skipped. Output is deterministic — packs sorted by name,
/// stickers sorted by file name.
pub fn scan_packs(root: &Path) -> Vec<StickerPack> {
    let mut packs = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return packs;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue; // non-directory entries at the root are not packs
        }
        let mut stickers = Vec::new();
        if let Ok(files) = fs::read_dir(entry.path()) {
            for file in files.flatten() {
                let path = file.path();
                if !file.file_type().is_ok_and(|t| t.is_file()) || !is_sticker_file(&path) {
                    continue;
                }
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if let Ok(decoded) = load_sticker(&path) {
                    stickers.push((name, decoded));
                }
            }
        }
        stickers.sort_by(|a, b| a.0.cmp(&b.0));
        packs.push(StickerPack {
            name: entry.file_name().to_string_lossy().into_owned(),
            stickers,
        });
    }
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
}

/// Decode a static image (PNG etc.) as a one-frame sticker.
fn load_still(path: &Path) -> Result<StickerFrames, StickerError> {
    let image = image::open(path).map_err(|source| match source {
        image::ImageError::IoError(source) => StickerError::Io {
            path: path.to_path_buf(),
            source,
        },
        source => StickerError::Decode {
            path: path.to_path_buf(),
            source,
        },
    })?;
    let frame = to_slint_image(image.into_rgba8());
    Ok(StickerFrames {
        frames: vec![frame],
        delays: vec![STATIC_DELAY_MS],
    })
}

/// Wrap an RGBA buffer as a `slint::Image` (straight alpha).
fn to_slint_image(buf: image::RgbaImage) -> slint::Image {
    let shared = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        buf.as_raw(),
        buf.width(),
        buf.height(),
    );
    slint::Image::from_rgba8(shared)
}

/// Case-insensitive extension test.
fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Whether the file name looks like a sticker (`.gif` / `.png`).
fn is_sticker_file(path: &Path) -> bool {
    has_extension(path, "gif") || has_extension(path, "png")
}
