// SPDX-License-Identifier: GPL-3.0-or-later
//! Video files (roadmap 3.9): reading a video's picture, frame by frame,
//! through the operating system's own media framework, so the app ships
//! no codec code and plays what the computer already can (see
//! `docs/research/VIDEO_SCORING.md`). The video's sound is not read here:
//! it becomes an ordinary audio clip (`daw_engine::import::soundtrack`).
//!
//! - Windows: the media framework's source reader, which also converts
//!   and scales the picture.
//! - Linux: the system's media framework, with whichever decoders the
//!   distribution installed.
//! - macOS: not yet; opening a video says so.
//!
//! On top of the reader: [`Filmstrip`], the frames a clip shows along
//! the timeline at a zoom, and [`thumbs`], those frames decoded small in
//! the background; [`player`], the frame at the playhead, kept up with
//! playback on a thread of its own.

use std::fmt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod system;
#[cfg(windows)]
#[path = "windows.rs"]
mod system;
#[cfg(not(any(target_os = "linux", windows)))]
#[path = "none.rs"]
mod system;

#[cfg(test)]
mod boxes;
mod cache;
mod filmstrip;
pub mod player;
pub mod thumbs;

pub use filmstrip::{Filmstrip, Tile};

/// File name endings taken as video when a file is dropped in.
pub const EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "ogv", "wmv"];

/// Whether `path` is named like a video file.
pub fn is_video(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// What a video file holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Info {
    /// The picture's size, in pixels, as shown (square pixels).
    pub width: u32,
    pub height: u32,
    /// Frames per second, as a fraction: 24000/1001 for 23.976.
    pub rate: (u32, u32),
    /// How long the picture runs, in seconds.
    pub seconds: f64,
}

impl Info {
    /// Frames per second.
    pub fn fps(&self) -> f64 {
        self.rate.0 as f64 / self.rate.1.max(1) as f64
    }

    /// How many frames there are.
    pub fn frames(&self) -> u64 {
        (self.seconds * self.fps()).round().max(1.0) as u64
    }

    /// When frame `n` (from 0) starts, in seconds.
    pub fn frame_time(&self, n: u64) -> f64 {
        n as f64 * self.rate.1 as f64 / self.rate.0.max(1) as f64
    }

    /// The frame showing at `seconds`: the one whose span holds it.
    pub fn frame_at(&self, seconds: f64) -> u64 {
        // A hair over, so a time computed from a frame's start lands
        // in that frame and not the one before.
        let n = (seconds * self.fps() + 1e-6).floor().max(0.0) as u64;
        n.min(self.frames() - 1)
    }

    /// The size that fits within `max`, keeping the shape, in even
    /// pixels (some converters need them).
    pub fn fit(&self, max: (u32, u32)) -> (u32, u32) {
        let (w, h) = (self.width.max(1) as f64, self.height.max(1) as f64);
        let k = (max.0 as f64 / w).min(max.1 as f64 / h).min(1.0);
        let even = |v: f64| ((v.round() as u32) & !1).max(2);
        (even(w * k), even(h * k))
    }
}

/// A frame rate as the file states it, tidied to the usual fraction it
/// is meant to be when it is within a hair of one: a file may say
/// 2500000/208333 for 12, or 2997/100 for 29.97 (30000/1001).
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(crate) fn tidy_rate((n, d): (u32, u32)) -> (u32, u32) {
    let fps = n as f64 / d.max(1) as f64;
    let whole = fps.round();
    if whole >= 1.0 && (fps - whole).abs() < 1e-4 * whole {
        return (whole as u32, 1);
    }
    let ntsc = (fps * 1.001).round();
    if ntsc >= 1.0 && (fps - ntsc / 1.001).abs() < 1e-3 {
        return (ntsc as u32 * 1000, 1001);
    }
    (n, d.max(1))
}

