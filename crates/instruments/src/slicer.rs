// SPDX-License-Identifier: GPL-3.0-or-later
//! A loop slicer (roadmap 3.4): cuts a loop where each hit starts, or
//! into equal parts, and puts each slice on a key from C2 (note 36)
//! up. Played in order, at the beats they came from, the slices play
//! the loop back at whatever tempo the song is at, without stretching
//! the sound; played in another order they make a new part. Each slice
//! has its own tuning, pan and low-pass filter, and can play backwards.

use std::sync::Arc;

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Adsr, Env, Mode, SVF_ZERO, Svf, db};
use crate::params::{Param, Params, Unit};
use crate::sample::{Player, Sample};

pub const MODES: &[&str] = &["Play the slice", "Stop with the note"];
/// The key of the first slice.
pub const FIRST_NOTE: u8 = 36;
/// The most slices a loop is cut into.
pub const MAX_SLICES: usize = 64;

pub mod at {
    pub const MODE: usize = 0;
    pub const TUNE: usize = 1;
    pub const ATTACK: usize = 2;
    pub const RELEASE: usize = 3;
    pub const VELOCITY: usize = 4;
    pub const LEVEL: usize = 5;
    /// The loop's length in beats, for laying its slices out in time.
    pub const BEATS: usize = 6;
    /// Where the slices' settings begin; [`SLICE`](super::SLICE) each.
    pub const SLICES: usize = 7;
}

/// A slice's settings, from its first.
pub mod slice {
    /// Where it starts, in frames of the loop as read.
    pub const START: usize = 0;
    pub const TUNE: usize = 1;
    pub const PAN: usize = 2;
    pub const REVERSE: usize = 3;
    pub const CUTOFF: usize = 4;
}

pub const SLICE: usize = 5;

/// The index of slice `i`'s setting `k`.
pub fn slice_at(i: usize, k: usize) -> usize {
    at::SLICES + i * SLICE + k
}

pub fn params(slices: usize) -> Vec<Param> {
    use Unit::*;
    let mut v = vec![
        Param::new("mode", "Notes", 0.0, 1.0, 0.0, Choice).choices(MODES),
        Param::new("tune", "Tune", -24.0, 24.0, 0.0, Semitones),
        Param::new("attack", "Attack", 0.0005, 1.0, 0.0005, Seconds).log(),
        Param::new("release", "Release", 0.005, 2.0, 0.02, Seconds).log(),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.5, Amount),
        Param::new("level", "Level", -36.0, 12.0, 0.0, Db),
        Param::new("beats", "Loop length in beats", 1.0, 64.0, 4.0, Count),
    ];
    for i in 0..slices.min(MAX_SLICES) {
        let k = |s: &str| format!("slice{i:02}.{s}");
        v.extend([
            Param::new(
                format!("slice{i:02}"),
                format!("Slice {}", i + 1),
                0.0,
                f32::MAX as f64,
                0.0,
                Hidden,
            ),
            Param::new(k("tune"), "Tune", -24.0, 24.0, 0.0, Semitones),
            Param::new(k("pan"), "Pan", -1.0, 1.0, 0.0, Pan),
            Param::new(k("reverse"), "Backwards", 0.0, 1.0, 0.0, Switch),
            Param::new(k("cutoff"), "Low-pass", 30.0, 20_000.0, 20_000.0, Hz).log(),
        ]);
    }
    v
}

/// The slices' starts as `params` hold them.
pub fn slices_in(p: &Params) -> Vec<usize> {
    (0..p.len().saturating_sub(at::SLICES) / SLICE)
        .map(|i| p.get(slice_at(i, slice::START)).max(0.0) as usize)
        .collect()
}

