// SPDX-License-Identifier: GPL-3.0-or-later
//! Chorus, flanger, phaser and tremolo.
//!
//! The chorus and flanger read a delay line at a time that a slow
//! oscillator moves, as described in J. Dattorro, "Effect Design, Part 2:
//! Delay-Line Modulation and Chorus", JAES 45(10), 1997, and in U. Zölzer
//! (ed.), "DAFX", chapter on delay-based effects. The phaser runs the
//! sound through a chain of first-order all-pass filters whose corner the
//! oscillator sweeps, and mixes it with the sound as it came in, so
//! notches move through it (the same "DAFX" chapter on filters). The
//! tremolo moves the level. The right side's oscillator can run ahead of
//! the left's, which spreads the effect across the stereo field.

use std::f64::consts::{PI, TAU};

use crate::{Dsp, Param, Unit, delay_read, flush};

/// What an oscillator's wave looks like.
pub const SHAPES: &[&str] = &["Smooth", "Triangle", "Square"];

/// A slow oscillator: a phase from 0 to 1 that turns at `hz`.
#[derive(Clone, Copy, Debug, Default)]
struct Lfo {
    phase: f64,
}

impl Lfo {
    /// Moves on one frame.
    #[inline]
    fn step(&mut self, hz: f64, rate: f64) {
        self.phase += hz / rate;
        self.phase -= self.phase.floor();
    }
}

/// The wave of `shape` at `phase` (any number; only its fraction
/// counts), from -1 to 1.
#[inline]
fn wave(shape: usize, phase: f64) -> f64 {
    let p = phase - phase.floor();
    match shape {
        1 => 1.0 - 4.0 * (p - 0.5).abs(),
        // A square with its corners rounded, so it doesn't click.
        2 => (6.0 * (TAU * p).sin()).tanh() / 6f64.tanh(),
        _ => (TAU * p).sin(),
    }
}

/// How far ahead the right side's oscillator runs at full spread: a
/// quarter turn.
const SPREAD: f64 = 0.25;

fn spread(percent: f64) -> f64 {
    percent / 100.0 * SPREAD
}

pub const CHORUS: &[Param] = &[
    Param::new("rate", "Speed", 0.05, 5.0, 0.8, Unit::Hz).log(),
    Param::new("depth", "Depth", 0.0, 100.0, 50.0, Unit::Percent),
    Param::new("delay", "Delay", 5.0, 30.0, 12.0, Unit::Ms),
    Param::new("voices", "Voices", 0.0, 2.0, 1.0, Unit::Choice).choices(&["1", "2", "3"]),
    Param::new("spread", "Stereo spread", 0.0, 100.0, 100.0, Unit::Percent),
    Param::new("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
];

/// The most the chorus's oscillator moves the delay either way, ms.
const CHORUS_SWING_MS: f64 = 5.0;

pub struct Chorus {
    rate: f64,
    lines: [Vec<f32>; 2],
    at: usize,
    lfo: Lfo,
    hz: f64,
    /// The delay and how far it swings, in frames.
    delay: f64,
    swing: f64,
    voices: usize,
    spread: f64,
    wet: f32,
}

impl Chorus {
    pub fn new(rate: f64) -> Chorus {
        let len = ((30.0 + CHORUS_SWING_MS) / 1000.0 * rate) as usize + 4;
        Chorus {
            rate,
            lines: [vec![0.0; len], vec![0.0; len]],
            at: 0,
            lfo: Lfo::default(),
            hz: 1.0,
            delay: 1.0,
            swing: 0.0,
            voices: 1,
            spread: 0.0,
            wet: 0.0,
        }
    }
}

impl Dsp for Chorus {
    fn update(&mut self, v: &[f64]) {
        self.hz = v[0];
        let ms = self.rate / 1000.0;
        self.delay = v[2] * ms;
        // Never so far that the delay would reach 1 ms or less.
        self.swing = (v[1] / 100.0 * CHORUS_SWING_MS).min(v[2] - 1.0) * ms;
        self.voices = v[3] as usize + 1;
        self.spread = spread(v[4]);
        self.wet = (v[5] / 100.0) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = self.lines[0].len();
        let n = self.voices;
        // Voices share the level, so more voices aren't louder.
        let share = 1.0 / n as f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.lines[0][self.at] = *l;
            self.lines[1][self.at] = *r;
            let mut out = [0f32; 2];
            for k in 0..n {
                let p = self.lfo.phase + k as f64 / n as f64;
                for (side, o) in out.iter_mut().enumerate() {
                    let p = p + side as f64 * self.spread;
                    let d = self.delay + self.swing * wave(0, p);
                    *o += delay_read(&self.lines[side], self.at, d) * share;
                }
            }
            self.at = (self.at + 1) % len;
            self.lfo.step(self.hz, self.rate);
            *l = *l * (1.0 - self.wet) + out[0] * self.wet;
            *r = *r * (1.0 - self.wet) + out[1] * self.wet;
        }
    }
}

