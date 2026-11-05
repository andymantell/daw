// SPDX-License-Identifier: GPL-3.0-or-later
//! A resonant filter over the whole range, 20 Hz to 20 kHz: low-pass,
//! high-pass, band-pass or notch, at 12 or 24 dB an octave, for filter
//! sweeps and for thinning a part out.
//!
//! Each stage is a state-variable filter discretised with the
//! trapezoidal rule and a prewarped cutoff (the "topology-preserving
//! transform" from Zavalishin, *The Art of VA Filter Design*, 2012,
//! chapter 4). Its state is kept as the integrators' outputs, so it
//! stays stable and quiet while the cutoff moves, as it does under
//! automation. The 24 dB slope runs two stages one after the other; the
//! resonance is on the first, the second is flat (Butterworth) for
//! low- and high-pass, so the peak stays where the 12 dB one has it.

use crate::{Dsp, Param, Unit};

const MODES: &[&str] = &["Low-pass", "High-pass", "Band-pass", "Notch"];
const SLOPES: &[&str] = &["12 dB/oct", "24 dB/oct"];

pub const PARAMS: &[Param] = &[
    Param::new("mode", "Type", 0.0, 3.0, 0.0, Unit::Choice).choices(MODES),
    Param::new("cutoff", "Cutoff", 20.0, 20_000.0, 1000.0, Unit::Hz).log(),
    Param::new("resonance", "Resonance", 0.0, 100.0, 20.0, Unit::Percent),
    Param::new("slope", "Slope", 0.0, 1.0, 1.0, Unit::Choice).choices(SLOPES),
    Param::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

/// The Q with no resonance (Butterworth), and how many times that the
/// most resonance gives.
const FLAT_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
const MOST_Q: f64 = 17.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Low,
    High,
    Band,
    Notch,
}

/// One stage's coefficients.
#[derive(Clone, Copy, Debug, Default)]
struct Coefs {
    /// 1 / Q.
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
}

