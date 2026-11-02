// SPDX-License-Identifier: GPL-3.0-or-later
//! The two-pole state-variable filter of the American two-pole poly and
//! module filters: two integrators in a loop, giving low-pass, band-pass
//! and high-pass at once. The low → notch → high control blends the
//! low-pass and high-pass outputs, passing through a notch half way, as
//! those filters' sweep does, and a switch takes the band-pass instead.
//! The damping path (what sets the resonance) saturates, so a hot input
//! or high resonance thickens instead of blowing up, and it can
//! self-oscillate.
//!
//! Trapezoidal (zero-delay feedback) form from Zavalishin; the
//! saturated loop is solved by Newton's method; runs at twice the rate.

use super::onepole::coefficient;
use super::oversample::Over2;
use super::saturate::{flush, soft, soft_pair};

/// Which output the filter gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SvfOut {
    /// 0 is low-pass, 0.5 the notch, 1 high-pass.
    Morph(f32),
    Band,
}

#[derive(Clone, Copy, Debug)]
struct Core {
    s1: f32,
    s2: f32,
    g: f32,
    /// The damping, `2R`: 2 is no resonance, 0 self-oscillates.
    damp: f32,
    gain: f32,
    out: SvfOut,
}

impl Default for Core {
    fn default() -> Core {
        Core {
            s1: 0.0,
            s2: 0.0,
            g: 0.0,
            damp: 2.0,
            gain: 1.0,
            out: SvfOut::Morph(0.0),
        }
    }
}

impl Core {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let x = soft(x * self.gain);
        let g = self.g;
        // hp = x − damp·soft(bp) − lp; bp = g hp + s1; lp = g bp + s2.
        // So bp (1 + g²) + g·damp·soft(bp) = g (x − s2) + s1.
        let rhs = g * (x - self.s2) + self.s1;
        let mut bp = rhs / (1.0 + g * g + g * self.damp);
        for _ in 0..2 {
            let (sat, slope) = soft_pair(bp);
            let f = bp * (1.0 + g * g) + g * self.damp * sat - rhs;
            let df = 1.0 + g * g + g * self.damp * slope;
            bp -= f / df;
        }
        let lp = g * bp + self.s2;
        let hp = x - self.damp * soft(bp) - lp;
        self.s1 = 2.0 * bp - self.s1;
        self.s2 = 2.0 * lp - self.s2;
        match self.out {
            SvfOut::Band => bp * self.damp.max(0.25),
            SvfOut::Morph(m) => {
                // Low-pass alone, through both summed (the notch), to
                // high-pass alone.
                let m = m.clamp(0.0, 1.0);
                ((1.0 - m) * lp + m * hp) / (1.0 - m).max(m)
            }
        }
    }
}

/// The state-variable filter, oversampled unless made [`plain`](Svf::plain).
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    core: Core,
    over: Over2,
    /// Runs at the sample rate with no oversampling: a quarter of the
    /// cost, for a synth with many two-pole filters a voice whose
    /// saturation is mild.
    plain: bool,
}

impl Svf {
    pub fn new() -> Svf {
        Svf::default()
    }

    /// A filter run at the sample rate, with no oversampling.
    pub fn plain() -> Svf {
        Svf {
            plain: true,
            ..Svf::default()
        }
    }

    /// Sets the output, the cutoff in Hz, the resonance (0 to 1; it
    /// sings at 1) and the drive into the filter.
    pub fn set(&mut self, out: SvfOut, cutoff: f32, resonance: f32, drive: f32, rate: f32) {
        let c = &mut self.core;
        c.out = out;
        let rate = if self.plain { rate } else { rate * 2.0 };
        c.g = coefficient(cutoff.max(5.0), rate).0;
        // A little past lossless at the top, so it sings rather than
        // merely rings, with the saturated loop setting the level.
        c.damp = 2.0 - 2.1 * resonance.clamp(0.0, 1.0);
        c.gain = drive.max(0.0);
        flush(&mut c.s1);
        flush(&mut c.s2);
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let core = &mut self.core;
        if self.plain {
            return core.tick(x);
        }
        self.over.run(x, |v| core.tick(v))
    }

    pub fn reset(&mut self) {
        self.core.s1 = 0.0;
        self.core.s2 = 0.0;
        self.over.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    const RATE: f32 = 48_000.0;

    fn at(out: SvfOut, cutoff: f32, res: f32, hz: f32) -> f32 {
        let mut s = Svf::new();
        s.set(out, cutoff, res, 1.0, RATE);
        measure::db(hz, RATE, |x| s.run(x * 0.1)) - measure::db(hz, RATE, |x| x * 0.1)
    }

    #[test]
    fn the_sweep_goes_low_notch_high() {
        let low = SvfOut::Morph(0.0);
        assert!(at(low, 1000.0, 0.0, 100.0).abs() < 0.3);
        assert!(at(low, 1000.0, 0.0, 8000.0) < -30.0);
        let d = at(low, 1000.0, 0.0, 4000.0) - at(low, 1000.0, 0.0, 8000.0);
        assert!((d - 12.0).abs() < 1.0, "{d}");
        let high = SvfOut::Morph(1.0);
        assert!(at(high, 1000.0, 0.0, 10_000.0).abs() < 0.5);
        assert!(at(high, 1000.0, 0.0, 100.0) < -30.0);
        let notch = SvfOut::Morph(0.5);
        assert!(
            at(notch, 1000.0, 0.0, 1000.0) < -30.0,
            "{}",
            at(notch, 1000.0, 0.0, 1000.0)
        );
        assert!(at(notch, 1000.0, 0.0, 100.0) > -1.0);
        assert!(at(notch, 1000.0, 0.0, 10_000.0) > -1.0);
        assert!(at(SvfOut::Band, 1000.0, 0.0, 1000.0) > -1.0);
        assert!(at(SvfOut::Band, 1000.0, 0.0, 100.0) < -12.0);
    }

    #[test]
    fn resonance_peaks_at_the_cutoff_and_sings_at_the_top() {
        let peak = at(SvfOut::Morph(0.0), 1000.0, 0.8, 1000.0);
        assert!(peak > 6.0, "{peak}");
        let mut s = Svf::new();
        s.set(SvfOut::Morph(0.0), 2000.0, 1.0, 1.0, RATE);
        let out: Vec<f32> = (0..RATE as usize)
            .map(|i| s.run(if i == 0 { 0.5 } else { 0.0 }))
            .collect();
        let hz = measure::ring_hz(&out, RATE);
        assert!((hz / 2000.0 - 1.0).abs() < 0.02, "{hz}");
        assert!(measure::peak(&out[40_000..]) > 0.1);
    }
}
