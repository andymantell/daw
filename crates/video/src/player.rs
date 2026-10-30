// SPDX-License-Identifier: GPL-3.0-or-later
//! The video window's picture: the frame at the playhead, decoded on a
//! thread of its own. While playing it decodes on from the last frame;
//! a jump (a seek, a loop) goes straight to the new place.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::{Error, Frame, Info, Reader};

/// The largest picture decoded, in pixels.
pub const MAX: (u32, u32) = (1280, 720);

/// Decoding on through frames is quicker than a seek for this long.
const AHEAD_SECONDS: f64 = 1.0;

#[derive(Clone, Debug, PartialEq)]
struct Want {
    path: Option<PathBuf>,
    /// Seconds into the video.
    at: f64,
}

#[derive(Default)]
struct Shown {
    frame: Option<Arc<Frame>>,
    info: Option<Info>,
    error: Option<(PathBuf, Error)>,
    /// Counts up with each new frame.
    count: u64,
}

struct Shared {
    want: Mutex<Want>,
    wake: Condvar,
    shown: Mutex<Shown>,
    stop: AtomicBool,
}

pub struct Player {
    shared: Arc<Shared>,
}

impl Default for Player {
    fn default() -> Player {
        Player::new()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
    }
}

impl Player {
    pub fn new() -> Player {
        let shared = Arc::new(Shared {
            want: Mutex::new(Want {
                path: None,
                at: 0.0,
            }),
            wake: Condvar::new(),
            shown: Mutex::new(Shown::default()),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        std::thread::Builder::new()
            .name("video player".into())
            .spawn(move || run(&s))
            .expect("a thread for the video player");
        Player { shared }
    }

    /// Shows `path` at `seconds` into it, or nothing for `None`.
    pub fn show(&self, path: Option<&Path>, seconds: f64) {
        let want = Want {
            path: path.map(Path::to_path_buf),
            at: seconds.max(0.0),
        };
        let mut w = self.shared.want.lock().unwrap();
        if *w != want {
            *w = want;
            self.shared.wake.notify_all();
        }
    }

    /// The latest frame, and a count that changes with each new one.
    pub fn frame(&self) -> (Option<Arc<Frame>>, u64) {
        let s = self.shared.shown.lock().unwrap();
        (s.frame.clone(), s.count)
    }

    /// What the video being shown holds.
    pub fn info(&self) -> Option<Info> {
        self.shared.shown.lock().unwrap().info
    }

    /// Why `path` couldn't be shown, if it couldn't.
    pub fn error(&self, path: &Path) -> Option<Error> {
        let s = self.shared.shown.lock().unwrap();
        s.error
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, e)| e.clone())
    }
}

fn run(s: &Shared) {
    let mut reader: Option<(PathBuf, Reader)> = None;
    // The last frame decoded, by number.
    let mut last: Option<u64> = None;
    let mut seen = Want {
        path: None,
        at: -1.0,
    };
    loop {
        let want = {
            let mut w = s.want.lock().unwrap();
            while *w == seen && !s.stop.load(Ordering::Relaxed) {
                w = s.wake.wait(w).unwrap();
            }
            w.clone()
        };
        if s.stop.load(Ordering::Relaxed) {
            return;
        }
        seen = want.clone();
        let Some(path) = want.path else {
            reader = None;
            last = None;
            let mut sh = s.shown.lock().unwrap();
            sh.frame = None;
            sh.info = None;
            sh.count += 1;
            continue;
        };
        if reader.as_ref().is_none_or(|(p, _)| *p != path) {
            last = None;
            reader = match Reader::open(&path, MAX) {
                Ok(r) => {
                    let mut sh = s.shown.lock().unwrap();
                    sh.info = Some(*r.info());
                    sh.error = None;
                    Some((path.clone(), r))
                }
                Err(e) => {
                    let mut sh = s.shown.lock().unwrap();
                    sh.error = Some((path.clone(), e));
                    sh.frame = None;
                    sh.count += 1;
                    None
                }
            };
        }
        let Some((_, r)) = &mut reader else { continue };
        let info = *r.info();
        let n = info.frame_at(want.at);
        if last == Some(n) {
            continue;
        }
        match step_to(r, &info, last, n) {
            Ok(Some((got, f))) => {
                last = Some(got);
                let mut sh = s.shown.lock().unwrap();
                sh.frame = Some(Arc::new(f));
                sh.count += 1;
            }
            Ok(None) => last = None,
            Err(e) => {
                let mut sh = s.shown.lock().unwrap();
                sh.error = Some((path.clone(), e));
                reader = None;
                last = None;
            }
        }
    }
}

/// Decodes frame `n`, on from `last` if it is a little before, or else
/// by seeking. Returns the frame's number and picture.
fn step_to(
    r: &mut Reader,
    info: &Info,
    last: Option<u64>,
    n: u64,
) -> Result<Option<(u64, Frame)>, Error> {
    let ahead = (AHEAD_SECONDS * info.fps()).ceil() as u64;
    if !last.is_some_and(|l| l < n && n - l <= ahead) {
        r.seek(info.frame_time(n))?;
    }
    while let Some(f) = r.next_frame()? {
        let got = info.frame_at(f.time + 0.25 / info.fps().max(1e-6));
        if got >= n {
            return Ok(Some((got, f)));
        }
    }
    Ok(None)
}