/// Where each hit starts in `s`, in frames: a slice at 0, and one where
/// the level jumps up. `sensitivity` from 0 (only the clearest hits) to
/// 1 (every small one). Found by comparing each short window's energy
/// with the windows before it, the energy-rise way of finding onsets
/// (Bello et al., "A Tutorial on Onset Detection in Music Signals",
/// 2005), then moved back to where the change first gets big.
pub fn find_hits(s: &Sample, sensitivity: f32) -> Vec<usize> {
    const HOP: usize = 256;
    let n = s.len();
    let windows = n / HOP;
    // The energy of the change from one frame to the next: a hit's
    // sharp start stands out from the ring of the one before it.
    let mono = |i: usize| 0.5 * (s.left[i] + s.right[i]);
    let energy: Vec<f32> = (0..windows)
        .map(|w| {
            (w * HOP..(w + 1) * HOP)
                .map(|i| {
                    let d = mono(i) - if i > 0 { mono(i - 1) } else { 0.0 };
                    d * d
                })
                .sum::<f32>()
                / HOP as f32
        })
        .collect();
    let loudest = energy.iter().fold(0.0f32, |m, &e| m.max(e));
    let ratio = 1.5 + 6.0 * (1.0 - sensitivity.clamp(0.0, 1.0));
    let floor = loudest * (0.002 + 0.05 * (1.0 - sensitivity.clamp(0.0, 1.0)));
    let gap = (s.rate as usize / 20).div_ceil(HOP).max(1);
    let mut out = vec![0usize];
    let mut last: Option<usize> = None;
    for w in 1..windows {
        let before = energy[w.saturating_sub(4)..w]
            .iter()
            .fold(0.0f32, |m, &e| m.max(e));
        let apart = last.is_none_or(|l| w - l >= gap);
        if energy[w] > floor && energy[w] > before * ratio && apart {
            // Back to where the change first gets big.
            let from = ((w - 1) * HOP).max(1);
            let to = ((w + 1) * HOP).min(n);
            let diff = |i: usize| (mono(i) - mono(i - 1)).abs();
            let big = (from..to).map(diff).fold(0.0f32, f32::max) * 0.2;
            let at = (from..to).find(|&i| diff(i) >= big).unwrap_or(from);
            last = Some(w);
            if at > *out.last().unwrap_or(&0) + HOP && out.len() < MAX_SLICES {
                out.push(at);
            }
        }
    }
    out
}

/// `count` equal slices of a sound `len` frames long.
pub fn equal(len: usize, count: usize) -> Vec<usize> {
    let c = count.clamp(1, MAX_SLICES);
    (0..c).map(|i| len * i / c).collect()
}

/// A loop's likely length in beats: a power of two that puts its tempo
/// between 80 and 160 bpm.
pub fn guess_beats(seconds: f64) -> f64 {
    let mut beats = 1.0;
    while beats < 64.0 && 60.0 * beats / seconds < 80.0 {
        beats *= 2.0;
    }
    beats
}

/// The notes that play the slices in order: (start, length, key), in
/// ticks at `ppq` beats, for a loop of `beats` beats `len` frames long.
pub fn notes(slices: &[usize], len: usize, beats: f64, ppq: i64) -> Vec<(i64, i64, u8)> {
    let tick = |f: usize| (f as f64 / len.max(1) as f64 * beats * ppq as f64).round() as i64;
    (slices.iter().enumerate())
        .take(MAX_SLICES)
        .map(|(i, &s)| {
            let end = slices.get(i + 1).copied().unwrap_or(len);
            let (a, b) = (tick(s), tick(end));
            (a, (b - a).max(1), FIRST_NOTE + i as u8)
        })
        .filter(|n| (n.2 as usize) < 128)
        .collect()
}

const VOICES: usize = 16;
const CHUNK: usize = 32;

#[derive(Clone, Copy)]
struct Voice {
    on: bool,
    note: u8,
    slice: usize,
    held: bool,
    player: Player,
    env: Env,
    gain: (f32, f32),
    filter: [Svf; 2],
    age: u64,
}

pub struct Slicer {
    params: Arc<Params>,
    sample: Option<Arc<Sample>>,
    /// The slices' starts in frames of `sample`.
    slices: Vec<usize>,
    rate: f32,
    voices: [Voice; VOICES],
    clock: u64,
}

