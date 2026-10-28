// SPDX-License-Identifier: GPL-3.0-or-later
//! Playing audio files from disk without loading them whole (roadmap 1.1).
//!
//! Files are read in chunks of [`CHUNK`] frames by a disk thread, the
//! [`Streamer`]. It watches the play position and loop, and keeps the
//! chunks just ahead of the playhead (and at the loop start) loaded. Each
//! chunk is a single allocation, made on the disk thread and moved to the
//! audio thread through a lock-free queue. The audio thread only stores
//! it in a slot of the graph, and hands it back when the disk thread asks
//! for it to be dropped, so all freeing happens off the audio thread too.
//!
//! If a chunk the audio thread needs has not arrived (a disk too slow, or
//! just after a seek), it plays silence for that part and counts an
//! underrun. [`Streamer::service`] can also be called directly, to fill
//! the cache before playback starts.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::engine::Status;

use rtrb::{Consumer, Producer};

/// Frames per chunk: about 0.7 s at 48 kHz.
pub const CHUNK: usize = 32_768;
/// How far ahead of the playhead to keep loaded, in chunks.
const AHEAD: usize = 5;
/// Frames kept loaded behind the play position: the practice stretcher
/// reads a little before where it plays.
const BEHIND: i64 = 8192;

/// A file the engine can stream: probed once, on import or load.
#[derive(Debug)]
pub struct StreamSource {
    /// Unique for the life of the process, so the audio thread can match
    /// chunks to sources with a number compare.
    pub(crate) id: u64,
    pub path: PathBuf,
    pub channels: usize,
    pub frames: usize,
    pub rate: u32,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl StreamSource {
    /// Reads the file's header. WAV files with 16-, 24- or 32-bit integer
    /// or 32-bit float samples, mono or stereo.
    pub fn open(path: &Path) -> Result<Arc<StreamSource>, String> {
        let r = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let spec = r.spec();
        if !(1..=2).contains(&spec.channels) {
            return Err(format!(
                "{}: only mono and stereo are supported",
                path.display()
            ));
        }
        Ok(Arc::new(StreamSource {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            path: path.to_path_buf(),
            channels: spec.channels as usize,
            frames: r.duration() as usize,
            rate: spec.sample_rate,
        }))
    }

    pub fn chunks(&self) -> usize {
        self.frames.div_ceil(CHUNK)
    }

    /// Reads up to `frames` frames from the start, mixed to mono. For
    /// analysis off the audio thread.
    pub fn read_mono(&self, frames: usize) -> Result<Vec<f32>, String> {
        let mut r = hound::WavReader::open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let s = read_samples(&mut r, frames * self.channels).map_err(|e| e.to_string())?;
        Ok(s.chunks_exact(self.channels)
            .map(|f| f.iter().sum::<f32>() / self.channels as f32)
            .collect())
    }
}

/// One chunk of a file: `channels` interleaved, up to [`CHUNK`] frames.
pub struct Chunk {
    pub(crate) source: u64,
    pub(crate) index: usize,
    pub(crate) channels: usize,
    pub(crate) samples: Box<[f32]>,
}

impl Chunk {
    pub(crate) fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }
}

/// Requests from the disk thread to the audio thread.
pub enum ToAudio {
    Loaded(Box<Chunk>),
    /// Give back this chunk; it is no longer needed.
    Drop {
        source: u64,
        index: usize,
    },
}

/// Where a streamed clip sits in the timeline, for the disk thread.
#[derive(Clone, Debug)]
pub struct Placement {
    pub source: Arc<StreamSource>,
    /// Timeline frames `[start, end)` play file frames from `offset`.
    pub start: i64,
    pub end: i64,
    pub offset: usize,
}

/// Where playback is and what loops, read by the disk thread each pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct Playhead {
    pub position: i64,
    pub looping: Option<(i64, i64)>,
}

/// The disk thread's side.
pub struct Streamer {
    to_audio: Producer<ToAudio>,
    returned: Consumer<Box<Chunk>>,
    status: Arc<Status>,
    /// Set by the controller with each new graph.
    shared: Arc<Mutex<Vec<Placement>>>,
    placements: Vec<Placement>,
    /// Chunks the audio thread holds or has been sent.
    sent: HashSet<(u64, usize)>,
    /// Asked back, not yet returned.
    dropping: HashSet<(u64, usize)>,
    readers: HashMap<u64, hound::WavReader<BufReader<File>>>,
    pub errors: Vec<String>,
}

