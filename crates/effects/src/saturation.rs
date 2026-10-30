// SPDX-License-Identifier: GPL-3.0-or-later
//! Saturation in up to four stages, each driven a little, which builds
//! up colour more smoothly than one stage driven hard. Each stage clips
//! softly in one of three characters (an even curve, a curve biased off
//! centre for even harmonics, and a firmer knee), with a blocker of the
//! offset that uneven clipping makes after it.
//!
//! The curves are the textbook ones (U. Zölzer (ed.), "DAFX", chapter on
//! nonlinear processing). As in the guitar amp, everything between the
//! input and the tone filter runs at four times the sample rate, so the
//! harmonics the clipping makes don't fold back as inharmonic whistles.
//! The level is matched so a sound at about -12 dB comes out near the
//! level it went in, whatever the drive and stages: turning the drive up
//! adds colour more than level.

use crate::biquad::{Biquad, Coefs};
use crate::{Dsp, Param, Unit, flush};

pub const CHARACTERS: &[&str] = &["Smooth", "Warm", "Firm"];

pub const PARAMS: &[Param] = &[
    Param::new("drive", "Drive", 0.0, 36.0, 12.0, Unit::Db),
    Param::new("stages", "Stages", 0.0, 3.0, 1.0, Unit::Choice).choices(&["1", "2", "3", "4"]),
    Param::new(
        "character",
        "Character",
        0.0,
        (CHARACTERS.len() - 1) as f64,
        1.0,
        Unit::Choice,
    )
    .choices(CHARACTERS),
    Param::new("tone", "Tone", 1000.0, 20000.0, 20000.0, Unit::Hz)
        .log()
        .off_at(20000.0),
    Param::new("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

const OVER: usize = 4;
const MAX_STAGES: usize = 4;
/// The level, as a gain, that comes out where it went in.
const MATCH: f64 = 0.25;

/// One stage's curve, passing silence as silence.
#[inline]
fn curve(character: usize, x: f64) -> f64 {
    match character {
        // Off centre: the two halves of a wave clip differently.
        1 => (x + 0.35).tanh() - 0.35f64.tanh(),
        // A firmer knee: straight until near the top, then flat.
        2 => {
            let x = x.clamp(-1.5, 1.5);
            x - x * x * x * 4.0 / 27.0
        }
        _ => x.tanh(),
    }
}

/// One side.
#[derive(Clone, Copy, Default)]
struct Side {
    up: [Biquad; 4],
    down: [Biquad; 4],
    /// After each stage, at the oversampled rate.
    dc: [Biquad; MAX_STAGES],
    tone: Biquad,
}

pub struct Saturation {
    rate: f64,
    sides: [Side; 2],
    stages: usize,
    character: usize,
    /// Each stage's gain in, and what brings its level back.
    drive: f64,
    trim: f64,
    out: f64,
}

impl Saturation {
    pub fn new(rate: f64) -> Saturation {
        let over = rate * OVER as f64;
        // An eighth-order Butterworth low pass at the oversampled rate,
        // as the amp has: its four sections' Qs.
        let qs = [0.5098, 0.6013, 0.9000, 2.5629];
        let edge = (rate * 0.45).min(20_000.0);
        let lp = |q: f64| Biquad::new(Coefs::low_pass(edge, q, over));
        let dc = Biquad::new(Coefs::high_pass(
            10.0,
            std::f64::consts::FRAC_1_SQRT_2,
            over,
        ));
        let side = Side {
            up: qs.map(lp),
            down: qs.map(lp),
            dc: [dc; MAX_STAGES],
            tone: Biquad::default(),
        };
        Saturation {
            rate,
            sides: [side; 2],
            stages: 1,
            character: 0,
            drive: 1.0,
            trim: 1.0,
            out: 1.0,
        }
    }
}

impl Dsp for Saturation {
    fn update(&mut self, v: &[f64]) {
        self.stages = (v[1] as usize + 1).min(MAX_STAGES);
        self.character = (v[2] as usize).min(CHARACTERS.len() - 1);
        self.drive = crate::gain(v[0] / self.stages as f64);
        // What one stage does to the matching level, undone: from its
        // top to its foot, as a curve off centre clips them differently.
        let m = MATCH * self.drive;
        let slope = (curve(self.character, m) - curve(self.character, -m)) / (2.0 * MATCH);
        self.trim = 1.0 / slope;
        let c = if PARAMS[3].off_at == Some(v[3]) {
            Coefs::THROUGH
        } else {
            Coefs::low_pass(v[3], std::f64::consts::FRAC_1_SQRT_2, self.rate)
        };
        for s in &mut self.sides {
            s.tone.c = c;
        }
        self.out = crate::gain(v[4]);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (i, s) in self.sides.iter_mut().enumerate() {
            let buf = if i == 0 { &mut *left } else { &mut *right };
            for x in buf.iter_mut() {
                let mut y = 0.0;
                for k in 0..OVER {
                    let u = if k == 0 { *x * OVER as f32 } else { 0.0 };
                    let mut u = s.up.iter_mut().fold(u, |u, f| f.run(u)) as f64;
                    for dc in &mut s.dc[..self.stages] {
                        u = curve(self.character, u * self.drive) * self.trim;
                        u = dc.run(u as f32) as f64;
                    }
                    let d = s.down.iter_mut().fold(u as f32, |d, f| f.run(d));
                    if k == 0 {
                        y = d;
                    }
                }
                *x = flush((s.tone.run(y) as f64 * self.out) as f32);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;
    use std::f64::consts::TAU;

    const RATE: f64 = 48_000.0;

    fn play(e: &Effect, input: &[f32]) -> Vec<f32> {
        let mut p = e.processor(RATE as u32);
        let (mut l, mut r) = (input.to_vec(), input.to_vec());
        for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(l, r);
        }
        assert_eq!(l, r);
        l
    }

    fn sine(hz: f64, level: f64) -> Vec<f32> {
        (0..RATE as usize)
            .map(|n| (level * (TAU * hz * n as f64 / RATE).sin()) as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// The share of a tone's energy that isn't the tone.
    fn distortion(out: &[f32], hz: f64) -> f64 {
        let x = &out[RATE as usize / 2..];
        let (mut re, mut im) = (0.0, 0.0);
        for (n, v) in x.iter().enumerate() {
            let w = TAU * hz * n as f64 / RATE;
            re += *v as f64 * w.cos();
            im += *v as f64 * w.sin();
        }
        let tone = 2.0 * (re * re + im * im) / (x.len() as f64).powi(2);
        let all = rms(x).powi(2);
        ((all - tone) / all).max(0.0).sqrt()
    }

    /// Every character, stage count and drive keeps a -12 dB tone within
    /// a few dB of its level, and more drive adds grit. The same drive
    /// over more stages is smoother than in one, which is the point.
    #[test]
    fn level_holds_and_drive_adds_grit() {
        let input = sine(220.0, MATCH * std::f64::consts::SQRT_2);
        let rin = crate::db(rms(&input));
        for c in 0..CHARACTERS.len() {
            for stages in [0, 3] {
                let mut last = 0.0;
                for drive in [6.0, 18.0, 36.0] {
                    let e = Effect::new(Kind::Saturation);
                    e.set(0, drive);
                    e.set(1, stages as f64);
                    e.set(2, c as f64);
                    let out = play(&e, &input);
                    let rout = crate::db(rms(&out[RATE as usize / 2..]));
                    assert!((rout - rin).abs() < 4.0, "{c} {stages} {drive}: {rout:.1}");
                    let d = distortion(&out, 220.0);
                    assert!(d > last, "{c} {stages} {drive}: {d} after {last}");
                    last = d;
                }
                assert!(last > 0.1, "{c} {stages}: {last}");
            }
        }
    }

    /// With no drive, it's nearly clean.
    #[test]
    fn no_drive_is_clean() {
        let e = Effect::new(Kind::Saturation);
        e.set(0, 0.0);
        e.set(1, 0.0);
        e.set(2, 0.0);
        let out = play(&e, &sine(220.0, 0.1));
        assert!(distortion(&out, 220.0) < 0.01);
    }

    /// A high tone driven hard makes harmonics above the top of the
    /// band; oversampling keeps them from folding back below it.
    #[test]
    fn harmonics_dont_fold_back() {
        let e = Effect::new(Kind::Saturation);
        e.set(0, 36.0);
        e.set(1, 3.0);
        e.set(2, 2.0);
        let hz = 5000.0;
        let out = play(&e, &sine(hz, 0.5));
        // Energy between the tone's own harmonics: at 1 kHz steps that
        // aren't multiples of 5 kHz, say 2.5 kHz and 7.5 kHz.
        let at = |f: f64| {
            let x = &out[RATE as usize / 2..];
            let (mut re, mut im) = (0.0, 0.0);
            for (n, v) in x.iter().enumerate() {
                let w = TAU * f * n as f64 / RATE;
                re += *v as f64 * w.cos();
                im += *v as f64 * w.sin();
            }
            crate::db(2.0 * (re * re + im * im).sqrt() / x.len() as f64)
        };
        let tone = at(hz);
        for f in [1000.0, 3000.0, 7000.0, 9000.0, 11000.0] {
            assert!(at(f) < tone - 50.0, "{f}: {:.1} dB vs {tone:.1}", at(f));
        }
    }
}
