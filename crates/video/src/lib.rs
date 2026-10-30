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

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod system;
#[cfg(windows)]
#[path = "windows.rs"]
mod system;
#[cfg(not(any(target_os = "linux", windows)))]
#[path = "none.rs"]
mod system;

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