pub const FLANGER: &[Param] = &[
    Param::new("rate", "Speed", 0.02, 5.0, 0.25, Unit::Hz).log(),
    Param::new("depth", "Depth", 0.0, 100.0, 80.0, Unit::Percent),
    Param::new("delay", "Delay", 0.5, 10.0, 3.0, Unit::Ms).log(),
    Param::new("feedback", "Feedback", -95.0, 95.0, 50.0, Unit::Percent),
    Param::new("spread", "Stereo spread", 0.0, 100.0, 50.0, Unit::Percent),
    Param::new("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
];

pub struct Flanger {
    rate: f64,
    lines: [Vec<f32>; 2],
    at: usize,
    lfo: Lfo,
    hz: f64,
    /// The longest delay, in frames, and the share of it the sweep
    /// takes off at its shortest.
    delay: f64,
    depth: f64,
    feedback: f32,
    spread: f64,
    wet: f32,
}

impl Flanger {
    pub fn new(rate: f64) -> Flanger {
        let len = (10.0 / 1000.0 * rate) as usize + 4;
        Flanger {
            rate,
            lines: [vec![0.0; len], vec![0.0; len]],
            at: 0,
            lfo: Lfo::default(),
            hz: 1.0,
            delay: 1.0,
            depth: 0.0,
            feedback: 0.0,
            spread: 0.0,
            wet: 0.0,
        }
    }
}

impl Dsp for Flanger {
    fn update(&mut self, v: &[f64]) {
        self.hz = v[0];
        self.depth = v[1] / 100.0;
        self.delay = v[2] / 1000.0 * self.rate;
        self.feedback = (v[3] / 100.0) as f32;
        self.spread = spread(v[4]);
        self.wet = (v[5] / 100.0) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = self.lines[0].len();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut out = [0f32; 2];
            for (side, (x, o)) in [*l, *r].into_iter().zip(&mut out).enumerate() {
                let p = self.lfo.phase + side as f64 * self.spread;
                // From the longest delay down to (1 - depth) of it and
                // back, along a triangle, so the sweep sounds even.
                let sweep = 0.5 + 0.5 * wave(1, p);
                let d = (self.delay * (1.0 - self.depth * sweep)).max(1.0);
                *o = delay_read(&self.lines[side], self.at, d);
                self.lines[side][self.at] = flush(x + *o * self.feedback);
            }
            self.at = (self.at + 1) % len;
            self.lfo.step(self.hz, self.rate);
            *l = *l * (1.0 - self.wet) + out[0] * self.wet;
            *r = *r * (1.0 - self.wet) + out[1] * self.wet;
        }
    }
}

const STAGES: &[&str] = &["2", "4", "6", "8", "12"];
const STAGE_COUNTS: [usize; 5] = [2, 4, 6, 8, 12];

