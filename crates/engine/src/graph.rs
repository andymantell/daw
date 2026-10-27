// SPDX-License-Identifier: GPL-3.0-or-later
//! The render graph (ARCHITECTURE §4.2): what the audio thread plays.
//!
//! A [`Session`] describes tracks, their clips and mixer settings in
//! musical time. [`Session::compile`] turns it, off the audio thread, into
//! a [`Graph`]: positions converted to frames, gains to linear factors,
//! and every buffer allocated up front. The audio thread only reads it.
//!
//! Phase 1 graph: audio clips → track (gain, pan, mute, solo) → master.

use std::sync::Arc;

use crate::metronome::Clicks;
use crate::stream::{CHUNK, Chunk, Placement, StreamNode, StreamSource};
use crate::time::{Frames, TempoMap, Ticks};

/// Most tracks a graph can hold; meters are sized to this up front.
pub const MAX_TRACKS: usize = 256;

/// Decoded audio, one `Vec` per channel (1 or 2 channels).
#[derive(Debug)]
pub struct AudioData {
    pub channels: Vec<Vec<f32>>,
    pub rate: u32,
}

impl AudioData {
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Where a clip's audio comes from.
#[derive(Clone, Debug)]
pub enum Source {
    /// Decoded and held in memory (short sounds, tests).
    Memory(Arc<AudioData>),
    /// Streamed from a file on disk.
    File(Arc<StreamSource>),
}

impl From<Arc<AudioData>> for Source {
    fn from(d: Arc<AudioData>) -> Source {
        Source::Memory(d)
    }
}

impl From<Arc<StreamSource>> for Source {
    fn from(s: Arc<StreamSource>) -> Source {
        Source::File(s)
    }
}

impl Source {
    fn rate(&self) -> u32 {
        match self {
            Source::Memory(d) => d.rate,
            Source::File(f) => f.rate,
        }
    }

    fn channels(&self) -> usize {
        match self {
            Source::Memory(d) => d.channels.len(),
            Source::File(f) => f.channels,
        }
    }

