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
//!
//! With a punch range, only input inside it is recorded, and the take
//! ends at the punch-out frame rather than when playback stops.
//!
//! Recording at a practice speed (Phase 4 D5): while the song plays
//! slowed down (or transposed), input is captured as it is played, into a
//! file of its own, and when the take ends the recorder speeds it back up
//! (and transposes it back) to the song's speed with the practice
//! stretcher, so the take lands in time with the song. Blocks then carry
//! the speed they were played at, and their `at` counts captured frames
//! from the timeline frame where the take began, so a take follows on
//! block by block as usual. A take played at another speed is a new take.
//!
//! Retrospective capture (roadmap 2.2): while it is on, armed tracks'
//! input played but *not* recorded is kept in memory here, for the last
//! so many seconds of playing, in stretches of continuous audio: one per
//! pass, placed where it was played. Input while stopped is not kept: it
//! has no place in the song. [`Controller::keep_capture`] writes what is
//! kept as takes, oldest first, and empties the memory.
//!
//! [`Controller::keep_capture`]: crate::engine::Controller::keep_capture

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rtrb::Consumer;

use crate::engine::Status;
use crate::time::{Frames, TempoMap, Ticks};

/// Frames of stereo in one block from the audio thread; a mono block
/// holds twice as many.
pub const BLOCK_FRAMES: usize = 128;
/// Samples in one block.
pub(crate) const BLOCK_SAMPLES: usize = BLOCK_FRAMES * 2;

/// What a [`RecBlock`] is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Recorded into a take.
    Record,
    /// Played but not recorded: kept for retrospective capture.
    Capture,
    /// The track's take ends here (punch out); no samples.
    End,
}

/// One track's input for up to [`BLOCK_SAMPLES`] samples.
#[derive(Clone, Copy)]
pub struct RecBlock {
    pub(crate) track: u16,
    pub(crate) channels: u8,
    pub(crate) len: u16,
    pub(crate) kind: Kind,
    /// Timeline frame of the first sample. At a practice speed: the
    /// take's first timeline frame plus the frames captured since.
    pub(crate) at: i64,
    /// The practice speed and transpose the input was played at.
    pub(crate) speed: f32,
    pub(crate) semitones: f32,
    /// Interleaved, `len * channels` used.
    pub(crate) samples: [f32; BLOCK_SAMPLES],
}

impl RecBlock {
    pub(crate) const EMPTY: RecBlock = RecBlock {
        track: 0,
        channels: 1,
        len: 0,
        kind: Kind::Record,
        at: 0,
        speed: 1.0,
        semitones: 0.0,
        samples: [0.0; BLOCK_SAMPLES],
    };

    fn slowed(&self) -> bool {
        self.speed != 1.0 || self.semitones != 0.0
    }
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
    /// Set if the take was played at a practice speed or transpose; then
    /// `path` and `frames` are the take sped back up to the song's speed.
    pub slowed: Option<Slowed>,
}

/// How a take recorded at a practice speed was played.
#[derive(Clone, Debug, PartialEq)]
pub struct Slowed {
    pub speed: f64,
    pub semitones: f64,
    /// The input as it was captured, at the practice speed.
    pub as_played: PathBuf,
}

impl Take {
    /// Where the take belongs in the timeline: `offset` frames before it
    /// was captured, `offset` being the interface's measured round trip.
    /// The round trip passes at the practice speed, so a slowed take
    /// moves back by less of the song.
    pub fn clip_start(&self, offset: Frames) -> Frames {
        let speed = self.slowed.as_ref().map_or(1.0, |s| s.speed);
        Frames(self.start.0 - (offset.0 as f64 * speed).round() as i64)
    }
}

struct Open {
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    take: Take,
    /// The frame the next block should start at to continue this take.
    next: i64,
    /// The practice speed and transpose its blocks were played at.
    played: (f32, f32),
}

/// How often open takes have their headers brought up to date.
const CHECKPOINT: Duration = Duration::from_secs(1);

/// A stretch of continuous input kept for retrospective capture.
struct Stretch {
    /// Timeline frame of the first kept sample.
    start: i64,
    /// The frame a block must start at to continue this stretch.
    next: i64,
    channels: usize,
    samples: VecDeque<f32>,
}

impl Stretch {
    fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }
}

/// Shared between the controller and the recorder for retrospective
/// capture.
#[derive(Default)]
pub(crate) struct CaptureShared {
    /// Seconds to keep; 0 is off.
    pub(crate) seconds: AtomicU32,
    /// Set by the controller; the recorder writes what it holds, then
    /// clears it.
    pub(crate) keep: AtomicBool,
    /// Frames held for the track with the most, for the UI.
    pub(crate) held: AtomicU32,
}

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
    capture: Arc<CaptureShared>,
    /// Per track, oldest first.
    kept: HashMap<usize, VecDeque<Stretch>>,
}