/// When each frame is decoded, from when each is shown (`shown`, in the
/// order they are stored, which is the order they are decoded in): a
/// frame stored before the ones it is shown after must be decoded early
/// enough. Decoding times rise with storage order, each as late as the
/// showing times allow, and all are held back by the same delay: the
/// most any frame has to wait, which is returned too. Frames in showing
/// order give a delay of 0.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn decode_times(shown: &[i64]) -> (Vec<i64>, i64) {
    let mut in_order = shown.to_vec();
    in_order.sort_unstable();
    let delay = in_order
        .iter()
        .zip(shown)
        .map(|(sorted, shown)| sorted - shown)
        .max()
        .unwrap_or(0)
        .max(0);
    (in_order.iter().map(|t| t - delay).collect(), delay)
}

/// One decoded picture: rows of red, green, blue and alpha bytes, top
/// row first.
#[derive(Clone, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// When it starts in the video, in seconds.
    pub time: f64,
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Frame({}x{} at {:.3}s)",
            self.width, self.height, self.time
        )
    }
}

impl Frame {
    /// Shrinks the picture to `w` by `h` by averaging, for a system
    /// reader that couldn't scale it itself.
    pub fn scaled(self, w: u32, h: u32) -> Frame {
        if (w, h) == (self.width, self.height) || w == 0 || h == 0 {
            return self;
        }
        let (sw, sh) = (self.width as usize, self.height as usize);
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        for y in 0..h as usize {
            let (y0, y1) = (
                y * sh / h as usize,
                ((y + 1) * sh / h as usize).max(y * sh / h as usize + 1),
            );
            for x in 0..w as usize {
                let (x0, x1) = (
                    x * sw / w as usize,
                    ((x + 1) * sw / w as usize).max(x * sw / w as usize + 1),
                );
                let mut sum = [0u32; 4];
                for sy in y0..y1.min(sh) {
                    for sx in x0..x1.min(sw) {
                        let p = (sy * sw + sx) * 4;
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += self.rgba[p + c] as u32;
                        }
                    }
                }
                let n = ((y1.min(sh) - y0) * (x1.min(sw) - x0)).max(1) as u32;
                let d = (y * w as usize + x) * 4;
                for c in 0..4 {
                    rgba[d + c] = (sum[c] / n) as u8;
                }
            }
        }
        Frame {
            width: w,
            height: h,
            rgba,
            time: self.time,
        }
    }
}