    fn frames(&self) -> usize {
        match self {
            Source::Memory(d) => d.len(),
            Source::File(f) => f.frames,
        }
    }
}

/// A clip placed on a track, in musical time.
#[derive(Clone, Debug)]
pub struct ClipSpec {
    pub start: Ticks,
    pub source: Source,
    /// Where in the source playback begins, in frames.
    pub offset: usize,
    /// How much of the source plays, in frames; `None` for the rest of it.
    pub len: Option<usize>,
    pub gain_db: f32,
}

impl ClipSpec {
    pub fn new(start: Ticks, source: impl Into<Source>) -> ClipSpec {
        ClipSpec {
            start,
            source: source.into(),
            offset: 0,
            len: None,
            gain_db: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TrackSpec {
    pub clips: Vec<ClipSpec>,
    pub mix: Mix,
}

impl TrackSpec {
    pub fn new(clips: Vec<ClipSpec>) -> TrackSpec {
        TrackSpec {
            clips,
            mix: Mix::default(),
        }
    }
}

/// A track's mixer settings. These can also change while playing,
/// without a new graph (see `Command::SetMix`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mix {
    pub gain_db: f32,
    /// −1 (left) to +1 (right).
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for Mix {
    fn default() -> Mix {
        Mix {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// Everything the engine plays.
#[derive(Clone, Debug)]
pub struct Session {
    pub tempo: TempoMap,
    pub tracks: Vec<TrackSpec>,
    pub master_db: f32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompileError {
    TooManyTracks,
    /// A clip's audio is at a different sample rate from the session.
    /// Converting it is the importer's job.
    RateMismatch {
        track: usize,
    },
    BadChannelCount {
        track: usize,
    },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::TooManyTracks => write!(f, "more than {MAX_TRACKS} tracks"),
            CompileError::RateMismatch { track } => {
                write!(
                    f,
                    "track {}: clip audio is at another sample rate",
                    track + 1
                )
            }
            CompileError::BadChannelCount { track } => {
                write!(f, "track {}: clip audio must be mono or stereo", track + 1)
            }
        }
    }
}

impl std::error::Error for CompileError {}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Left and right gains for a track.
///
/// Mono sources are panned with a constant-power law (−3 dB each side in
/// the centre). Stereo sources use a balance control: the centre leaves
/// both channels untouched, and turning towards one side fades the other.
pub fn pan_gains(pan: f32, stereo: bool) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    if stereo {
        ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
    } else {
        let a = (p + 1.0) * std::f32::consts::FRAC_PI_4;
        (a.cos(), a.sin())
    }
}

pub(crate) struct ClipNode {
    pub start: i64,
    pub end: i64,
    pub offset: usize,
    pub gain: f32,
    pub src: NodeSrc,
}

pub(crate) enum NodeSrc {
    Memory(Arc<AudioData>),
    /// Index into `Graph::streams`.
    Stream(usize),
}

pub(crate) struct TrackNode {
    /// Sorted by start.
    pub clips: Vec<ClipNode>,
    pub mix: Mix,
    /// Gains reached at the end of the last block, for smoothing.
    pub last: (f32, f32),
    pub stereo: bool,
}

impl TrackNode {
    /// Left and right gains the mixer settings ask for.
    fn target(&self, any_solo: bool) -> (f32, f32) {
        if self.mix.mute || any_solo && !self.mix.solo {
            return (0.0, 0.0);
        }
        let g = db_to_gain(self.mix.gain_db);
        let (l, r) = pan_gains(self.mix.pan, self.stereo);
        (g * l, g * r)
    }
}

/// Adds `run` frames of a chunk, from frame `at`, into the two buffers.
fn mix_chunk(ch: &Chunk, at: usize, run: usize, gain: f32, l: &mut [f32], r: &mut [f32]) {
    let k = ch.channels;
    let s = &ch.samples[at * k..(at + run) * k];
    for (i, frame) in s.chunks_exact(k).enumerate() {
        l[i] += frame[0] * gain;
        r[i] += frame[k - 1] * gain;
    }
}

/// The compiled, immutable-shape graph the audio thread plays.
pub struct Graph {
    pub(crate) tempo: TempoMap,
    pub(crate) tracks: Vec<TrackNode>,
    pub(crate) master: f32,
    pub(crate) any_solo: bool,
    /// Per-track scratch, two channels each, `max_block` long.
    pub(crate) scratch: Vec<[Vec<f32>; 2]>,
    pub(crate) clicks: Clicks,
    pub(crate) streams: Vec<StreamNode>,
    /// The file each stream reads, for the disk thread.
    placements: Vec<Placement>,
}

impl Session {
    pub fn new(tempo: TempoMap) -> Session {
        Session {
            tempo,
            tracks: Vec::new(),
            master_db: 0.0,
        }
    }

    /// Builds the graph. Allocates; never call on the audio thread.
    pub fn compile(&self, max_block: usize) -> Result<Graph, CompileError> {
        if self.tracks.len() > MAX_TRACKS {
            return Err(CompileError::TooManyTracks);
        }
        let rate = self.tempo.sample_rate();
        let mut tracks = Vec::with_capacity(self.tracks.len());
        let mut streams: Vec<StreamNode> = Vec::new();
        let mut placements = Vec::new();
        for (ti, t) in self.tracks.iter().enumerate() {
            let mut clips = Vec::with_capacity(t.clips.len());
            let mut stereo = false;
            for c in &t.clips {
                if c.source.rate() != rate {
                    return Err(CompileError::RateMismatch { track: ti });
                }
                match c.source.channels() {
                    1 => {}
                    2 => stereo = true,
                    _ => return Err(CompileError::BadChannelCount { track: ti }),
                }
                let avail = c.source.frames().saturating_sub(c.offset);
                let len = c.len.unwrap_or(avail).min(avail);
                if len == 0 {
                    continue;
                }
                let start = self.tempo.frames_at(c.start).0;
                let src = match &c.source {
                    Source::Memory(d) => NodeSrc::Memory(d.clone()),
                    Source::File(f) => {
                        placements.push(Placement {
                            source: f.clone(),
                            start,
                            end: start + len as i64,
                            offset: c.offset,
                        });
                        let i = match streams.iter().position(|s| s.id == f.id) {
                            Some(i) => i,
                            None => {
                                streams.push(StreamNode {
                                    id: f.id,
                                    slots: (0..f.chunks()).map(|_| None).collect(),
                                });
                                streams.len() - 1
                            }
                        };
                        NodeSrc::Stream(i)
                    }
                };
                clips.push(ClipNode {
                    start,
                    end: start + len as i64,
                    offset: c.offset,
                    gain: db_to_gain(c.gain_db),
                    src,
                });
            }
            clips.sort_by_key(|c| c.start);
            tracks.push(TrackNode {
                clips,
                mix: t.mix,
                last: (0.0, 0.0),
                stereo,
            });
        }
        let any_solo = tracks.iter().any(|t| t.mix.solo);
        // Start each track at its settled gains, so a new graph does not
        // fade in.
        for t in &mut tracks {
            t.last = t.target(any_solo);
        }
        Ok(Graph {
            tempo: self.tempo.clone(),
            scratch: (0..tracks.len())
                .map(|_| [vec![0.0; max_block], vec![0.0; max_block]])
                .collect(),
            tracks,
            master: db_to_gain(self.master_db),
            any_solo,
            clicks: Clicks::new(rate),
            streams,
            placements,
        })
    }
}

impl Graph {
    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// The frame where the last clip ends, for offline renders.
    pub fn end(&self) -> Frames {
        Frames(
            self.tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(|c| c.end))
                .max()
                .unwrap_or(0),
        )
    }

    pub(crate) fn set_mix(&mut self, track: usize, mix: Mix) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.mix = mix;
        }
        self.any_solo = self.tracks.iter().any(|t| t.mix.solo);
    }

    /// Plays frames `[pos, pos + n)` of every track into its scratch
    /// buffers, applies the mixer, and adds the tracks into `out`. Peak
    /// levels go to `peaks` (track by track, after the fader).
    /// Returns how many frames of streamed audio were missing.
    pub(crate) fn render(
        &mut self,
        pos: i64,
        out: [&mut [f32]; 2],
        peaks: &mut [(f32, f32)],
    ) -> usize {
        let n = out[0].len();
        let mut missing = 0;
        let [out_l, out_r] = out;
        for (ti, t) in self.tracks.iter_mut().enumerate() {
            let [sl, sr] = &mut self.scratch[ti];
            let (sl, sr) = (&mut sl[..n], &mut sr[..n]);
            sl.fill(0.0);
            sr.fill(0.0);
            // Clips are sorted by start; skip those that end before this
            // block, stop at the first that starts after it.
            for c in &t.clips {
                if c.start >= pos + n as i64 {
                    break;
                }
                if c.end <= pos {
                    continue;
                }
                let from = c.start.max(pos);
                let to = c.end.min(pos + n as i64);
                let dst = (from - pos) as usize;
                let src = c.offset + (from - c.start) as usize;
                let len = (to - from) as usize;
                match &c.src {
                    NodeSrc::Memory(d) => {
                        let chans = &d.channels;
                        let right = chans.get(1).unwrap_or(&chans[0]);
                        for i in 0..len {
                            sl[dst + i] += chans[0][src + i] * c.gain;
                            sr[dst + i] += right[src + i] * c.gain;
                        }
                    }
                    NodeSrc::Stream(si) => {
                        let slots = &self.streams[*si].slots;
                        let mut done = 0;
                        // One chunk at a time.
                        while done < len {
                            let f = src + done;
                            let (ci, within) = (f / CHUNK, f % CHUNK);
                            let run = (len - done).min(CHUNK - within);
                            match slots.get(ci).and_then(|s| s.as_deref()) {
                                Some(ch) => {
                                    let run = run.min(ch.frames().saturating_sub(within));
                                    mix_chunk(
                                        ch,
                                        within,
                                        run,
                                        c.gain,
                                        &mut sl[dst + done..],
                                        &mut sr[dst + done..],
                                    );
                                    missing += (len - done).min(CHUNK - within) - run;
                                }
                                None => missing += run,
                            }
                            done += (len - done).min(CHUNK - within);
                        }
                    }
                }
            }

            let target = t.target(self.any_solo);
            // Ramp linearly from last block's gains so changes don't click.
            let (l0, r0) = t.last;
            let step = 1.0 / n.max(1) as f32;
            let mut peak = (0.0f32, 0.0f32);
            for i in 0..n {
                let k = (i + 1) as f32 * step;
                let gl = l0 + (target.0 - l0) * k;
                let gr = r0 + (target.1 - r0) * k;
                // A mono source feeds both sides of the pan.
                let (a, b) = if t.stereo {
                    (sl[i], sr[i])
                } else {
                    (sl[i], sl[i])
                };
                let (a, b) = (a * gl, b * gr);
                peak = (peak.0.max(a.abs()), peak.1.max(b.abs()));
                out_l[i] += a;
                out_r[i] += b;
            }
            t.last = target;
            if let Some(p) = peaks.get_mut(ti) {
                *p = peak;
            }
        }
        for s in out_l.iter_mut().chain(out_r.iter_mut()) {
            *s *= self.master;
        }
        missing
    }

    /// The streamed clips, for [`Streamer::set_placements`](crate::stream::Streamer::set_placements).
    pub fn placements(&self) -> Vec<Placement> {
        self.placements.clone()
    }

    /// Stores a chunk from the disk thread. Gives it back if no stream in
    /// this graph wants it.
    pub(crate) fn insert_chunk(&mut self, c: Box<Chunk>) -> Option<Box<Chunk>> {
        let Some(s) = self.streams.iter_mut().find(|s| s.id == c.source) else {
            return Some(c);
        };
        match s.slots.get_mut(c.index) {
            Some(slot) => slot.replace(c),
            None => Some(c),
        }
    }

    pub(crate) fn take_chunk(&mut self, source: u64, index: usize) -> Option<Box<Chunk>> {
        self.streams
            .iter_mut()
            .find(|s| s.id == source)?
            .slots
            .get_mut(index)?
            .take()
    }

    /// Moves every loaded chunk out of `old` into this graph where it has
    /// a slot for it; `give_back` gets the rest.
    pub(crate) fn adopt_chunks(&mut self, old: &mut Graph, mut give_back: impl FnMut(Box<Chunk>)) {
        for s in &mut old.streams {
            for slot in &mut s.slots {
                if let Some(c) = slot.take()
                    && let Some(c) = self.insert_chunk(c)
                {
                    give_back(c);
                }
            }
        }
    }
}
