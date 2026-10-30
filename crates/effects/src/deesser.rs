// SPDX-License-Identifier: GPL-3.0-or-later
//! A de-esser: turns down sharp "s" and "t" sounds (or a pick's click)
//! when they get loud, and leaves the rest alone.
//!
//! It listens through a high pass at the chosen frequency, and when what
//! it hears goes over the threshold it turns down, by a ratio of 4:1 and
//! at most the range. It can turn down only the highs, split from the
//! rest by a low pass and what that leaves (which add back to the sound
//! exactly), or the whole sound. Both sides share one gain.

use crate::biquad::{Biquad, Coefs};
use crate::{Dsp, Param, Unit, coef, db, gain};

pub const PARAMS: &[Param] = &[
    Param::new("frequency", "Frequency", 2000.0, 12000.0, 6000.0, Unit::Hz).log(),
    Param::new("threshold", "Threshold", -60.0, 0.0, -30.0, Unit::Db),
    Param::new("range", "Most turned down", 0.0, 24.0, 10.0, Unit::Db),
    Param::new("mode", "Turns down", 0.0, 1.0, 0.0, Unit::Choice)
        .choices(&["Only the highs", "The whole sound"]),
    Param::new(
        "listen",
        "Hear what it listens to",
        0.0,
        1.0,
        0.0,
        Unit::Switch,
    ),
];

const RATIO: f64 = 4.0;
const Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

pub struct DeEsser {
    rate: f64,
    /// What it listens through, per side.
    listen_to: [Biquad; 2],
    /// The low part of each side, when it turns down only the highs.
    low: [Biquad; 2],
    threshold: f64,
    range: f64,
    highs_only: bool,
    listen: bool,
    env: f64,
    up: f64,
    down: f64,
    /// How far it is turning down now, in dB (0 or above).
    level: f64,
}

impl DeEsser {
    pub fn new(rate: f64) -> DeEsser {
        DeEsser {
            rate,
            listen_to: Default::default(),
            low: Default::default(),
            threshold: 0.0,
            range: 0.0,
            highs_only: true,
            listen: false,
            env: 0.0,
            up: coef(0.0005, rate),
            down: coef(0.06, rate),
            level: 0.0,
        }
    }
}

impl Dsp for DeEsser {
    fn update(&mut self, v: &[f64]) {
        let hp = Coefs::high_pass(v[0], Q, self.rate);
        let lp = Coefs::low_pass(v[0], Q, self.rate);
        for f in &mut self.listen_to {
            f.c = hp;
        }
        for f in &mut self.low {
            f.c = lp;
        }
        self.threshold = v[1];
        self.range = v[2];
        self.highs_only = v[3] < 0.5;
        self.listen = v[4] >= 0.5;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let hl = self.listen_to[0].run(*l);
            let hr = self.listen_to[1].run(*r);
            let x = hl.abs().max(hr.abs()) as f64;
            let c = if x > self.env { self.up } else { self.down };
            self.env = x + (self.env - x) * c;
            let over = db(self.env) - self.threshold;
            self.level = (over * (1.0 - 1.0 / RATIO)).clamp(0.0, self.range);
            let g = gain(-self.level) as f32;
            // Keeps the low part in step even while listening, so
            // switching back doesn't click.
            let ll = self.low[0].run(*l);
            let lr = self.low[1].run(*r);
            if self.listen {
                *l = hl;
                *r = hr;
            } else if self.highs_only {
                *l = ll + (*l - ll) * g;
                *r = lr + (*r - lr) * g;
            } else {
                *l *= g;
                *r *= g;
            }
        }
    }

    fn reduction_db(&self) -> f32 {
        -self.level as f32
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;
    use std::f64::consts::TAU;

    const RATE: f64 = 48_000.0;

    fn level_out(e: &Effect, hz: f64, amp: f64) -> f64 {
        let mut p = e.processor(RATE as u32);
        let mut l: Vec<f32> = (0..24_000)
            .map(|n| (amp * (TAU * hz * n as f64 / RATE).sin()) as f32)
            .collect();
        let mut r = l.clone();
        for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(l, r);
        }
        // From the RMS, as a high tone's samples can miss its peaks.
        let tail = &l[12_000..];
        let rms =
            (tail.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        crate::db(rms * std::f64::consts::SQRT_2 / amp)
    }

    #[test]
    fn loud_highs_are_turned_down_and_lows_are_not() {
        let e = Effect::new(Kind::DeEsser);
        // A loud 8 kHz "s": turned down by its range (the part of it
        // below the split stays).
        let s = level_out(&e, 8000.0, 0.5);
        assert!(s < -4.0, "{s:.1} dB");
        assert!((e.reduction_db() + 10.0).abs() < 0.01);
        e.set(3, 1.0);
        let s = level_out(&e, 8000.0, 0.5);
        assert!((s + 10.0).abs() < 0.3, "{s:.1} dB");
        e.set(3, 0.0);
        // A loud 200 Hz note: through it goes.
        assert!(level_out(&e, 200.0, 0.5).abs() < 0.2);
        // A quiet "s": below the threshold, left alone.
        assert!(level_out(&e, 8000.0, 0.005).abs() < 0.2);
    }

    #[test]
    fn the_split_adds_back_to_the_sound() {
        let e = Effect::new(Kind::DeEsser);
        e.set(1, 0.0);
        let mut p = e.processor(RATE as u32);
        let mut seed = 7u32;
        let input: Vec<f32> = (0..4800)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1 << 23) as f32 - 1.0) * 0.1
            })
            .collect();
        let (mut l, mut r) = (input.clone(), input.clone());
        p.process(&mut l, &mut r);
        for (a, b) in l.iter().zip(&input) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn whole_sound_mode_turns_the_lows_down_too() {
        let e = Effect::new(Kind::DeEsser);
        e.set(3, 1.0);
        let mut p = e.processor(RATE as u32);
        // A low note and a loud "s" together: the low note comes down.
        let mut l: Vec<f32> = (0..24_000)
            .map(|n| {
                let t = n as f64 / RATE;
                (0.3 * (TAU * 200.0 * t).sin() + 0.5 * (TAU * 8000.0 * t).sin()) as f32
            })
            .collect();
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        assert!(e.reduction_db() < -6.0);
    }
}