impl Coefs {
    fn new(hz: f64, q: f64, rate: f64) -> Coefs {
        // Below the top of the band, where the prewarp holds.
        let hz = hz.clamp(1.0, rate * 0.49);
        let g = (std::f64::consts::PI * hz / rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Coefs {
            k,
            a1,
            a2,
            a3: g * a2,
        }
    }
}

/// One stage's state, for one side.
#[derive(Clone, Copy, Debug, Default)]
struct Stage {
    ic1: f64,
    ic2: f64,
}

impl Stage {
    #[inline]
    fn run(&mut self, c: &Coefs, mode: Mode, x: f64) -> f64 {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        let (low, band) = (v2, v1);
        let high = x - c.k * band - low;
        match mode {
            Mode::Low => low,
            Mode::High => high,
            // Scaled by k so the peak is at 0 dB whatever the Q.
            Mode::Band => c.k * band,
            Mode::Notch => low + high,
        }
    }
}

pub struct Filter {
    rate: f64,
    mode: Mode,
    two: bool,
    first: Coefs,
    second: Coefs,
    /// Per side, the two stages.
    stages: [[Stage; 2]; 2],
    wet: f64,
}

impl Filter {
    pub fn new(rate: f64) -> Filter {
        Filter {
            rate,
            mode: Mode::Low,
            two: true,
            first: Coefs::default(),
            second: Coefs::default(),
            stages: [[Stage::default(); 2]; 2],
            wet: 1.0,
        }
    }
}

/// The Q for a resonance from 0 to 100%.
fn q_of(resonance: f64) -> f64 {
    FLAT_Q * MOST_Q.powf((resonance / 100.0).clamp(0.0, 1.0))
}

/// The filter's gain at `hz` with settings `v`, in dB: what its panel
/// draws (the analogue response the filter is made to follow).
pub(crate) fn response_db(v: &[f64], hz: f64) -> f64 {
    // Complex numbers as pairs.
    fn mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
        (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
    }
    fn div(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    }
    let mode = v[0].round() as i64;
    let w = hz / v[1].max(1.0);
    let stage = |q: f64| {
        let k = 1.0 / q;
        let den = (1.0 - w * w, k * w);
        let num = match mode {
            1 => (-w * w, 0.0),
            2 => (0.0, k * w),
            3 => (1.0 - w * w, 0.0),
            _ => (1.0, 0.0),
        };
        div(num, den)
    };
    let q = q_of(v[2]);
    let mut h = stage(q);
    if v[3] >= 0.5 {
        let q2 = if mode <= 1 { FLAT_Q } else { q };
        h = mul(h, stage(q2));
    }
    let wet = v[4] / 100.0;
    let y = (1.0 + (h.0 - 1.0) * wet, h.1 * wet);
    20.0 * y.0.hypot(y.1).max(1e-6).log10()
}

impl Dsp for Filter {
    fn update(&mut self, v: &[f64]) {
        let mode = match v[0].round() as i64 {
            1 => Mode::High,
            2 => Mode::Band,
            3 => Mode::Notch,
            _ => Mode::Low,
        };
        if mode != self.mode {
            self.stages = Default::default();
        }
        self.mode = mode;
        let q = q_of(v[2]);
        self.first = Coefs::new(v[1], q, self.rate);
        let two = v[3] >= 0.5;
        if two && !self.two {
            // The second stage starts from silence; clear it so it
            // doesn't start from what it held when last used.
            for s in &mut self.stages {
                s[1] = Stage::default();
            }
        }
        self.two = two;
        let q2 = match mode {
            Mode::Low | Mode::High => FLAT_Q,
            Mode::Band | Mode::Notch => q,
        };
        self.second = Coefs::new(v[1], q2, self.rate);
        self.wet = v[4] / 100.0;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (mode, two, wet) = (self.mode, self.two, self.wet);
        let (c1, c2) = (self.first, self.second);
        for (side, buf) in [left, right].into_iter().enumerate() {
            let [a, b] = &mut self.stages[side];
            for x in buf.iter_mut() {
                let dry = *x as f64;
                let mut y = a.run(&c1, mode, dry);
                if two {
                    y = b.run(&c2, mode, y);
                }
                *x = (dry + (y - dry) * wet) as f32;
            }
        }
        // Keep tiny tails from going denormal.
        for s in self.stages.iter_mut().flatten() {
            for v in [&mut s.ic1, &mut s.ic2] {
                if v.abs() < 1e-20 {
                    *v = 0.0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;
    use std::f64::consts::TAU;

    const RATE: u32 = 48_000;

    fn with(values: &[(&str, f64)]) -> Effect {
        let e = Effect::new(Kind::Filter);
        for (k, v) in values {
            let i = Kind::Filter
                .params()
                .iter()
                .position(|p| p.key == *k)
                .unwrap();
            e.set(i, *v);
        }
        e
    }

    /// The level of a steady tone at `hz` through `e`, in dB against
    /// the tone going in.
    fn response(e: &Effect, hz: f64) -> f64 {
        let n = 24_000;
        let src: Vec<f32> = (0..n)
            .map(|i| (0.25 * (TAU * hz * i as f64 / RATE as f64).sin()) as f32)
            .collect();
        let (mut l, mut r) = (src.clone(), src.clone());
        let mut p = e.processor(RATE);
        for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(l, r);
        }
        let rms = |x: &[f32]| {
            (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
        };
        // After it has settled.
        20.0 * (rms(&l[n / 2..]) / rms(&src[n / 2..])).max(1e-10).log10()
    }

    #[test]
    fn low_pass_keeps_the_lows_and_cuts_the_highs() {
        let e = with(&[("mode", 0.0), ("cutoff", 1000.0), ("resonance", 0.0)]);
        assert!(response(&e, 100.0).abs() < 0.5);
        // About -3 dB a stage at the cutoff, -6 for two.
        assert!((response(&e, 1000.0) + 6.0).abs() < 1.0);
        // 24 dB an octave: two octaves up is about 48 dB down.
        assert!(response(&e, 4000.0) < -40.0);
    }

    #[test]
    fn twelve_db_falls_half_as_fast() {
        let e = with(&[("cutoff", 1000.0), ("resonance", 0.0), ("slope", 0.0)]);
        let at = response(&e, 4000.0);
        assert!((-28.0..-20.0).contains(&at), "{at}");
    }

    #[test]
    fn high_pass_keeps_the_highs() {
        let e = with(&[("mode", 1.0), ("cutoff", 300.0), ("resonance", 0.0)]);
        assert!(response(&e, 5000.0).abs() < 0.5);
        assert!(response(&e, 50.0) < -40.0);
    }

    #[test]
    fn band_pass_peaks_at_the_cutoff() {
        let e = with(&[("mode", 2.0), ("cutoff", 1000.0), ("resonance", 50.0)]);
        assert!(response(&e, 1000.0).abs() < 1.0);
        assert!(response(&e, 250.0) < -20.0);
        assert!(response(&e, 4000.0) < -20.0);
    }

    #[test]
    fn notch_takes_out_the_cutoff() {
        let e = with(&[("mode", 3.0), ("cutoff", 1000.0), ("slope", 0.0)]);
        assert!(response(&e, 1000.0) < -30.0);
        assert!(response(&e, 100.0).abs() < 1.0);
    }

    #[test]
    fn resonance_lifts_the_cutoff() {
        let flat = with(&[("cutoff", 1000.0), ("resonance", 0.0)]);
        let peaky = with(&[("cutoff", 1000.0), ("resonance", 80.0)]);
        assert!(response(&peaky, 1000.0) > response(&flat, 1000.0) + 12.0);
    }

    #[test]
    fn it_reaches_both_ends_of_the_range() {
        // A dark 30 Hz low-pass takes most of a 300 Hz tone away, and
        // wide open at 20 kHz leaves 10 kHz alone.
        let dark = with(&[("cutoff", 30.0), ("resonance", 0.0)]);
        assert!(response(&dark, 300.0) < -60.0);
        let open = with(&[("cutoff", 20_000.0), ("resonance", 0.0)]);
        assert!(response(&open, 10_000.0).abs() < 1.0);
    }

    #[test]
    fn mix_at_nothing_is_the_sound_unchanged() {
        let e = with(&[("cutoff", 100.0), ("mix", 0.0)]);
        assert!(response(&e, 5000.0).abs() < 0.01);
    }

    /// Sweeping the cutoff block by block, as a lane does, stays finite
    /// and bounded, even with the most resonance.
    #[test]
    fn a_fast_sweep_stays_stable() {
        let e = with(&[("resonance", 100.0)]);
        let mut p = e.processor(RATE);
        let cutoff = 1u32;
        let mut most = 0f32;
        for b in 0..400 {
            let hz = 20.0 * 1000f64.powf((b % 100) as f64 / 99.0);
            p.set_param(cutoff, hz);
            let mut l: Vec<f32> = (0..128)
                .map(|i| if (b * 128 + i) % 97 == 0 { 0.5 } else { -0.1 })
                .collect();
            let mut r = l.clone();
            p.process(&mut l, &mut r);
            for v in l.iter().chain(&r) {
                assert!(v.is_finite());
                most = most.max(v.abs());
            }
        }
        assert!(most < 50.0, "{most}");
    }
}
