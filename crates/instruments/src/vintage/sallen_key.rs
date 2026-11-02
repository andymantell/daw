// SPDX-License-Identifier: GPL-3.0-or-later
//! The two-pole Sallen-Key filters of the Japanese semi-modular: two
//! one-pole sections with the voltage across the second's capacitor fed
//! back to the first's input through a gain, and a clipping pair in that
//! path. The feedback adds no gain at DC (the capacitor passes none) and
//! makes a two-pole resonance whose peak is set by the gain alone; the
//! clipper limits it, so turning the resonance up past self-oscillation
//! doesn't blow up but distorts, hard and on purpose: the sound of that
//! filter screaming.
//!
//! Zero-delay feedback form (Zavalishin's treatment of Sallen-Key
//! filters); the clipped loop is solved by Newton's method; runs at twice
//! the rate.

use super::onepole::{OnePole, coefficient};
use super::oversample::Over2;
use super::saturate::{flush, soft, soft_pair};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SkMode {
    #[default]
    Low,
    High,
}

#[derive(Clone, Copy, Debug, Default)]
struct Core {
    a: OnePole,
    b: OnePole,
    big_g: f32,
    k: f32,
    gain: f32,
    mode: SkMode,
}

impl Core {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let x = soft(x * self.gain);
        let g = self.big_g;
        let (sa, sb) = (self.a.s, self.b.s);
        // With f the clipped feedback, the first section's input is
        // x + k f. The feedback signal is the second section's capacitor
        // voltage: y1 − y2 for the low-pass pair, G h1 + (1−G) sb for the
        // high-pass pair (the high-pass takes what the low-pass leaves).
        // Both are linear in f: v = c0 + c1 f. Solve f = soft(c0 + c1 f).
        let (c0, c1) = match self.mode {
            SkMode::Low => {
                // y1 = G(x + kf) + (1−G) sa; y2 = G y1 + (1−G) sb.
                let y1_0 = g * x + (1.0 - g) * sa;
                let y1_1 = g * self.k;
                let v0 = y1_0 - (g * y1_0 + (1.0 - g) * sb);
                let v1 = y1_1 - g * y1_1;
                (v0, v1)
            }
            SkMode::High => {
                // h1 = (x + kf) − lp1 = (1−G)(x + kf) − (1−G) sa.
                let h1_0 = (1.0 - g) * (x - sa);
                let h1_1 = (1.0 - g) * self.k;
                (g * h1_0 + (1.0 - g) * sb, g * h1_1)
            }
        };
        let mut f = soft(c0 / (1.0 - c1).max(0.1));
        for _ in 0..3 {
            let (sat, slope) = soft_pair(c0 + c1 * f);
            let r = f - sat;
            let dr = 1.0 - c1 * slope;
            f -= r / dr;
        }
        let input = x + self.k * soft(c0 + c1 * f);
        match self.mode {
            SkMode::Low => {
                let y1 = self.a.lp(input, g);
                self.b.lp(y1, g)
            }
            SkMode::High => {
                let h1 = self.a.hp(input, g);
                self.b.hp(h1, g)
            }
        }
    }
}

/// A Sallen-Key filter, oversampled.
#[derive(Clone, Copy, Debug, Default)]
pub struct SallenKey {
    core: Core,
    over: Over2,
}

impl SallenKey {
    pub fn new() -> SallenKey {
        SallenKey::default()
    }

    /// Sets the mode, the cutoff in Hz, the resonance (0 to 1; it sings
    /// from about 0.9 and screams above) and the drive into the filter.
    pub fn set(&mut self, mode: SkMode, cutoff: f32, resonance: f32, drive: f32, rate: f32) {
        let c = &mut self.core;
        c.mode = mode;
        c.big_g = coefficient(cutoff.max(5.0), rate * 2.0).1;
        c.k = 2.3 * resonance.clamp(0.0, 1.0);
        c.gain = drive.max(0.0);
        flush(&mut c.a.s);
        flush(&mut c.b.s);
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let core = &mut self.core;
        self.over.run(x, |v| core.tick(v))
    }

    pub fn reset(&mut self) {
        self.core.a.reset();
        self.core.b.reset();
        self.over.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    const RATE: f32 = 48_000.0;

    fn at(mode: SkMode, cutoff: f32, res: f32, hz: f32) -> f32 {
        let mut s = SallenKey::new();
        s.set(mode, cutoff, res, 1.0, RATE);
        measure::db(hz, RATE, |x| s.run(x * 0.1)) - measure::db(hz, RATE, |x| x * 0.1)
    }

    #[test]
    fn both_pairs_are_two_pole_and_peak_at_the_cutoff() {
        assert!(at(SkMode::Low, 1000.0, 0.0, 50.0).abs() < 0.3);
        let d = at(SkMode::Low, 1000.0, 0.0, 4000.0) - at(SkMode::Low, 1000.0, 0.0, 8000.0);
        assert!((d - 12.0).abs() < 1.0, "{d}");
        assert!(at(SkMode::High, 1000.0, 0.0, 15_000.0).abs() < 0.5);
        let d = at(SkMode::High, 1000.0, 0.0, 250.0) - at(SkMode::High, 1000.0, 0.0, 125.0);
        assert!((d - 12.0).abs() < 1.0, "{d}");
        // Resonance lifts the cutoff, and leaves the bass alone.
        assert!(at(SkMode::Low, 1000.0, 0.75, 1000.0) > 6.0);
        assert!(at(SkMode::Low, 1000.0, 0.75, 50.0).abs() < 0.5);
        assert!(at(SkMode::High, 1000.0, 0.75, 1000.0) > 6.0);
    }

    #[test]
    fn it_sings_at_the_top_and_stays_bounded_past_it() {
        for mode in [SkMode::Low, SkMode::High] {
            let mut s = SallenKey::new();
            s.set(mode, 1500.0, 1.0, 1.0, RATE);
            let out: Vec<f32> = (0..RATE as usize)
                .map(|i| s.run(if i == 0 { 0.5 } else { 0.0 }))
                .collect();
            let hz = measure::ring_hz(&out, RATE);
            assert!((hz / 1500.0 - 1.0).abs() < 0.05, "{mode:?}: {hz}");
            let p = measure::peak(&out[40_000..]);
            assert!(p > 0.1 && p < 4.0, "{mode:?}: {p}");
        }
    }
}