pub const PHASER: &[Param] = &[
    Param::new("rate", "Speed", 0.02, 5.0, 0.4, Unit::Hz).log(),
    Param::new("depth", "Depth", 0.0, 100.0, 70.0, Unit::Percent),
    Param::new("centre", "Centre", 100.0, 4000.0, 700.0, Unit::Hz).log(),
    Param::new("stages", "Stages", 0.0, 4.0, 1.0, Unit::Choice).choices(STAGES),
    Param::new("feedback", "Feedback", 0.0, 90.0, 30.0, Unit::Percent),
    Param::new("spread", "Stereo spread", 0.0, 100.0, 50.0, Unit::Percent),
    Param::new("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
];

/// How many octaves either way the sweep goes at full depth.
const PHASER_OCTAVES: f64 = 2.0;

pub struct Phaser {
    rate: f64,
    lfo: Lfo,
    hz: f64,
    centre: f64,
    depth: f64,
    stages: usize,
    feedback: f32,
    spread: f64,
    wet: f32,
    /// Each all-pass's state, per side.
    state: [[f32; 12]; 2],
    /// What came out of the chain last, per side, for feedback.
    last: [f32; 2],
}

impl Phaser {
    pub fn new(rate: f64) -> Phaser {
        Phaser {
            rate,
            lfo: Lfo::default(),
            hz: 1.0,
            centre: 1000.0,
            depth: 0.0,
            stages: 4,
            feedback: 0.0,
            spread: 0.0,
            wet: 0.0,
            state: [[0.0; 12]; 2],
            last: [0.0; 2],
        }
    }
}

/// A first-order all-pass's coefficient for a corner at `hz`.
#[inline]
fn all_pass(hz: f64, rate: f64) -> f32 {
    let t = (PI * hz.clamp(20.0, rate * 0.45) / rate).tan();
    ((t - 1.0) / (t + 1.0)) as f32
}

impl Dsp for Phaser {
    fn update(&mut self, v: &[f64]) {
        self.hz = v[0];
        self.depth = v[1] / 100.0;
        self.centre = v[2];
        let stages = STAGE_COUNTS[(v[3] as usize).min(STAGE_COUNTS.len() - 1)];
        if stages != self.stages {
            // Stages that join start from silence.
            for s in &mut self.state {
                s[self.stages.min(stages)..].fill(0.0);
            }
            self.stages = stages;
        }
        self.feedback = (v[4] / 100.0) as f32;
        self.spread = spread(v[5]);
        self.wet = (v[6] / 100.0) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut out = [0f32; 2];
            for (side, (x, o)) in [*l, *r].into_iter().zip(&mut out).enumerate() {
                let p = self.lfo.phase + side as f64 * self.spread;
                let hz = self.centre * 2f64.powf(self.depth * PHASER_OCTAVES * wave(0, p));
                let a = all_pass(hz, self.rate);
                let mut y = x + self.last[side] * self.feedback;
                for s in &mut self.state[side][..self.stages] {
                    let next = a * y + *s;
                    *s = flush(y - a * next);
                    y = next;
                }
                self.last[side] = flush(y);
                *o = y;
            }
            self.lfo.step(self.hz, self.rate);
            *l = *l * (1.0 - self.wet) + out[0] * self.wet;
            *r = *r * (1.0 - self.wet) + out[1] * self.wet;
        }
    }
}

pub const TREMOLO: &[Param] = &[
    Param::new("rate", "Speed", 0.1, 20.0, 5.0, Unit::Hz).log(),
    Param::new("depth", "Depth", 0.0, 100.0, 50.0, Unit::Percent),
    Param::new("shape", "Shape", 0.0, 2.0, 0.0, Unit::Choice).choices(SHAPES),
    Param::new("stereo", "Stereo", 0.0, 1.0, 0.0, Unit::Choice)
        .choices(&["Both sides together", "Side to side"]),
];

pub struct Tremolo {
    rate: f64,
    lfo: Lfo,
    hz: f64,
    depth: f64,
    shape: usize,
    /// How far ahead the right side runs: none, or half a turn (which
    /// moves the sound from side to side).
    offset: f64,
}

impl Tremolo {
    pub fn new(rate: f64) -> Tremolo {
        Tremolo {
            rate,
            lfo: Lfo::default(),
            hz: 1.0,
            depth: 0.0,
            shape: 0,
            offset: 0.0,
        }
    }
}

impl Dsp for Tremolo {
    fn update(&mut self, v: &[f64]) {
        self.hz = v[0];
        self.depth = v[1] / 100.0;
        self.shape = v[2] as usize;
        self.offset = if v[3] >= 0.5 { 0.5 } else { 0.0 };
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // Full level at the wave's top, down by `depth` at its foot.
            let level = |p: f64| (1.0 - self.depth * (0.5 - 0.5 * wave(self.shape, p))) as f32;
            *l *= level(self.lfo.phase);
            *r *= level(self.lfo.phase + self.offset);
            self.lfo.step(self.hz, self.rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;

    const RATE: f64 = 48_000.0;

    fn sine(hz: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| (0.5 * (TAU * hz * n as f64 / RATE).sin()) as f32)
            .collect()
    }

    fn play(e: &Effect, l: &mut [f32], r: &mut [f32]) {
        let mut p = e.processor(RATE as u32);
        for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(l, r);
        }
    }

    #[test]
    fn waves_reach_both_ends() {
        for shape in 0..SHAPES.len() {
            let (lo, hi) = (0..1000)
                .map(|i| wave(shape, i as f64 / 1000.0))
                .fold((1f64, -1f64), |(lo, hi), x| (lo.min(x), hi.max(x)));
            assert!(lo < -0.99 && hi > 0.99, "{shape}: {lo} {hi}");
            assert!(lo >= -1.0 - 1e-9 && hi <= 1.0 + 1e-9);
        }
    }