impl Slicer {
    /// `scale` turns the slices' frames (as `params` hold them) into
    /// frames of `sample`, which may be at another rate.
    pub fn new(rate: u32, params: Arc<Params>, sample: Option<Arc<Sample>>, scale: f64) -> Slicer {
        let slices = slices_in(&params)
            .into_iter()
            .map(|f| (f as f64 * scale) as usize)
            .collect();
        Slicer {
            params,
            sample,
            slices,
            rate: rate.max(1) as f32,
            voices: [Voice {
                on: false,
                note: 0,
                slice: 0,
                held: false,
                player: Player::default(),
                env: Env::IDLE,
                gain: (0.0, 0.0),
                filter: [SVF_ZERO; 2],
                age: 0,
            }; VOICES],
            clock: 0,
        }
    }

    fn hit(&mut self, note: u8, velocity: u8) {
        let Some(s) = &self.sample else { return };
        let Some(i) = (note as usize).checked_sub(FIRST_NOTE as usize) else {
            return;
        };
        let Some(&start) = self.slices.get(i) else {
            return;
        };
        let end = self
            .slices
            .get(i + 1)
            .copied()
            .unwrap_or(s.len())
            .min(s.len());
        let p = &self.params;
        // The same slice again cuts the last one.
        for v in self.voices.iter_mut().filter(|v| v.on && v.note == note) {
            v.env.release();
        }
        let semis = p.read(at::TUNE) + p.read(slice_at(i, slice::TUNE));
        let step = (semis as f64 / 12.0).exp2();
        let sens = p.read(at::VELOCITY);
        let g = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
        let (pl, pr) = crate::dsp::pan(p.read(slice_at(i, slice::PAN)));
        let (a, b) = if p.read(slice_at(i, slice::REVERSE)) >= 0.5 {
            (end.saturating_sub(1) as f64, start as f64)
        } else {
            (start as f64, end as f64)
        };
        self.clock += 1;
        let slot = (self.voices.iter().position(|v| !v.on))
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap_or(0));
        let mut env = Env::IDLE;
        env.start();
        self.voices[slot] = Voice {
            on: true,
            note,
            slice: i,
            held: true,
            player: Player::new(a, b, step),
            env,
            gain: (g * pl, g * pr),
            filter: [SVF_ZERO; 2],
            age: self.clock,
        };
    }

    fn event(&mut self, e: &Event) {
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.hit(pitch, velocity),
            Kind::NoteOff { pitch, .. } => {
                let gate = self.params.read(at::MODE) >= 0.5;
                for v in self
                    .voices
                    .iter_mut()
                    .filter(|v| v.on && v.held && v.note == pitch)
                {
                    v.held = false;
                    if gate {
                        v.env.release();
                    }
                }
            }
            Kind::AllNotesOff | Kind::Control { number: 120, .. } => {
                for v in &mut self.voices {
                    v.env.release();
                }
            }
            _ => {}
        }
    }

    fn run(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let Some(s) = self.sample.clone() else { return };
        let p = &self.params;
        let adsr = Adsr::new(
            p.read(at::ATTACK),
            30.0,
            1.0,
            p.read(at::RELEASE),
            self.rate,
        );
        let gain = db(p.read(at::LEVEL));
        let mut at = from;
        while at < to {
            let end = (at + CHUNK).min(to);
            for v in self.voices.iter_mut().filter(|v| v.on) {
                let cutoff = p.read(slice_at(v.slice, slice::CUTOFF));
                let filtering = cutoff < 19_999.0;
                if filtering {
                    for f in &mut v.filter {
                        f.set(cutoff, 0.0, self.rate);
                    }
                }
                for i in at..end {
                    let e = v.env.next(&adsr);
                    if !v.env.active() || v.player.done {
                        v.on = false;
                        break;
                    }
                    let (mut a, mut b) = v.player.next(&s);
                    if filtering {
                        a = v.filter[0].run(a, Mode::Low);
                        b = v.filter[1].run(b, Mode::Low);
                    }
                    let g = e * gain;
                    left[i] += a * g * v.gain.0;
                    right[i] += b * g * v.gain.1;
                }
            }
            at = end;
        }
    }
}

