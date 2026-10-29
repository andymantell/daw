// SPDX-License-Identifier: GPL-3.0-or-later
//! A six-band EQ: a low cut, a bass shelf, two bell bands, a treble shelf
//! and a high cut.

use std::f64::consts::FRAC_1_SQRT_2;

use crate::biquad::{Biquad, Coefs};
use crate::{Dsp, Param, Unit, gain};

pub const PARAMS: &[Param] = &[
    Param::new("low_cut", "Low cut", 20.0, 1000.0, 20.0, Unit::Hz)
        .log()
        .off_at(20.0),
    Param::new("low_gain", "Bass", -18.0, 18.0, 0.0, Unit::Db),
    Param::new("low_freq", "Bass frequency", 30.0, 500.0, 100.0, Unit::Hz).log(),
    Param::new("mid1_gain", "Low middle", -18.0, 18.0, 0.0, Unit::Db),
    Param::new(
        "mid1_freq",
        "Low middle frequency",
        100.0,
        2000.0,
        400.0,
        Unit::Hz,
    )
    .log(),
    Param::new("mid1_q", "Low middle width (Q)", 0.3, 8.0, 1.0, Unit::Plain).log(),
    Param::new("mid2_gain", "High middle", -18.0, 18.0, 0.0, Unit::Db),
    Param::new(
        "mid2_freq",
        "High middle frequency",
        500.0,
        10000.0,
        2500.0,
        Unit::Hz,
    )
    .log(),
    Param::new(
        "mid2_q",
        "High middle width (Q)",
        0.3,
        8.0,
        1.0,
        Unit::Plain,
    )
    .log(),
    Param::new("high_gain", "Treble", -18.0, 18.0, 0.0, Unit::Db),
    Param::new(
        "high_freq",
        "Treble frequency",
        1500.0,
        16000.0,
        8000.0,
        Unit::Hz,
    )
    .log(),
    Param::new("high_cut", "High cut", 2000.0, 20000.0, 20000.0, Unit::Hz)
        .log()
        .off_at(20000.0),
    Param::new("output", "Output", -18.0, 18.0, 0.0, Unit::Db),
];

const BANDS: usize = 6;

pub struct Eq {
    rate: f64,
    /// Each band, for the left and right channels.
    bands: [[Biquad; 2]; BANDS],
    /// Bands that do something, so the others can be skipped.
    active: [bool; BANDS],
    out: f32,
}

impl Eq {
    pub fn new(rate: f64) -> Eq {
        Eq {
            rate,
            bands: Default::default(),
            active: [false; BANDS],
            out: 1.0,
        }
    }

    /// The coefficients for `v`, and whether each band does anything.
    pub(crate) fn design(v: &[f64], rate: f64) -> [Option<Coefs>; BANDS] {
        let off = |i: usize| PARAMS[i].off_at == Some(v[i]);
        let flat = |db: f64| db.abs() < 0.01;
        [
            (!off(0)).then(|| Coefs::high_pass(v[0], FRAC_1_SQRT_2, rate)),
            (!flat(v[1])).then(|| Coefs::low_shelf(v[2], v[1], rate)),
            (!flat(v[3])).then(|| Coefs::peak(v[4], v[5], v[3], rate)),
            (!flat(v[6])).then(|| Coefs::peak(v[7], v[8], v[6], rate)),
            (!flat(v[9])).then(|| Coefs::high_shelf(v[10], v[9], rate)),
            (!off(11)).then(|| Coefs::low_pass(v[11], FRAC_1_SQRT_2, rate)),
        ]
    }
}

impl Dsp for Eq {
    fn update(&mut self, v: &[f64]) {
        for (b, c) in Eq::design(v, self.rate).into_iter().enumerate() {
            self.active[b] = c.is_some();
            for ch in &mut self.bands[b] {
                ch.c = c.unwrap_or(Coefs::THROUGH);
            }
        }
        self.out = gain(v[12]) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (b, [l, r]) in self.bands.iter_mut().enumerate() {
            if !self.active[b] {
                continue;
            }
            for x in left.iter_mut() {
                *x = l.run(*x);
            }
            for x in right.iter_mut() {
                *x = r.run(*x);
            }
        }
        if self.out != 1.0 {
            for x in left.iter_mut().chain(right.iter_mut()) {
                *x *= self.out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;

    fn tone_level(e: &Effect, hz: f64) -> f64 {
        let mut p = e.processor(48_000);
        let mut l = vec![0f32; 48_000];
        for (n, x) in l.iter_mut().enumerate() {
            *x = (2.0 * std::f64::consts::PI * hz * n as f64 / 48_000.0).sin() as f32 * 0.25;
        }
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        // By its power: the peaks of a high tone fall between samples.
        let rms = (l[24_000..].iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / 24_000.0).sqrt();
        20.0 * (rms * 2f64.sqrt() / 0.25).log10()
    }

    #[test]
    fn flat_by_default() {
        let e = Effect::new(Kind::Eq);
        for hz in [40.0, 400.0, 4000.0, 15000.0] {
            assert!(tone_level(&e, hz).abs() < 0.01, "{hz}");
        }
    }

    #[test]
    fn bands_do_what_they_say() {
        let e = Effect::new(Kind::Eq);
        e.set(3, 9.0); // Low middle +9 dB at 400 Hz.
        e.set(0, 200.0); // Low cut at 200 Hz.
        assert!((tone_level(&e, 400.0) - 9.0).abs() < 0.6);
        assert!(tone_level(&e, 40.0) < -24.0);
        let t = tone_level(&e, 8000.0);
        assert!(t.abs() < 0.2, "{t}");
        let e = Effect::new(Kind::Eq);
        e.set(11, 3000.0); // High cut.
        e.set(12, -6.0);
        assert!(tone_level(&e, 15000.0) < -30.0);
        assert!((tone_level(&e, 200.0) + 6.0).abs() < 0.05);
    }
}
