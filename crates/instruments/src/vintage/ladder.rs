// SPDX-License-Identifier: GPL-3.0-or-later
//! The four-pole transistor ladder: four one-pole stages in a row, each
//! a differential transistor pair whose transfer saturates, with the
//! fourth's output fed back to the first's input to make the
//! resonance. The saturation inside each stage is what makes it sing
//! and thicken as the input gets hotter, and why the bass drops as the
//! resonance rises.
//!
//! The method is Huovilainen's (DAFx 2004): each stage's input and
//! output pass through the saturating curve, the feedback is taken
//! through half a sample's delay compensation, and the whole runs at
//! twice the sample rate so the distortion doesn't alias.

use super::oversample::Over2;
use super::saturate::{flush, soft};

/// Resonance 1.0 is this much feedback: just past where a linear ladder
/// starts to oscillate (4.0), so the top of the range sings.
const MAX_FEEDBACK: f32 = 4.6;

#[derive(Clone, Copy, Debug, Default)]
struct Core {
    /// Each stage's output.
    y: [f32; 4],
    /// The saturated form of each stage's output, kept between samples.
    sat: [f32; 4],
    y4_prev: f32,
    /// The stage gain at the doubled rate.
    g: f32,
    k: f32,
    /// What the input is scaled by: drive, and the bass compensation.
    gain: f32,
}

impl Core {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        // The feedback as it was half a sample ago, which is where the
        // unit-delay loop puts the resonance right.
        let fb = 0.5 * (self.y[3] + self.y4_prev);
        self.y4_prev = self.y[3];
        let mut input = soft(x * self.gain - self.k * fb);
        for i in 0..4 {
            self.y[i] += self.g * (input - self.sat[i]);
            self.sat[i] = soft(self.y[i]);
            input = self.sat[i];
        }
        self.y[3]
    }
}

/// The ladder filter, oversampled.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ladder {
    core: Core,
    over: Over2,
}

impl Ladder {
    pub fn new() -> Ladder {
        Ladder::default()
    }

    /// Sets the cutoff in Hz, the resonance (0 to 1; it sings from
    /// about 0.93), the drive into the first stage (1.0 is clean for a
    /// signal within ±0.5, 4.0 is hard) and how much of the bass lost
    /// to resonance to put back (0 as the circuit is, 1 all of it).
    pub fn set(&mut self, cutoff: f32, resonance: f32, drive: f32, compensation: f32, rate: f32) {
        let rate2 = rate * 2.0;
        let fc = cutoff.clamp(5.0, rate2 * 0.22);
        let w = std::f32::consts::TAU * fc / rate2;
        // The loop's delay puts its resonance above the stage frequency
        // by 1 + 0.24 w − 0.1 w² (from the model's phase); the stages are
        // set lower by that, so it sings where the cutoff says.
        let w = w / (1.0 + 0.24 * w - 0.1 * w * w);
        self.core.g = 1.0 - (-w).exp();
        // The feedback it takes to oscillate also rises with the
        // frequency (4 at the bottom, 4.9 at 12 kHz); the resonance
        // control follows, so the top of its range sings everywhere.
        let k = MAX_FEEDBACK * resonance.clamp(0.0, 1.0) * (1.0 + 0.5 * w - 0.3 * w * w);
        self.core.k = k;
        self.core.gain = drive.max(0.0) * (1.0 + compensation.clamp(0.0, 1.0) * k);
        self.core.y.iter_mut().for_each(flush);
        flush(&mut self.core.y4_prev);
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let core = &mut self.core;
        self.over.run(x, |v| core.tick(v))
    }

    pub fn reset(&mut self) {
        self.core = Core {
            g: self.core.g,
            k: self.core.k,
            gain: self.core.gain,
            ..Core::default()
        };
        self.over.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    const RATE: f32 = 48_000.0;

    fn ladder(cutoff: f32, res: f32) -> Ladder {
        let mut l = Ladder::new();
        l.set(cutoff, res, 1.0, 0.0, RATE);
        l
    }

    #[test]
    fn it_rolls_off_at_twenty_four_decibels_an_octave() {
        let at = |hz| {
            let mut l = ladder(500.0, 0.0);
            measure::db(hz, RATE, |x| l.run(x * 0.1)) - measure::db(hz, RATE, |x| x * 0.1)
        };
        assert!(at(50.0).abs() < 0.5, "{}", at(50.0));
        let (a, b) = (at(4000.0), at(8000.0));
        assert!((a - b - 24.0).abs() < 1.5, "{a} {b}");
    }

    #[test]
    fn its_resonance_sits_at_the_cutoff_and_sings_at_the_top() {
        for cutoff in [100.0, 1000.0, 4000.0] {
            let mut l = ladder(cutoff, 1.0);
            let out: Vec<f32> = (0..RATE as usize)
                .map(|i| l.run(if i == 0 { 0.5 } else { 0.0 }))
                .collect();
            let hz = measure::ring_hz(&out, RATE);
            assert!((hz / cutoff - 1.0).abs() < 0.06, "{cutoff}: rings at {hz}");
            assert!(measure::peak(&out[40_000..]) > 0.1, "{cutoff}: died away");
        }
        // Below the top it rings and dies.
        let mut l = ladder(1000.0, 0.6);
        let out: Vec<f32> = (0..RATE as usize)
            .map(|i| l.run(if i == 0 { 0.5 } else { 0.0 }))
            .collect();
        assert!(measure::peak(&out[40_000..]) < 1e-3);
    }

    #[test]
    fn resonance_takes_the_bass_unless_compensated() {
        let mut flat = ladder(2000.0, 0.0);
        let mut res = ladder(2000.0, 0.7);
        let mut comp = Ladder::new();
        comp.set(2000.0, 0.7, 1.0, 1.0, RATE);
        let a = measure::db(100.0, RATE, |x| flat.run(x * 0.1));
        let b = measure::db(100.0, RATE, |x| res.run(x * 0.1));
        let c = measure::db(100.0, RATE, |x| comp.run(x * 0.1));
        assert!(a - b > 6.0, "{a} {b}");
        assert!((a - c).abs() < 1.0, "{a} {c}");
    }

    #[test]
    fn drive_adds_harmonics_without_aliasing_much() {
        let mut l = Ladder::new();
        l.set(8000.0, 0.2, 4.0, 0.0, RATE);
        let n = RATE as usize;
        let out: Vec<f32> = (0..n)
            .map(|i| l.run((std::f32::consts::TAU * 220.0 * i as f32 / RATE).sin()))
            .collect();
        let at = |hz: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, &v) in out.iter().enumerate().skip(n / 2) {
                let p = std::f32::consts::TAU * hz * i as f32 / RATE;
                re += v * p.cos();
                im += v * p.sin();
            }
            (re * re + im * im).sqrt() * 2.0 / (n / 2) as f32
        };
        let (f1, f3) = (at(220.0), at(660.0));
        assert!(f3 > 0.05 * f1, "third harmonic {f3} of {f1}");
        // Nothing much where no harmonic of 220 Hz lies.
        assert!(at(1000.0) < 0.02 * f1);
    }
}
