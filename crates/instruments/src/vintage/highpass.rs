// SPDX-License-Identifier: GPL-3.0-or-later
//! The plain high-pass before a synth's main filter: no resonance, one
//! or two poles, stepped (a four-position switch) or continuous.

use super::onepole::{OnePole, coefficient};

#[derive(Clone, Copy, Debug, Default)]
pub struct Highpass {
    stage: [OnePole; 2],
    big_g: f32,
    poles: u8,
    /// Below this the signal passes straight through.
    off: bool,
}

impl Highpass {
    pub fn new() -> Highpass {
        Highpass::default()
    }

    /// One or two poles at `cutoff` Hz; a cutoff under 10 Hz is off.
    pub fn set(&mut self, poles: u8, cutoff: f32, rate: f32) {
        self.poles = poles.clamp(1, 2);
        self.off = cutoff < 10.0;
        self.big_g = coefficient(cutoff.max(10.0), rate).1;
    }

    /// The cutoff of a four-position switch: off, then three steps.
    pub fn stepped(position: u32) -> f32 {
        match position {
            0 => 0.0,
            1 => 100.0,
            2 => 250.0,
            _ => 600.0,
        }
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        if self.off {
            return x;
        }
        let mut v = x;
        for s in &mut self.stage[..self.poles as usize] {
            v = s.hp(v, self.big_g);
        }
        v
    }

    pub fn reset(&mut self) {
        self.stage = [OnePole::default(); 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    #[test]
    fn it_cuts_below_its_corner_by_its_poles() {
        let rate = 48_000.0;
        let at = |poles, hz| {
            let mut h = Highpass::new();
            h.set(poles, 400.0, rate);
            measure::db(hz, rate, |x| h.run(x))
        };
        assert!(at(1, 8000.0).abs() < 0.1);
        assert!((at(1, 50.0) - at(1, 25.0) - 6.0).abs() < 0.5);
        assert!((at(2, 50.0) - at(2, 25.0) - 12.0).abs() < 0.5);
        let mut h = Highpass::new();
        h.set(2, 0.0, rate);
        assert_eq!(h.run(0.3), 0.3);
    }
}
