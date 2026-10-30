// SPDX-License-Identifier: GPL-3.0-or-later
//! Multiband dynamics: the sound split into low, middle and high bands,
//! each with its own compressor and level, then added back together.
//!
//! The splits are fourth-order Linkwitz-Riley crossovers (S. H. Linkwitz,
//! "Active Crossover Networks for Noncoincident Drivers", JAES 24(1),
//! 1976): two second-order Butterworth filters in a row, whose low and
//! high parts add back to the sound with its level unchanged at every
//! frequency. The low band also passes through the upper split's
//! all-pass, so all three bands stay in phase and, with nothing turned
//! down, the bands add back to a flat response. Each band's compressor is
//! the one the Compressor effect uses.

use crate::biquad::{Biquad, Coefs};
use crate::dynamics::Compressor;
use crate::{Dsp, Param, Unit};

pub const PARAMS: &[Param] = &[
    Param::new("low_split", "Low split", 40.0, 1000.0, 200.0, Unit::Hz).log(),
    Param::new(
        "high_split",
        "High split",
        1000.0,
        12000.0,
        3000.0,
        Unit::Hz,
    )
    .log(),
    Param::new(
        "low_threshold",
        "Low threshold",
        -60.0,
        0.0,
        -24.0,
        Unit::Db,
    ),
    Param::new("low_ratio", "Low ratio", 1.0, 20.0, 2.0, Unit::Ratio).log(),
    Param::new("low_gain", "Low level", -12.0, 12.0, 0.0, Unit::Db),
    Param::new(
        "mid_threshold",
        "Middle threshold",
        -60.0,
        0.0,
        -24.0,
        Unit::Db,
    ),
    Param::new("mid_ratio", "Middle ratio", 1.0, 20.0, 2.0, Unit::Ratio).log(),
    Param::new("mid_gain", "Middle level", -12.0, 12.0, 0.0, Unit::Db),
    Param::new(
        "high_threshold",
        "High threshold",
        -60.0,
        0.0,
        -24.0,
        Unit::Db,
    ),
    Param::new("high_ratio", "High ratio", 1.0, 20.0, 2.0, Unit::Ratio).log(),
    Param::new("high_gain", "High level", -12.0, 12.0, 0.0, Unit::Db),
    Param::new("attack", "Attack", 0.1, 100.0, 10.0, Unit::Ms).log(),
    Param::new("release", "Release", 10.0, 1000.0, 150.0, Unit::Ms).log(),
];

const Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Each band compressor's knee, dB.
const KNEE: f64 = 6.0;

/// A fourth-order Linkwitz-Riley low or high pass.
#[derive(Clone, Copy, Default)]
struct Lr4([Biquad; 2]);

impl Lr4 {
    fn set(&mut self, c: Coefs) {
        self.0[0].c = c;
        self.0[1].c = c;
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.0[0].run(x);
        self.0[1].run(y)
    }
}

/// One side's filters.
#[derive(Clone, Copy, Default)]
struct Split {
    low: Lr4,
    above_low: Lr4,
    mid: Lr4,
    high: Lr4,
    /// The upper split's all-pass, for the low band: its two halves.
    low_ap: [Lr4; 2],
}

impl Split {
    /// Low, middle and high.
    #[inline]
    fn run(&mut self, x: f32) -> [f32; 3] {
        let low = self.low.run(x);
        let rest = self.above_low.run(x);
        let low = self.low_ap[0].run(low) + self.low_ap[1].run(low);
        [low, self.mid.run(rest), self.high.run(rest)]
    }
}

pub struct Multiband {
    rate: f64,
    sides: [Split; 2],
    bands: [Compressor; 3],
}

impl Multiband {
    pub fn new(rate: f64) -> Multiband {
        Multiband {
            rate,
            sides: Default::default(),
            bands: std::array::from_fn(|_| Compressor::new(rate)),
        }
    }
}

impl Dsp for Multiband {
    fn update(&mut self, v: &[f64]) {
        let (lo, hi) = (v[0], v[1].max(v[0]));
        let rate = self.rate;
        for s in &mut self.sides {
            s.low.set(Coefs::low_pass(lo, Q, rate));
            s.above_low.set(Coefs::high_pass(lo, Q, rate));
            s.mid.set(Coefs::low_pass(hi, Q, rate));
            s.high.set(Coefs::high_pass(hi, Q, rate));
            s.low_ap[0].set(Coefs::low_pass(hi, Q, rate));
            s.low_ap[1].set(Coefs::high_pass(hi, Q, rate));
        }
        for (b, c) in self.bands.iter_mut().enumerate() {
            let at = 2 + b * 3;
            // Threshold, ratio, attack, release, knee, make-up, mix.
            c.update(&[v[at], v[at + 1], v[11], v[12], KNEE, v[at + 2], 100.0]);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let bl = self.sides[0].run(*l);
            let br = self.sides[1].run(*r);
            let (mut ol, mut or) = (0.0, 0.0);
            for (b, c) in self.bands.iter_mut().enumerate() {
                let g = c.step(bl[b].abs().max(br[b].abs()));
                ol += bl[b] * g;
                or += br[b] * g;
            }
            *l = ol;
            *r = or;
        }
    }

    fn reduction_db(&self) -> f32 {
        self.bands
            .iter()
            .map(|b| b.reduction_db())
            .fold(0.0, f32::min)
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

    /// With nothing turned down, the bands add back to the sound's own
    /// level at every frequency, splits included.
    #[test]
    fn the_bands_add_back_flat() {
        let e = Effect::new(Kind::Multiband);
        for b in 0..3 {
            e.set(3 + b * 3, 1.0);
        }
        for hz in [30.0, 100.0, 200.0, 700.0, 3000.0, 8000.0, 16000.0] {
            let d = level_out(&e, hz, 0.25);
            assert!(d.abs() < 0.1, "{hz} Hz: {d:.2} dB");
        }
    }

    /// Each band's level moves only its own band.
    #[test]
    fn band_levels_move_their_band() {
        let e = Effect::new(Kind::Multiband);
        for b in 0..3 {
            e.set(3 + b * 3, 1.0);
        }
        e.set(7, 6.0);
        assert!((level_out(&e, 900.0, 0.25) - 6.0).abs() < 0.5);
        assert!(level_out(&e, 50.0, 0.25).abs() < 0.3);
        assert!(level_out(&e, 12000.0, 0.25).abs() < 0.3);
    }

    /// A loud bass note is turned down by the low band's compressor and
    /// a quiet treble alongside isn't.
    #[test]
    fn each_band_compresses_alone() {
        let e = Effect::new(Kind::Multiband);
        e.set(2, -30.0);
        e.set(3, 10.0);
        e.set(5, 0.0);
        e.set(8, 0.0);
        assert!(level_out(&e, 60.0, 0.5) < -10.0);
        assert!(e.reduction_db() < -10.0);
        assert!(level_out(&e, 8000.0, 0.5).abs() < 0.3);
    }
}
