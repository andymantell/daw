// SPDX-License-Identifier: GPL-3.0-or-later
//! Small pictures of a video's frames for the filmstrip, decoded on a
//! thread of their own as soon as the video comes in.
//!
//! The thread goes through the whole video once and keeps every frame,
//! or for a long video every few frames, so the strip has a picture
//! for any zoom straight away. Frames it skipped are decoded when the
//! strip asks for them (zoomed in close), kept for a while, and let go
//! of again when many more have been asked for.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::{Error, Frame, Info, Reader};

/// How tall the pictures are, in pixels.
pub const HEIGHT: u32 = 54;
/// How many frames the first pass keeps at most: about 30 MB of
/// pictures.
const KEEP: u64 = 2000;
/// How many frames asked for later are kept at most.
const EXTRA: usize = 600;

#[derive(Default)]
struct State {
    info: Option<Info>,
    /// Every `every`th frame, from the first pass.
    every: u64,
    kept: HashMap<u64, Arc<Frame>>,
    /// Frames asked for since, oldest first.
    extra: HashMap<u64, Arc<Frame>>,
    extra_order: VecDeque<u64>,
    /// Frames asked for and not decoded yet.
    asked: Vec<u64>,
    /// How far the first pass has got, in frames.
    done: u64,
    finished: bool,
    error: Option<Error>,
    /// A new picture came in since [`Thumbs::changed`] was last asked.
    fresh: bool,
    /// The first pass's pictures were read from the cache.
    from_cache: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    stop: AtomicBool,
}

/// The pictures of one video file.
pub struct Thumbs {
    path: PathBuf,
    shared: Arc<Shared>,
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
    }
}

impl Thumbs {
    /// Starts decoding `path`'s pictures.
    pub fn start(path: &Path) -> Thumbs {
        Thumbs::start_in(path, None)
    }

    /// Starts decoding `path`'s pictures, keeping them in `cache` for
    /// next time, or taking them from there if they were kept before.
    pub fn start_cached(path: &Path, cache: &Path) -> Thumbs {
        Thumbs::start_in(path, Some(cache.to_path_buf()))
    }

