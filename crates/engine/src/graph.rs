// SPDX-License-Identifier: GPL-3.0-or-later
//! The render graph (ARCHITECTURE §4.2): what the audio thread plays.
//!
//! A [`Session`] describes tracks, their clips and mixer settings in
//! musical time. [`Session::compile`] turns it, off the audio thread, into
//! a [`Graph`]: positions converted to frames, gains to linear factors,
//! and every buffer allocated up front. The audio thread only reads it.
//!
//! Phase 1 graph: audio clips and, when monitoring, the track's input →
//! the track's insert effects → track (gain, pan, mute, solo) → master.
//!
//! Insert effects ([`Insert`]) are made off the audio thread and live
//! across graphs: a new graph takes them over from the one it replaces
//! by id, so editing the session never restarts a plugin. A track whose
//! effects add latency has its clips read that far ahead, so it stays in
//! time with the others.

use std::sync::Arc;

use crate::metronome::Clicks;
use crate::stream::{CHUNK, Chunk, Placement, StreamNode, StreamSource};
use crate::stretch::{Feed, Stretcher};
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

/// Where a clip starts. Each clip keeps its own time base (decision
/// 0005): musical clips move with the tempo, absolute ones (recorded
/// takes) stay on the frame they were captured at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Musical(Ticks),
    Absolute(Frames),
}

impl From<Ticks> for Position {
    fn from(t: Ticks) -> Position {
        Position::Musical(t)
    }
}

impl From<Frames> for Position {
    fn from(f: Frames) -> Position {
        Position::Absolute(f)
    }
}

impl Position {
    pub fn frames(self, map: &TempoMap) -> Frames {
        match self {
            Position::Musical(t) => map.frames_at(t),
            Position::Absolute(f) => f,
        }
    }
}

/// A clip placed on a track.
#[derive(Clone, Debug)]
pub struct ClipSpec {
    pub start: Position,
    pub source: Source,
    /// Where in the source playback begins, in frames.
    pub offset: usize,
    /// How much of the source plays, in frames; `None` for the rest of it.
    pub len: Option<usize>,
    pub gain_db: f32,
}

impl ClipSpec {
    pub fn new(start: impl Into<Position>, source: impl Into<Source>) -> ClipSpec {
        ClipSpec {
            start: start.into(),
            source: source.into(),
            offset: 0,
            len: None,
            gain_db: 0.0,
        }
    }
}

/// An effect in a track's insert chain, run on the audio thread.
pub trait Insert: Send {
    /// Processes one block of stereo audio in place, at most the graph's
    /// `max_block` frames. Must not allocate, lock or block.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
}

/// A slot in a track's insert chain. The effect itself is handed to
/// [`Session::compile_with`] once, when first added, under this `id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsertSpec {
    /// Unique for the life of the process.
    pub id: u64,
    pub bypass: bool,
    /// Frames of delay the effect reports, compensated on the track.
    pub latency: u32,
}

#[derive(Clone, Debug)]
pub struct TrackSpec {
    pub clips: Vec<ClipSpec>,
    pub mix: Mix,
    /// Effects, in processing order.
    pub inserts: Vec<InsertSpec>,
    /// The audio input this track records from and monitors.
    pub input: Option<Input>,
    /// Records its input while the transport records.
    pub arm: bool,
    /// Plays its input through the track (software monitoring).
    pub monitor: bool,
}

impl TrackSpec {
    pub fn new(clips: Vec<ClipSpec>) -> TrackSpec {
        TrackSpec {
            clips,
            mix: Mix::default(),
            inserts: Vec::new(),
            input: None,
            arm: false,
            monitor: false,
        }
    }
}

/// Device input channels (from 0) for a track: one, or a stereo pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub first: usize,
    pub stereo: bool,
}

impl Input {
    pub fn mono(ch: usize) -> Input {
        Input {
            first: ch,
            stereo: false,
        }
    }

    pub fn stereo(first: usize) -> Input {
        Input {
            first,
            stereo: true,
        }
    }

    pub fn channels(self) -> usize {
        if self.stereo { 2 } else { 1 }
    }
}

/// One block of device input, interleaved.
#[derive(Clone, Copy)]
pub(crate) struct InputBlock<'a> {
    pub data: &'a [f32],
    pub channels: usize,
}

