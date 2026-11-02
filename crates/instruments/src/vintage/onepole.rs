// SPDX-License-Identifier: GPL-3.0-or-later
//! The one-pole low-pass every filter here is built from, discretised by
//! the trapezoidal rule (the topology-preserving transform of
//! Zavalishin's book): its cutoff can move every sample without the
//! state jumping.

use std::f32::consts::PI;

/// A one-pole low-pass. Its output for an input `x` is `G x + (1 − G) s`
/// where `s` is its state and `G = g / (1 + g)`; the other filters use
/// that linear form to solve their feedback loops before they advance
/// the stage.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    pub s: f32,
}

/// The gain term of a cutoff: `g = tan(π fc / fs)`, clamped below the
/// Nyquist frequency, and `G = g / (1 + g)`.
#[inline]
pub fn coefficient(cutoff: f32, rate: f32) -> (f32, f32) {
    let g = (PI * cutoff.clamp(1.0, rate * 0.49) / rate).tan();
    (g, g / (1.0 + g))
}

impl OnePole {
    /// The low-pass output for `x`, given `big_g` (`G`), and the state
    /// advanced.
    #[inline]
    pub fn lp(&mut self, x: f32, big_g: f32) -> f32 {
        let v = big_g * (x - self.s);
        let y = v + self.s;
        self.s = y + v;
        y
    }

    /// The high-pass output: what the low-pass leaves out.
    #[inline]
    pub fn hp(&mut self, x: f32, big_g: f32) -> f32 {
        x - self.lp(x, big_g)
    }

    /// What the stage would output for `x` before advancing: the
    /// linear form the loop solvers use.
    #[inline]
    pub fn predict(&self, x: f32, big_g: f32) -> f32 {
        big_g * x + (1.0 - big_g) * self.s
    }

    pub fn reset(&mut self) {
        self.s = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    #[test]
    fn it_is_three_decibels_down_at_its_cutoff() {
        let rate = 48_000.0;
        let (_, big_g) = coefficient(1000.0, rate);
        let at = |hz| {
            let mut p = OnePole::default();
            measure::db(hz, rate, |x| p.lp(x, big_g))
        };
        assert!((at(1000.0) + 3.0).abs() < 0.2);
        assert!((at(100.0)).abs() < 0.1);
        // Six decibels an octave well above the cutoff.
        assert!((at(4000.0) - at(8000.0) - 6.0).abs() < 0.5);
        let mut p = OnePole::default();
        assert!(measure::db(20.0, rate, |x| p.hp(x, big_g)) < -30.0);
    }
}