    fn start_in(path: &Path, cache: Option<PathBuf>) -> Thumbs {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let (p, s) = (path.to_path_buf(), shared.clone());
        std::thread::Builder::new()
            .name("video thumbnails".into())
            .spawn(move || {
                if let Err(e) = run(&p, &s, cache.as_deref()) {
                    let mut st = s.state.lock().unwrap();
                    st.error = Some(e);
                    st.finished = true;
                    st.fresh = true;
                }
            })
            .expect("a thread for thumbnails");
        Thumbs {
            path: path.to_path_buf(),
            shared,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the video holds, once opened.
    pub fn info(&self) -> Option<Info> {
        self.shared.state.lock().unwrap().info
    }

    /// Why the pictures couldn't be made, if they couldn't.
    pub fn error(&self) -> Option<Error> {
        self.shared.state.lock().unwrap().error.clone()
    }

    /// Whether the first pass's pictures came from the cache.
    pub fn from_cache(&self) -> bool {
        self.shared.state.lock().unwrap().from_cache
    }

    /// How far through the first pass it is, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let st = self.shared.state.lock().unwrap();
        if st.finished {
            return 1.0;
        }
        match st.info {
            Some(i) => st.done as f32 / i.frames() as f32,
            None => 0.0,
        }
    }

    /// Whether a picture has come in since this was last asked.
    pub fn changed(&self) -> bool {
        std::mem::take(&mut self.shared.state.lock().unwrap().fresh)
    }

    /// Frame `n`'s picture if it has been made; or else, the nearest
    /// one before it that has, with `false`, to show meanwhile (and
    /// frame `n` is asked for).
    pub fn get(&self, n: u64) -> Option<(Arc<Frame>, bool)> {
        let mut st = self.shared.state.lock().unwrap();
        if let Some(f) = st.kept.get(&n).or_else(|| st.extra.get(&n)) {
            return Some((f.clone(), true));
        }
        if st.finished && st.error.is_some() {
            return None;
        }
        // The first pass keeps every frame of a short video anyway.
        let coming = st.every == 1 && !st.finished;
        if !coming && !st.asked.contains(&n) {
            st.asked.push(n);
            self.shared.wake.notify_all();
        }
        let every = st.every.max(1);
        let before = n / every * every;
        (0..=before / every)
            .rev()
            .take(4)
            .find_map(|k| st.kept.get(&(k * every)))
            .map(|f| (f.clone(), false))
    }

    /// Forgets frames asked for that aren't wanted any more, so the
    /// latest ones are made first.
    pub fn only_want(&self, frames: &[u64]) {
        self.shared
            .state
            .lock()
            .unwrap()
            .asked
            .retain(|n| frames.contains(n));
    }
}

/// The frame number of a decoded frame.
fn number(info: &Info, f: &Frame) -> u64 {
    // A quarter frame on, so a time a hair early still lands in it.
    info.frame_at(f.time + 0.25 / info.fps().max(1e-6))
}

fn run(path: &Path, s: &Shared, cache: Option<&Path>) -> Result<(), Error> {
    // Kept from before: no first pass, and the file is only opened when
    // a frame is asked for.
    if let Some(strip) = cache.and_then(|d| crate::cache::load(d, path).ok()) {
        {
            let mut st = s.state.lock().unwrap();
            st.info = Some(strip.info);
            st.every = strip.every;
            st.kept = strip.kept;
            st.done = strip.info.frames();
            st.finished = true;
            st.from_cache = true;
            st.fresh = true;
        }
        let mut r = None;
        return asked(&mut r, path, &strip.info, s);
    }
    let mut r = Reader::open(path, (HEIGHT * 4, HEIGHT))?;
    let info = *r.info();
    let every = info.frames().div_ceil(KEEP).max(1);
    {
        let mut st = s.state.lock().unwrap();
        st.info = Some(info);
        st.every = every;
    }
    // The first pass, stopping to make frames asked for.
    let mut at = 0u64;
    loop {
        if s.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if serve(&mut r, &info, s)? {
            r.seek(info.frame_time(at))?;
        }
        let Some(f) = r.next_frame()? else { break };
        let n = number(&info, &f);
        at = n + 1;
        let mut st = s.state.lock().unwrap();
        st.done = at;
        if n.is_multiple_of(every) {
            st.kept.insert(n, Arc::new(f));
            st.fresh = true;
        }
    }
    let kept = {
        let mut st = s.state.lock().unwrap();
        st.finished = true;
        st.fresh = true;
        cache.map(|_| st.kept.clone())
    };
    if let (Some(dir), Some(kept)) = (cache, kept) {
        // A cache that can't be written only means decoding again
        // next time.
        let _ = crate::cache::save(dir, path, &crate::cache::Strip { info, every, kept });
    }
    asked(&mut Some(r), path, &info, s)
}

/// Makes frames as they are asked for, until stopped, opening the file
/// when the first is.
fn asked(r: &mut Option<Reader>, path: &Path, info: &Info, s: &Shared) -> Result<(), Error> {
    loop {
        {
            let mut st = s.state.lock().unwrap();
            while st.asked.is_empty() && !s.stop.load(Ordering::Relaxed) {
                st = s.wake.wait(st).unwrap();
            }
        }
        if s.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if r.is_none() {
            *r = Some(Reader::open(path, (HEIGHT * 4, HEIGHT))?);
        }
        serve(r.as_mut().expect("opened"), info, s)?;
    }
}

/// Makes the frames asked for, nearest first. Returns whether it moved
/// the reader.
fn serve(r: &mut Reader, info: &Info, s: &Shared) -> Result<bool, Error> {
    let mut moved = false;
    loop {
        let next = {
            let mut st = s.state.lock().unwrap();
            st.asked.sort_unstable();
            st.asked.dedup();
            if st.asked.is_empty() {
                return Ok(moved);
            }
            st.asked.remove(0)
        };
        // Frames close after it come from the same decoding run.
        r.seek(info.frame_time(next))?;
        moved = true;
        let mut want = next;
        while let Some(f) = r.next_frame()? {
            let n = number(info, &f);
            let mut st = s.state.lock().unwrap();
            if n >= want {
                let f = Arc::new(f);
                st.extra.insert(n, f);
                st.extra_order.push_back(n);
                while st.extra_order.len() > EXTRA {
                    if let Some(old) = st.extra_order.pop_front() {
                        st.extra.remove(&old);
                    }
                }
                st.fresh = true;
            }
            st.asked.retain(|&a| a != n);
            // Carry on while the next one asked for is just ahead.
            match st.asked.iter().copied().filter(|&a| a > n).min() {
                Some(a) if a <= n + info.fps().ceil() as u64 => want = a,
                _ => break,
            }
        }
    }
}