impl InputBlock<'_> {
    /// Sample `i` of channel `ch`; silence for a channel the device lacks.
    #[inline]
    pub fn get(&self, i: usize, ch: usize) -> f32 {
        if ch < self.channels {
            self.data
                .get(i * self.channels + ch)
                .copied()
                .unwrap_or(0.0)
        } else {
            0.0
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
    pub input: Option<Input>,
    pub arm: bool,
    pub monitor: bool,
    pub inserts: Vec<InsertNode>,
    /// Practice speed and transpose; used only when either is set.
    pub stretch: Box<Stretcher>,
    /// Effects' total latency: clips are read this far ahead.
    pub latency: i64,
    /// The source is mono but the effects are stereo: copy left to right
    /// before them.
    pub widen: bool,
}

pub(crate) struct InsertNode {
    pub id: u64,
    pub bypass: bool,
    /// Taken over from the previous graph, or handed in when new.
    pub effect: Option<Box<dyn Insert>>,
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
/// Adds the clips' audio for frames `[pos, pos + l.len())` into `l` and
/// `r`. Returns how many frames of streamed audio were missing.
fn read_clips(
    clips: &[ClipNode],
    streams: &[StreamNode],
    pos: i64,
    sl: &mut [f32],
    sr: &mut [f32],
) -> usize {
    let n = sl.len();
    let mut missing = 0;
    // Clips are sorted by start; skip those that end before this
    // block, stop at the first that starts after it.
    for c in clips {
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
                let slots = &streams[*si].slots;
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
    missing
}

/// Adds the clips' audio for `l.len()` frames from `start` into `l` and
/// `r`, where `start` is in unwrapped play time: past the loop end it
/// goes back round the loop. Returns how many frames of streamed audio
/// were missing.
fn read_looped(
    clips: &[ClipNode],
    streams: &[StreamNode],
    looping: Option<(i64, i64)>,
    start: i64,
    l: &mut [f32],
    r: &mut [f32],
) -> usize {
    let mut missing = 0;
    let mut done = 0;
    while done < l.len() {
        let u = start + done as i64;
        // Where `u` falls on the timeline, and how far until the loop
        // sends it back.
        let (pos, run) = match looping {
            Some((a, b)) if u >= b && b > a => {
                let p = a + (u - b).rem_euclid(b - a);
                (p, (b - p) as usize)
            }
            Some((_, b)) if u < b => (u, (b - u) as usize),
            _ => (u, usize::MAX),
        };
        let run = run.min(l.len() - done);
        let (sl, sr) = (&mut l[done..done + run], &mut r[done..done + run]);
        missing += read_clips(clips, streams, pos, sl, sr);
        done += run;
    }
    missing
}

/// A track's clips as the stretcher reads them: by position in
/// unwrapped play time, which goes round the loop.
struct TrackFeed<'a> {
    clips: &'a [ClipNode],
    streams: &'a [StreamNode],
    looping: Option<(i64, i64)>,
    missing: usize,
}

impl Feed for TrackFeed<'_> {
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
        self.missing += read_looped(self.clips, self.streams, self.looping, start, l, r);
    }
}