impl Streamer {
    pub(crate) fn new(
        to_audio: Producer<ToAudio>,
        returned: Consumer<Box<Chunk>>,
        status: Arc<Status>,
        shared: Arc<Mutex<Vec<Placement>>>,
    ) -> Streamer {
        Streamer {
            to_audio,
            returned,
            status,
            shared,
            placements: Vec::new(),
            sent: HashSet::new(),
            dropping: HashSet::new(),
            readers: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn refresh_placements(&mut self) {
        let p = self.shared.lock().unwrap().clone();
        self.placements = p;
        let live: HashSet<u64> = self.placements.iter().map(|p| p.source.id).collect();
        self.readers.retain(|id, _| live.contains(id));
    }

    /// Runs passes every few milliseconds on a thread of its own, until
    /// the returned handle is dropped.
    pub fn spawn(mut self) -> StreamerThread {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::Builder::new()
            .name("disk streaming".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    if self.service() == 0 {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            })
            .expect("could not start the disk thread");
        StreamerThread {
            stop,
            handle: Some(handle),
        }
    }

    /// Chunks wanted for a playhead: the next few at the position, and at
    /// the loop start.
    fn wanted(&self, at: Playhead) -> HashSet<(u64, usize)> {
        let mut want = HashSet::new();
        let span = (AHEAD * CHUNK) as i64;
        let mut windows = vec![(at.position - BEHIND, at.position + span)];
        if let Some((s, e)) = at.looping {
            windows.push((s, (s + span).min(e)));
            if at.position >= s && at.position < e && at.position + span > e {
                // Close to the loop end: the loop start is next.
                windows.push((s, s + span - (e - at.position)));
            }
        }
        for p in &self.placements {
            for &(a, b) in &windows {
                let (from, to) = (a.max(p.start), b.min(p.end));
                if from >= to {
                    continue;
                }
                let f0 = p.offset + (from - p.start) as usize;
                let f1 = p.offset + (to - p.start) as usize;
                for i in f0 / CHUNK..f1.div_ceil(CHUNK).min(p.source.chunks()) {
                    want.insert((p.source.id, i));
                }
            }
        }
        want
    }

    /// One pass: takes back returned chunks, asks the audio thread to drop
    /// chunks no longer wanted, and loads missing ones. Returns how many
    /// chunks it loaded.
    pub fn service(&mut self) -> usize {
        self.refresh_placements();
        let at = self.status.playhead();
        while let Ok(c) = self.returned.pop() {
            let key = (c.source, c.index);
            self.sent.remove(&key);
            self.dropping.remove(&key);
        }
        let want = self.wanted(at);
        let stale: Vec<_> = self
            .sent
            .iter()
            .filter(|k| !want.contains(k) && !self.dropping.contains(k))
            .copied()
            .collect();
        for (source, index) in stale {
            if self.to_audio.push(ToAudio::Drop { source, index }).is_ok() {
                self.dropping.insert((source, index));
            }
        }
        let mut missing: Vec<_> = want.difference(&self.sent).copied().collect();
        missing.sort();
        let mut loaded = 0;
        for (source, index) in missing {
            if self.to_audio.slots() == 0 {
                break;
            }
            let Some(src) = self
                .placements
                .iter()
                .find(|p| p.source.id == source)
                .map(|p| p.source.clone())
            else {
                continue;
            };
            match self.read(&src, index) {
                Ok(chunk) => {
                    if self.to_audio.push(ToAudio::Loaded(chunk)).is_ok() {
                        self.sent.insert((source, index));
                        loaded += 1;
                    }
                }
                Err(e) => {
                    if self.errors.len() < 100 {
                        self.errors.push(e);
                    }
                }
            }
        }
        loaded
    }

    fn read(&mut self, src: &StreamSource, index: usize) -> Result<Box<Chunk>, String> {
        let err = |e: hound::Error| format!("{}: {e}", src.path.display());
        let r = match self.readers.entry(src.id) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(hound::WavReader::open(&src.path).map_err(err)?)
            }
        };
        let start = index * CHUNK;
        let frames = CHUNK.min(src.frames - start);
        r.seek(start as u32)
            .map_err(|e| format!("{}: {e}", src.path.display()))?;
        let samples = read_samples(r, frames * src.channels).map_err(err)?;
        Ok(Box::new(Chunk {
            source: src.id,
            index,
            channels: src.channels,
            samples: samples.into_boxed_slice(),
        }))
    }
}

/// Reads up to `n` samples from where the reader is, as floats in
/// -1.0..1.0, whatever the file stores.
pub(crate) fn read_samples<R: std::io::Read>(
    r: &mut hound::WavReader<R>,
    n: usize,
) -> Result<Vec<f32>, hound::Error> {
    let spec = r.spec();
    match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().take(n).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .take(n)
                .map(|s| s.map(|v| v as f32 * scale))
                .collect()
        }
    }
}

/// A streamed source's chunk slots on the audio thread.
pub(crate) struct StreamNode {
    pub id: u64,
    pub slots: Vec<Option<Box<Chunk>>>,
}

/// The disk thread; stops it when dropped.
pub struct StreamerThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for StreamerThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