    /// The tremolo dips by its depth once per cycle, and side to side
    /// dips the right while the left is up.
    #[test]
    fn tremolo_moves_the_level() {
        let e = Effect::new(Kind::Tremolo);
        e.set(0, 4.0);
        e.set(1, 100.0);
        e.set(3, 1.0);
        let mut l = vec![1f32; RATE as usize];
        let mut r = l.clone();
        play(&e, &mut l, &mut r);
        // A quarter of a 4 Hz cycle in, the left is at the top and the
        // right at the foot; a cycle later, the same.
        for at in [3000, 3000 + 12_000] {
            assert!(l[at] > 0.99, "{}", l[at]);
            assert!(r[at] < 0.01, "{}", r[at]);
        }
        let lo = l.iter().fold(1f32, |m, x| m.min(*x));
        assert!(lo < 0.01);
    }

    /// With no depth, the chorus is a steady delay, and the mix's dry
    /// half is the sound unchanged.
    #[test]
    fn chorus_without_depth_is_a_plain_delay() {
        let e = Effect::new(Kind::Chorus);
        e.set(1, 0.0);
        e.set(2, 10.0);
        e.set(3, 0.0);
        e.set(5, 100.0);
        let mut l = vec![0f32; 4800];
        l[0] = 1.0;
        let mut r = l.clone();
        play(&e, &mut l, &mut r);
        assert!((l[480] - 1.0).abs() < 1e-3, "{}", l[480]);
        assert!(l[..479].iter().all(|x| x.abs() < 1e-6));
        assert_eq!(l, r);
    }

    /// The chorus moves the delay, so a steady tone comes out wavering
    /// in pitch: its zero crossings drift against the input's.
    #[test]
    fn chorus_wavers() {
        let e = Effect::new(Kind::Chorus);
        e.set(0, 2.0);
        e.set(1, 100.0);
        e.set(3, 0.0);
        e.set(4, 0.0);
        e.set(5, 100.0);
        let input = sine(1000.0, 48_000);
        let (mut l, mut r) = (input.clone(), input.clone());
        play(&e, &mut l, &mut r);
        let crossings = |x: &[f32]| {
            x.windows(2)
                .enumerate()
                .filter(|(_, w)| w[0] <= 0.0 && w[1] > 0.0)
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        };
        let gaps: Vec<_> = crossings(&l[4800..])
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();
        let (lo, hi) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
        assert!(hi - lo >= 2, "periods {lo}..{hi}");
    }

    /// A flanger with no sweep cuts a notch where half a cycle fits the
    /// delay, and passes twice the delay's frequency.
    #[test]
    fn flanger_notches() {
        let e = Effect::new(Kind::Flanger);
        e.set(1, 0.0);
        e.set(2, 1.0);
        e.set(3, 0.0);
        e.set(5, 50.0);
        let level = |hz: f64| {
            let input = sine(hz, 9600);
            let (mut l, mut r) = (input.clone(), input);
            play(&e, &mut l, &mut r);
            l[4800..].iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        // 1 ms: a notch at 500 Hz, full level at 1 kHz.
        assert!(level(500.0) < 0.02, "{}", level(500.0));
        assert!((level(1000.0) - 0.5).abs() < 0.02, "{}", level(1000.0));
    }

    /// The phaser's chain passes every frequency at full level (it only
    /// shifts phase), and mixed half and half it notches some.
    #[test]
    fn phaser_chain_is_all_pass_and_mixes_into_notches() {
        let e = Effect::new(Kind::Phaser);
        e.set(1, 0.0);
        e.set(4, 0.0);
        e.set(6, 100.0);
        let level = |e: &Effect, hz: f64| {
            let input = sine(hz, 9600);
            let (mut l, mut r) = (input.clone(), input);
            play(e, &mut l, &mut r);
            l[4800..].iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        for hz in [100.0, 700.0, 5000.0] {
            assert!((level(&e, hz) - 0.5).abs() < 0.01, "{hz}");
        }
        // Two stages at 700 Hz: half a turn of phase there, which the
        // dry half cancels.
        e.set(3, 0.0);
        e.set(6, 50.0);
        assert!(level(&e, 700.0) < 0.02, "{}", level(&e, 700.0));
        assert!(level(&e, 50.0) > 0.4);
    }
}