impl Instrument for Slicer {
    fn process(&mut self, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut at = 0;
        for e in events {
            let f = (e.frame as usize).min(n);
            self.run(at, f, left, right);
            at = f;
            self.event(e);
        }
        self.run(at, n, left, right);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four clicks a quarter of a second apart, each a decaying burst.
    fn clicks() -> Sample {
        let rate = 48_000;
        let mut x = vec![0.0f32; rate as usize];
        for k in 0..4 {
            let at = 1000 + k * 12_000;
            for i in 0..3000 {
                x[at + i] = (-(i as f32) / 600.0).exp() * if i % 2 == 0 { 0.8 } else { -0.8 };
            }
        }
        Sample::mono(x, rate)
    }

    #[test]
    fn hits_are_found_where_they_start() {
        let s = clicks();
        let hits = find_hits(&s, 0.5);
        assert_eq!(hits.len(), 5, "{hits:?}");
        assert_eq!(hits[0], 0);
        for (k, h) in hits[1..].iter().enumerate() {
            let want = 1000 + k * 12_000;
            assert!(h.abs_diff(want) <= 256, "{h} for {want}");
        }
        assert_eq!(equal(1000, 4), [0, 250, 500, 750]);
    }

    #[test]
    fn notes_lay_the_slices_out_on_their_beats() {
        assert_eq!(guess_beats(2.0), 4.0);
        assert_eq!(guess_beats(0.5), 1.0);
        assert_eq!(guess_beats(8.0), 16.0);
        let n = notes(&[0, 250, 500, 750], 1000, 4.0, 960);
        assert_eq!(
            n,
            [
                (0, 960, 36),
                (960, 960, 37),
                (1920, 960, 38),
                (2880, 960, 39)
            ]
        );
    }

    #[test]
    fn each_key_plays_its_slice_and_stops_at_its_end() {
        let s = Arc::new(clicks());
        let p = Arc::new(Params::new(params(5)));
        for (i, f) in find_hits(&s, 0.5).into_iter().enumerate() {
            p.set(slice_at(i, slice::START), f as f64);
        }
        let mut sl = Slicer::new(48_000, p, Some(s.clone()), 1.0);
        let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
        // The third slice: the second click, which starts at once.
        sl.process(
            &[Event::note_on(0, 0, FIRST_NOTE + 2, 127, 0)],
            &mut l,
            &mut r,
        );
        let first_loud = l.iter().position(|v| v.abs() > 0.3).unwrap();
        assert!(first_loud < 300, "{first_loud}");
        // It ends where the next slice begins, about 12000 frames on.
        assert!(l[12_500..].iter().all(|v| v.abs() < 1e-3));
        assert!(sl.voices.iter().all(|v| !v.on));
    }

    #[test]
    fn a_slice_plays_backwards_and_panned_on_its_own() {
        // A rising ramp, one slice.
        let s = Arc::new(Sample::mono(
            (0..4800).map(|i| i as f32 / 4800.0).collect(),
            48_000,
        ));
        let p = Arc::new(Params::new(params(1)));
        p.set(at::VELOCITY, 0.0);
        p.set(slice_at(0, slice::REVERSE), 1.0);
        p.set(slice_at(0, slice::PAN), -1.0);
        let mut sl = Slicer::new(48_000, p, Some(s), 1.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        sl.process(&[Event::note_on(0, 0, FIRST_NOTE, 127, 0)], &mut l, &mut r);
        assert!(l[200] > l[3000], "backwards it starts high");
        assert!(r.iter().all(|v| v.abs() < 1e-6), "all to the left");
    }
}
