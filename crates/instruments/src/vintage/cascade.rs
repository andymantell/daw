// SPDX-License-Identifier: GPL-3.0-or-later
//! The four-pole transconductance cascade: four buffered one-pole stages
//! with the feedback taken round all four (24 dB) or, in the two-pole
//! mode, round the first two with the voltage between them fed back,
//! as the integrated filter chips of the late-1970s and 1980s polysynths
//! have it (two one-poles alone can't reach the 180° a resonance
//! needs, so the two-pole mode feeds back the difference of its stages,
//! the Sallen-Key way). Its stages are cleaner than the transistor
//! ladder's, so its resonance is sharper and reaches self-oscillation
//! with a purer tone; the soft limits on the input and the feedback
//! path colour a hot input and keep that oscillation at a fixed level.
//!
//! Zero-delay feedback form (Zavalishin): the linear chain is solved in
//! closed form, the feedback limit by Newton's method around it, at twice
//! the sample rate.

use super::onepole::{OnePole, coefficient};
use super::oversample::Over2;
use super::saturate::{flush, soft, soft_pair};

/// How many poles the feedback goes round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Poles {
    Two,
    #[default]
    Four,
}

#[derive(Clone, Copy, Debug, Default)]
struct Core {
    stage: [OnePole; 4],
    big_g: f32,
    k: f32,
    gain: f32,
    poles: Poles,
}

impl Core {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let x = soft(x * self.gain);
        let g = self.big_g;
        let mut v = match self.poles {
            Poles::Four => {
                // The chain's last output is G⁴ u + (state terms):
                // y = y0 + yu u, and u = x − soft(k y): Newton on y.
                let (mut y0, mut yu) = (0.0, 1.0);
                for s in &self.stage {
                    y0 = g * y0 + (1.0 - g) * s.s;
                    yu *= g;
                }
                let mut y = y0 + yu * (x - soft(self.k * y0));
                for _ in 0..2 {
                    let (sat, slope) = soft_pair(self.k * y);
                    let f = y - y0 - yu * (x - sat);
                    let df = 1.0 + yu * self.k * slope;
                    y -= f / df;
                }
                x - soft(self.k * y)
            }
            Poles::Two => {
                // The difference of the two stages, d = y1 − y2, is fed
                // back: u = x + k soft(d), with d = d0 + d1 u.
                let (s1, s2) = (self.stage[0].s, self.stage[1].s);
                let y1_0 = (1.0 - g) * s1;
                let d0 = y1_0 - (g * y1_0 + (1.0 - g) * s2);
                let d1 = g - g * g;
                let mut d = d0 + d1 * (x + self.k * soft(d0));
                for _ in 0..2 {
                    let (sat, slope) = soft_pair(d);
                    let f = d - d0 - d1 * (x + self.k * sat);
                    let df = 1.0 - d1 * self.k * slope;
                    d -= f / df;
                }
                x + self.k * soft(d)
            }
        };
        let n = match self.poles {
            Poles::Two => 2,
            Poles::Four => 4,
        };
        for s in &mut self.stage[..n] {
            v = s.lp(v, g);
        }
        v
    }
}

/// The cascade filter, oversampled.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cascade {
    core: Core,
    over: Over2,
}

impl Cascade {
    pub fn new() -> Cascade {
        Cascade::default()
    }

    /// Sets the poles, the cutoff in Hz, the resonance (0 to 1, singing
    /// from about 0.9), the drive into the first stage and the bass
    /// compensation (0 as the circuit is, 1 all of it).
    pub fn set(
        &mut self,
        poles: Poles,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        compensation: f32,
        rate: f32,
    ) {
        let c = &mut self.core;
        c.poles = poles;
        c.big_g = coefficient(cutoff.max(5.0), rate * 2.0).1;
        let max = match poles {
            Poles::Two => 2.15,
            Poles::Four => 4.8,
        };
        c.k = max * resonance.clamp(0.0, 1.0);
        c.gain = drive.max(0.0) * (1.0 + compensation.clamp(0.0, 1.0) * c.k);
        for s in &mut c.stage {
            flush(&mut s.s);
        }
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let core = &mut self.core;
        self.over.run(x, |v| core.tick(v))
    }

    pub fn reset(&mut self) {
        for s in &mut self.core.stage {
            s.reset();
        }
        self.over.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    const RATE: f32 = 48_000.0;

    fn cascade(poles: Poles, cutoff: f32, res: f32) -> Cascade {
        let mut c = Cascade::new();
        c.set(poles, cutoff, res, 1.0, 0.0, RATE);
        c
    }

    #[test]
    fn four_poles_roll_off_at_twenty_four_and_two_at_twelve() {
        for (poles, slope) in [(Poles::Four, 24.0), (Poles::Two, 12.0)] {
            let at = |hz| {
                let mut c = cascade(poles, 500.0, 0.0);
                measure::db(hz, RATE, |x| c.run(x * 0.1)) - measure::db(hz, RATE, |x| x * 0.1)
            };
            assert!(at(50.0).abs() < 0.3);
            assert!((at(500.0) + 3.0 * slope / 6.0).abs() < 0.5, "{}", at(500.0));
            let d = at(4000.0) - at(8000.0);
            assert!((d - slope).abs() < 1.0, "{poles:?}: {d}");
        }
    }

    #[test]
    fn it_sings_at_the_cutoff_with_a_steady_level() {
        for (poles, cutoff) in [
            (Poles::Four, 300.0),
            (Poles::Four, 3000.0),
            (Poles::Two, 1000.0),
        ] {
            let mut c = cascade(poles, cutoff, 1.0);
            let out: Vec<f32> = (0..RATE as usize * 2)
                .map(|i| c.run(if i == 0 { 0.5 } else { 0.0 }))
                .collect();
            let hz = measure::ring_hz(&out, RATE);
            assert!((hz / cutoff - 1.0).abs() < 0.03, "{poles:?} {cutoff}: {hz}");
            let a = measure::peak(&out[48_000..72_000]);
            let b = measure::peak(&out[72_000..]);
            assert!(a > 0.08 && (a - b).abs() < 0.05 * a, "{a} {b}");
        }
    }
}