/// Why a video couldn't be read.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// This computer has no way to read video yet.
    NoFramework(String),
    /// The file couldn't be opened or decoded: a codec the system lacks,
    /// or a damaged file.
    Unreadable(String),
    /// It holds no picture.
    NoPicture,
    /// A copy with new sound couldn't be written.
    CantWrap(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::NoFramework(why) => write!(f, "this computer can't play video yet: {why}"),
            Error::Unreadable(why) => write!(
                f,
                "this computer can't play this video ({why}). Converting it to MP4 \
                 (H.264) with a video tool usually helps"
            ),
            Error::NoPicture => write!(f, "this file has no picture"),
            Error::CantWrap(why) => write!(f, "the copy of the video couldn't be made: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Reads one video file's picture, scaled to a fixed size. Not shared
/// between threads: each worker opens its own.
pub struct Reader {
    inner: system::Reader,
    info: Info,
    size: (u32, u32),
}

impl Reader {
    /// Opens `path`, giving frames that fit within `max` pixels.
    pub fn open(path: &Path, max: (u32, u32)) -> Result<Reader, Error> {
        let info = system::probe(path)?;
        let size = info.fit(max);
        let inner = system::Reader::open(path, &info, size)?;
        Ok(Reader { inner, info, size })
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// The size frames come out at.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Goes to `seconds`: the next frame read is the one showing then.
    pub fn seek(&mut self, seconds: f64) -> Result<(), Error> {
        self.inner.seek(seconds.max(0.0))
    }

    /// The next frame, or `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, Error> {
        let Some(f) = self.inner.next_frame()? else {
            return Ok(None);
        };
        Ok(Some(f.scaled(self.size.0, self.size.1)))
    }

    /// The frame showing at `seconds`, seeking to it.
    pub fn frame_at(&mut self, seconds: f64) -> Result<Option<Frame>, Error> {
        self.seek(seconds)?;
        self.next_frame()
    }
}

/// What `path` holds, without decoding any of it.
pub fn probe(path: &Path) -> Result<Info, Error> {
    system::probe(path)
}

/// Whether this system can make a copy of a video with new sound
/// ([`wrap`]). Not yet on Windows, whose copies come out a few frames
/// off, nor on macOS.
pub const CAN_WRAP: bool = cfg!(target_os = "linux");

/// The file name ending a copy of `video` with new sound gets: the
/// same kind of file where the system can write one, so the picture
/// fits it as it is.
pub fn wrapped_extension(video: &Path) -> &'static str {
    let ext = (video.extension())
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if cfg!(windows) {
        // The system's writer makes MP4 files.
        return "mp4";
    }
    match ext.as_str() {
        "mp4" | "m4v" => "mp4",
        "mov" => "mov",
        "webm" => "webm",
        "ogv" | "ogg" => "ogv",
        "avi" => "avi",
        // Matroska holds nearly any picture.
        _ => "mkv",
    }
}

/// Writes a copy of `video` to `out` with `sound` (a WAV file, which
/// starts with the film's first frame) in place of its own sound. The
/// picture is copied as it is, not re-encoded, so it loses nothing and
/// takes moments; the sound is encoded by the system for the kind of
/// file (see [`wrapped_extension`]). `out` appears only once complete.
/// Setting `cancel` stops it.
pub fn wrap(video: &Path, sound: &Path, out: &Path, cancel: &AtomicBool) -> Result<(), Error> {
    let ext = out.extension().map(|e| e.to_string_lossy().into_owned());
    let part = out.with_extension(format!("part.{}", ext.unwrap_or_default()));
    let r = system::wrap(video, sound, &part, cancel)
        .and_then(|()| std::fs::rename(&part, out).map_err(|e| Error::CantWrap(e.to_string())));
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_counted_from_their_rate() {
        let i = Info {
            width: 1920,
            height: 1080,
            rate: (24000, 1001),
            seconds: 10.0,
        };
        assert!((i.fps() - 23.976).abs() < 1e-3);
        assert_eq!(i.frames(), 240);
        // A time exactly on a frame's start shows that frame.
        assert_eq!(i.frame_at(i.frame_time(100)), 100);
        assert_eq!(i.frame_at(i.frame_time(100) - 1e-4), 99);
        assert_eq!(i.frame_at(1e9), 239, "held to the last");
        assert_eq!(i.fit((320, 320)), (320, 180));
        assert_eq!(i.fit((4000, 4000)), (1920, 1080), "never made bigger");
    }

    #[test]
    fn rates_near_a_usual_one_are_tidied_to_it() {
        assert_eq!(tidy_rate((2_500_000, 208_333)), (12, 1));
        assert_eq!(tidy_rate((2997, 100)), (30000, 1001));
        assert_eq!(tidy_rate((24000, 1001)), (24000, 1001));
        assert_eq!(tidy_rate((25, 1)), (25, 1));
        assert_eq!(tidy_rate((15, 2)), (15, 2), "7.5 stays");
    }

    #[test]
    fn decoding_times_come_before_showing_times() {
        // Shown in order: decoded as shown, no delay.
        assert_eq!(decode_times(&[0, 10, 20, 30]), (vec![0, 10, 20, 30], 0));
        // Stored I P B B, with the P shown fourth: each is decoded no
        // later than it is shown, and a frame's worth earlier is enough.
        let (dts, delay) = decode_times(&[0, 30, 10, 20, 60, 40, 50]);
        assert_eq!(delay, 10);
        assert_eq!(dts, vec![-10, 0, 10, 20, 30, 40, 50]);
        for (d, s) in dts.iter().zip([0, 30, 10, 20, 60, 40, 50]) {
            assert!(d <= &s);
        }
        assert_eq!(decode_times(&[]), (vec![], 0));
    }

    #[test]
    fn shrinking_averages_the_pixels() {
        let f = Frame {
            width: 2,
            height: 2,
            rgba: vec![0, 0, 0, 255, 200, 0, 0, 255, 0, 100, 0, 255, 0, 0, 40, 255],
            time: 0.0,
        };
        let s = f.scaled(1, 1);
        assert_eq!(s.rgba, [50, 25, 10, 255]);
    }
}