/// Practice speed and transpose for one block of [`Graph::render`].
pub(crate) struct Rate {
    /// Play position at the block's first frame, in unwrapped play time:
    /// it runs on past the loop end, which the feed maps back.
    pub u: f64,
    pub speed: f64,
    pub semitones: f64,
    /// Start the stretchers afresh at `u` (play started, seek, loop
    /// changed).
    pub reset: bool,
}

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
    max_block: usize,
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
        self.compile_with(max_block, &mut Vec::new())
    }

    /// As [`compile`](Self::compile), taking newly made effects from
    /// `new_effects` by insert id. Effects already playing are taken over
    /// from the current graph when this one replaces it.
    pub fn compile_with(
        &self,
        max_block: usize,
        new_effects: &mut Vec<(u64, Box<dyn Insert>)>,
    ) -> Result<Graph, CompileError> {
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
                let start = c.start.frames(&self.tempo).0;
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
            let monitor = t.monitor && t.input.is_some();
            if monitor && t.input.is_some_and(|i| i.stereo) {
                stereo = true;
            }
            let inserts: Vec<InsertNode> = t
                .inserts
                .iter()
                .map(|i| InsertNode {
                    id: i.id,
                    bypass: i.bypass,
                    effect: new_effects
                        .iter()
                        .position(|(id, _)| *id == i.id)
                        .map(|k| new_effects.swap_remove(k).1),
                })
                .collect();
            let latency = t
                .inserts
                .iter()
                .filter(|i| !i.bypass)
                .map(|i| i.latency as i64)
                .sum();
            // Effects work in stereo, so a mono track becomes stereo.
            let widen = !stereo && !inserts.is_empty();
            stereo |= !inserts.is_empty();
            tracks.push(TrackNode {
                clips,
                mix: t.mix,
                last: (0.0, 0.0),
                stereo,
                input: t.input,
                arm: t.arm && t.input.is_some(),
                monitor,
                inserts,
                latency,
                widen,
                stretch: Box::new(Stretcher::new(max_block)),
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
            max_block,
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
    /// The largest block this graph was compiled for.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

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
    /// buffers, adds monitored input, applies the mixer, and adds the
    /// tracks into `out`. With `play` false (stopped, or counting in) only
    /// monitored input sounds. `looping` is the loop region while
    /// playing: effects' latency compensation reads ahead round it. Peak
    /// levels go to `peaks` (track by track, after the fader). Returns
    /// how many frames of streamed audio were missing.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        pos: i64,
        looping: Option<(i64, i64)>,
        out: [&mut [f32]; 2],
        peaks: &mut [(f32, f32)],
        input: InputBlock,
        play: bool,
        rate: Option<Rate>,
    ) -> usize {
        let n = out[0].len();
        let mut missing = 0;
        let [out_l, out_r] = out;
        for (ti, t) in self.tracks.iter_mut().enumerate() {
            let [sl, sr] = &mut self.scratch[ti];
            let (sl, sr) = (&mut sl[..n], &mut sr[..n]);
            sl.fill(0.0);
            sr.fill(0.0);
            if t.monitor
                && let Some(inp) = t.input
            {
                let right = if inp.stereo { inp.first + 1 } else { inp.first };
                for i in 0..n {
                    sl[i] += input.get(i, inp.first);
                    sr[i] += input.get(i, right);
                }
            }
            if play {
                match &rate {
                    Some(rate) => {
                        let st = &mut t.stretch;
                        st.set(rate.speed, rate.semitones);
                        if rate.reset {
                            // Effects' latency: read that far ahead, in
                            // input time.
                            st.reset(rate.u + t.latency as f64 * rate.speed);
                        }
                        let mut feed = TrackFeed {
                            clips: &t.clips,
                            streams: &self.streams,
                            looping,
                            missing: 0,
                        };
                        st.process(&mut feed, sl, sr);
                        missing += feed.missing;
                    }
                    // Read ahead by the effects' latency, round the loop
                    // when that runs past its end.
                    None => {
                        missing +=
                            read_looped(&t.clips, &self.streams, looping, pos + t.latency, sl, sr)
                    }
                }
            }

            if t.widen {
                sr.copy_from_slice(sl);
            }
            for i in &mut t.inserts {
                if let (false, Some(fx)) = (i.bypass, &mut i.effect) {
                    fx.process(sl, sr);
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

    /// Takes over the effects of `old` that this graph still uses. The
    /// rest stay in `old`, to be dropped with it off the audio thread.
    /// Takes over the stretchers of `old`, track by track, so a graph
    /// change while playing slowed does not restart them.
    pub(crate) fn adopt_stretchers(&mut self, old: &mut Graph) {
        for (t, o) in self.tracks.iter_mut().zip(&mut old.tracks) {
            std::mem::swap(&mut t.stretch, &mut o.stretch);
        }
    }

    pub(crate) fn adopt_inserts(&mut self, old: &mut Graph) {
        for t in &mut self.tracks {
            for slot in t.inserts.iter_mut().filter(|s| s.effect.is_none()) {
                slot.effect = old
                    .tracks
                    .iter_mut()
                    .flat_map(|o| o.inserts.iter_mut())
                    .find(|o| o.id == slot.id)
                    .and_then(|o| o.effect.take());
            }
        }
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
