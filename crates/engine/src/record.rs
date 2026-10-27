// SPDX-License-Identifier: GPL-3.0-or-later
//! Recording (roadmap 1.2).
//!
//! While recording, the audio thread copies each armed track's input into
//! fixed-size [`RecBlock`]s and pushes them into a lock-free queue. The
//! [`Recorder`], on a thread of its own, writes them to one WAV file per
//! take (32-bit float) and finishes the file's header about once a second,
//! so if the app or the computer dies mid-take, what was captured up to
//! then is still a readable file.
//!
//! A new take starts whenever a track's audio stops following on from the
//! last block: when recording starts, and each time a loop goes round. So
//! loop recording leaves one take per pass. A take's file name carries
//! its track and start frame, so takes can be found again after a crash.
//!
//! Takes are positioned in absolute time, where the input was captured.
//! The input reaches the computer later than the sound was made (the
//! interface's round trip), so the app places the take that much earlier;
//! see [`Take::clip_start`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rtrb::Consumer;

use crate::engine::Status;
use crate::time::Frames;

/// Frames in one block from the audio thread.
pub const BLOCK_FRAMES: usize = 128;

/// One track's input for up to [`BLOCK_FRAMES`] frames.
#[derive(Clone, Copy)]
pub struct RecBlock {
    pub(crate) track: u16,
    pub(crate) channels: u8,
    pub(crate) len: u16,
    /// Timeline frame of the first sample.
    pub(crate) at: i64,
    /// Interleaved, `len * channels` used.
    pub(crate) samples: [f32; BLOCK_FRAMES * 2],
}

impl RecBlock {
    pub(crate) const EMPTY: RecBlock = RecBlock {
        track: 0,
        channels: 1,
        len: 0,
        at: 0,
        samples: [0.0; BLOCK_FRAMES * 2],
    };
}

/// A finished take.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    pub track: usize,
    pub path: PathBuf,
    /// Timeline frame where capture began.
    pub start: Frames,
    pub frames: usize,
    pub channels: usize,
    /// Set if a write failed; the file holds what was written before.
    pub error: Option<String>,
}

impl Take {
    /// Where the take belongs in the timeline: `offset` frames before it
    /// was captured, `offset` being the interface's measured round trip.
    pub fn clip_start(&self, offset: Frames) -> Frames {
        Frames(self.start.0 - offset.0)
    }
}

struct Open {
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    take: Take,
    /// The frame the next block should start at to continue this take.
    next: i64,
}

/// How often open takes have their headers brought up to date.
const CHECKPOINT: Duration = Duration::from_secs(1);

/// The disk side of recording.
pub struct Recorder {
    blocks: Consumer<RecBlock>,
    status: Arc<Status>,
    dir: Arc<Mutex<PathBuf>>,
    rate: Arc<Mutex<u32>>,
    open: HashMap<usize, Open>,
    finished: Arc<Mutex<Vec<Take>>>,
    last_checkpoint: Instant,
    seq: u64,
}

impl Recorder {
    pub(crate) fn new(
        blocks: Consumer<RecBlock>,
        status: Arc<Status>,
        dir: Arc<Mutex<PathBuf>>,
        rate: Arc<Mutex<u32>>,
        finished: Arc<Mutex<Vec<Take>>>,
    ) -> Recorder {
        Recorder {
            blocks,
            status,
            dir,
            rate,
            open: HashMap::new(),
            finished,
            last_checkpoint: Instant::now(),
            seq: 0,
        }
    }

    /// One pass: writes every block waiting, finishes takes that have
    /// ended, and checkpoints open ones. Returns the blocks written.
    pub fn service(&mut self) -> usize {
        let mut n = 0;
        while let Ok(b) = self.blocks.pop() {
            self.write(&b);
            n += 1;
        }
        // Recording or playback has stopped, and everything sent has been
        // written.
        if !(self.status.recording() && self.status.playing()) && self.blocks.is_empty() {
            let tracks: Vec<usize> = self.open.keys().copied().collect();
            for t in tracks {
                self.finish(t);
            }
        }
        if self.last_checkpoint.elapsed() >= CHECKPOINT {
            self.last_checkpoint = Instant::now();
            for o in self.open.values_mut() {
                if o.take.error.is_none()
                    && let Err(e) = o.writer.flush()
                {
                    o.take.error = Some(e.to_string());
                }
            }
        }
        n
    }

    fn write(&mut self, b: &RecBlock) {
        let track = b.track as usize;
        let channels = b.channels as usize;
        let follows = self
            .open
            .get(&track)
            .is_some_and(|o| o.next == b.at && o.take.channels == channels);
        if !follows {
            self.finish(track);
            if let Err(e) = self.start(track, channels, b.at) {
                self.report_failed(track, channels, b.at, e);
                return;
            }
        }
        let Some(o) = self.open.get_mut(&track) else {
            return;
        };
        if o.take.error.is_none() {
            for s in &b.samples[..b.len as usize * channels] {
                if let Err(e) = o.writer.write_sample(*s) {
                    o.take.error = Some(e.to_string());
                    break;
                }
            }
        }
        o.take.frames += b.len as usize;
        o.next = b.at + b.len as i64;
    }

    fn start(&mut self, track: usize, channels: usize, at: i64) -> Result<(), String> {
        let dir = self.dir.lock().unwrap().clone();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        self.seq += 1;
        let path = take_path(&dir, track, at, self.seq);
        let rate = *self.rate.lock().unwrap();
        let writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: channels as u16,
                sample_rate: rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        self.open.insert(
            track,
            Open {
                writer,
                take: Take {
                    track,
                    path,
                    start: Frames(at),
                    frames: 0,
                    channels,
                    error: None,
                },
                next: at,
            },
        );
        Ok(())
    }

    fn report_failed(&mut self, track: usize, channels: usize, at: i64, e: String) {
        self.finished.lock().unwrap().push(Take {
            track,
            path: PathBuf::new(),
            start: Frames(at),
            frames: 0,
            channels,
            error: Some(e),
        });
    }

    fn finish(&mut self, track: usize) {
        let Some(mut o) = self.open.remove(&track) else {
            return;
        };
        if let Err(e) = o.writer.finalize()
            && o.take.error.is_none()
        {
            o.take.error = Some(e.to_string());
        }
        self.finished.lock().unwrap().push(o.take);
    }

    /// Runs passes every few milliseconds on a thread of its own, until
    /// the returned handle is dropped. Dropping it finishes open takes.
    pub fn spawn(mut self) -> RecorderThread {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::Builder::new()
            .name("recording".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    if self.service() == 0 {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                self.service();
                let tracks: Vec<usize> = self.open.keys().copied().collect();
                for t in tracks {
                    self.finish(t);
                }
            })
            .expect("could not start the recording thread");
        RecorderThread {
            stop,
            handle: Some(handle),
        }
    }
}

/// `track03-at000123456-0007.wav`: track (from 1), start frame, and a
/// sequence number so names never collide.
fn take_path(dir: &Path, track: usize, at: i64, seq: u64) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    dir.join(format!(
        "track{:02}-at{at:09}-{stamp}-{seq:04}.wav",
        track + 1
    ))
}

/// The recording thread; finishes open takes and stops when dropped.
pub struct RecorderThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for RecorderThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
