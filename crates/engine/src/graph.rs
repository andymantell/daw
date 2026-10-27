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

/// A clip placed on a track, in musical time.
#[derive(Clone, Debug)]
pub struct ClipSpec {
    pub start: Ticks,
    pub data: Arc<AudioData>,
    /// Where in `data` playback begins, in frames.
    pub offset: usize,
    /// How much of `data` plays, in frames; `None` for the rest of it.
    pub len: Option<usize>,
    pub gain_db: f32,
}

impl ClipSpec {
    pub fn new(start: Ticks, data: Arc<AudioData>) -> ClipSpec {
        ClipSpec {
            start,
            data,
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
    pub data: Arc<AudioData>,
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

/// The compiled, immutable-shape graph the audio thread plays.
pub struct Graph {
    pub(crate) tempo: TempoMap,
    pub(crate) tracks: Vec<TrackNode>,
    pub(crate) master: f32,
    pub(crate) any_solo: bool,
    /// Per-track scratch, two channels each, `max_block` long.
    pub(crate) scratch: Vec<[Vec<f32>; 2]>,
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
        for (ti, t) in self.tracks.iter().enumerate() {
            let mut clips = Vec::with_capacity(t.clips.len());
            let mut stereo = false;
            for c in &t.clips {
                if c.data.rate != rate {
                    return Err(CompileError::RateMismatch { track: ti });
                }
                match c.data.channels.len() {
                    1 => {}
                    2 => stereo = true,
                    _ => return Err(CompileError::BadChannelCount { track: ti }),
                }
                let avail = c.data.len().saturating_sub(c.offset);
                let len = c.len.unwrap_or(avail).min(avail);
                if len == 0 {
                    continue;
                }
                let start = self.tempo.frames_at(c.start).0;
                clips.push(ClipNode {
                    start,
                    end: start + len as i64,
                    offset: c.offset,
                    gain: db_to_gain(c.gain_db),
                    data: c.data.clone(),
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
    pub(crate) fn render(&mut self, pos: i64, out: [&mut [f32]; 2], peaks: &mut [(f32, f32)]) {
        let n = out[0].len();
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
                let chans = &c.data.channels;
                let right = chans.get(1).unwrap_or(&chans[0]);
                for i in 0..len {
                    sl[dst + i] += chans[0][src + i] * c.gain;
                    sr[dst + i] += right[src + i] * c.gain;
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
    }
}