impl Recorder {
    pub(crate) fn new(
        blocks: Consumer<RecBlock>,
        status: Arc<Status>,
        dir: Arc<Mutex<PathBuf>>,
        rate: Arc<Mutex<u32>>,
        finished: Arc<Mutex<Vec<Take>>>,
        capture: Arc<CaptureShared>,
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
            capture,
            kept: HashMap::new(),
        }
    }

    /// One pass: writes every block waiting, finishes takes that have
    /// ended, checkpoints open ones, and keeps captured input if asked.
    /// Returns the blocks handled.
    pub fn service(&mut self) -> usize {
        let mut n = 0;
        while let Ok(b) = self.blocks.pop() {
            match b.kind {
                Kind::Record => self.write(&b),
                Kind::Capture => self.hold(&b),
                Kind::End => self.finish(b.track as usize),
            }
            n += 1;
        }
        if n > 0 || self.capture.seconds.load(Ordering::Relaxed) == 0 {
            self.trim_capture();
        }
        if self.capture.keep.swap(false, Ordering::Relaxed) {
            self.keep_capture();
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
        let follows = self.open.get(&track).is_some_and(|o| {
            o.next == b.at && o.take.channels == channels && o.played == (b.speed, b.semitones)
        });
        if !follows {
            self.finish(track);
            if let Err(e) = self.start(track, channels, b) {
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

    /// Adds a block to the track's kept input, continuing the newest
    /// stretch if it follows on.
    fn hold(&mut self, b: &RecBlock) {
        // Input that isn't recorded comes after the end of any take on the
        // track (blocks arrive in order).
        self.finish(b.track as usize);
        if self.capture.seconds.load(Ordering::Relaxed) == 0 {
            return;
        }
        let channels = b.channels as usize;
        let kept = self.kept.entry(b.track as usize).or_default();
        let follows = kept
            .back()
            .is_some_and(|s| s.next == b.at && s.channels == channels);
        if !follows {
            kept.push_back(Stretch {
                start: b.at,
                next: b.at,
                channels,
                samples: VecDeque::new(),
            });
        }
        let s = kept.back_mut().expect("just made sure there is one");
        s.samples.extend(&b.samples[..b.len as usize * channels]);
        s.next = b.at + b.len as i64;
    }

    /// Drops kept input older than the capture window, and publishes how
    /// much is held.
    fn trim_capture(&mut self) {
        let secs = self.capture.seconds.load(Ordering::Relaxed);
        let limit = secs as usize * *self.rate.lock().unwrap() as usize;
        let mut held = 0;
        for kept in self.kept.values_mut() {
            let mut total: usize = kept.iter().map(Stretch::frames).sum();
            while total > limit {
                let Some(s) = kept.front_mut() else { break };
                let cut = (total - limit).min(s.frames());
                s.samples.drain(..cut * s.channels);
                s.start += cut as i64;
                total -= cut;
                if s.samples.is_empty() {
                    kept.pop_front();
                }
            }
            held = held.max(total);
        }
        self.kept.retain(|_, k| !k.is_empty());
        self.capture
            .held
            .store(held.min(u32::MAX as usize) as u32, Ordering::Relaxed);
    }

    /// Writes all kept input as takes, oldest first on each track, and
    /// empties it.
    fn keep_capture(&mut self) {
        let mut tracks: Vec<usize> = self.kept.keys().copied().collect();
        tracks.sort_unstable();
        for track in tracks {
            for s in self.kept.remove(&track).unwrap_or_default() {
                let take = self.write_kept(track, &s);
                self.finished.lock().unwrap().push(take);
            }
        }
        self.capture.held.store(0, Ordering::Relaxed);
    }

    fn write_kept(&mut self, track: usize, s: &Stretch) -> Take {
        let mut take = Take {
            track,
            path: PathBuf::new(),
            start: Frames(s.start),
            frames: s.frames(),
            channels: s.channels,
            error: None,
            slowed: None,
        };
        let result = self
            .create(track, s.channels, s.start, "kept")
            .and_then(|(path, mut w)| {
                take.path = path;
                for x in &s.samples {
                    w.write_sample(*x).map_err(|e| e.to_string())?;
                }
                w.finalize().map_err(|e| e.to_string())
            });
        if let Err(e) = result {
            take.error = Some(e);
        }
        take
    }

    /// A new take file.
    fn create(
        &mut self,
        track: usize,
        channels: usize,
        at: i64,
        what: &str,
    ) -> Result<(PathBuf, hound::WavWriter<std::io::BufWriter<std::fs::File>>), String> {
        let dir = self.dir.lock().unwrap().clone();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        self.seq += 1;
        let path = take_path(&dir, track, what, at, self.seq);
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
        Ok((path, writer))
    }

    fn start(&mut self, track: usize, channels: usize, b: &RecBlock) -> Result<(), String> {
        let at = b.at;
        let what = if b.slowed() { "slow" } else { "at" };
        let (path, writer) = self.create(track, channels, at, what)?;
        let slowed = b.slowed().then(|| Slowed {
            speed: b.speed as f64,
            semitones: b.semitones as f64,
            as_played: path.clone(),
        });
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
                    slowed,
                },
                next: at,
                played: (b.speed, b.semitones),
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
            slowed: None,
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
        if let Some(sl) = o.take.slowed.clone()
            && o.take.frames > 0
        {
            let t = &o.take;
            let sped = self
                .create(track, t.channels, t.start.0, "at")
                .and_then(|(path, w)| {
                    speed_up(&sl.as_played, w, sl.speed, sl.semitones).map(|n| (path, n))
                });
            match sped {
                Ok((path, frames)) => {
                    o.take.path = path;
                    o.take.frames = frames;
                }
                // The take stays as played, so nothing is lost.
                Err(e) => o.take.error = Some(format!("could not speed the take up: {e}")),
            }
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

/// Writes the take in `slow`, played at `speed` and `semitones`, to
/// `out` at the song's speed and pitch: `speed` times as long. Returns
/// the frames written.
pub fn speed_up(
    slow: &Path,
    mut out: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    speed: f64,
    semitones: f64,
) -> Result<usize, String> {
    let mut r = hound::WavReader::open(slow).map_err(|e| format!("{}: {e}", slow.display()))?;
    let ch = r.spec().channels.max(1) as usize;
    let samples: Vec<f32> = r
        .samples::<f32>()
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", slow.display()))?;
    let (l, rt): (Vec<f32>, Vec<f32>) = samples.chunks_exact(ch).map(|f| (f[0], f[ch - 1])).unzip();
    let (ol, or) = crate::stretch::whole(&l, &rt, 1.0 / speed, -semitones);
    for (a, b) in ol.iter().zip(&or) {
        out.write_sample(*a).map_err(|e| e.to_string())?;
        if ch > 1 {
            out.write_sample(*b).map_err(|e| e.to_string())?;
        }
    }
    out.finalize().map_err(|e| e.to_string())?;
    Ok(ol.len())
}

/// A punch range on the timeline, with pre-roll and post-roll in bars:
/// playback starts `pre_roll` bars before `start` and stops `post_roll`
/// bars after `end`, recording only in between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Punch {
    pub start: Frames,
    pub end: Frames,
    pub pre_roll: u32,
    pub post_roll: u32,
}

impl Punch {
    /// Where playback starts: the pre-roll before `start`, but not before
    /// the beginning of the song.
    pub fn play_from(&self, map: &TempoMap) -> Frames {
        let at = map.ticks_at(self.start);
        let bar = map.signature_at(at).bar_ticks();
        Frames(
            map.frames_at(Ticks(at.0 - bar * self.pre_roll as i64))
                .0
                .max(0),
        )
    }

    /// Where playback stops by itself: the post-roll after `end`, and
    /// never before the input played at `end` has arrived, `offset` (the
    /// interface's round trip) later.
    pub fn stop_at(&self, map: &TempoMap, offset: Frames) -> Frames {
        let at = map.ticks_at(self.end);
        let bar = map.signature_at(at).bar_ticks();
        let post = map.frames_at(Ticks(at.0 + bar * self.post_roll as i64));
        Frames(post.0.max(self.end.0 + offset.0))
    }

    /// The range for [`Command::SetPunch`]: later by `offset`, the
    /// interface's round trip, because input arrives that much after it
    /// was played (see [`Take::clip_start`]).
    ///
    /// [`Command::SetPunch`]: crate::engine::Command::SetPunch
    pub fn capture_range(&self, offset: Frames) -> (Frames, Frames) {
        (
            Frames(self.start.0 + offset.0),
            Frames(self.end.0 + offset.0),
        )
    }
}

/// `track03-at000123456-…-0007.wav`: track (from 1), start frame, the
/// time, and a sequence number so names never collide. Input kept by
/// retrospective capture says `kept` in place of `at`, and input as
/// played at a practice speed says `slow`.
fn take_path(dir: &Path, track: usize, what: &str, at: i64, seq: u64) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    dir.join(format!(
        "track{:02}-{what}{at:09}-{stamp}-{seq:04}.wav",
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
