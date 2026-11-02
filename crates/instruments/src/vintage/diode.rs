// SPDX-License-Identifier: GPL-3.0-or-later
//! The four-pole diode ladder of the acid bass synth. Unlike the
//! transistor ladder, nothing buffers its stages: each capacitor sits
//! between two stages, so every stage is loaded by the next and the
//! current flows both ways along the ladder. That spreads the poles
//! apart, which gives it its softer resonance peak and a slope that
//! starts gently (close to 18 dB an octave over the first octaves,
//! which is how it is often mislabelled) before reaching 24 dB. Its
//! resonance needs far more feedback than the transistor ladder's, and
//! a capacitor in the feedback path makes it a high-pass, so the bass
//! thins as the resonance rises: the sound of that synth.
//!
//! The stages are solved together each sample (the chain is a
//! tridiagonal system in the zero-delay feedback form, Zavalishin's
//! method), with the feedback's diode clipping solved by Newton's method
//! around it; the whole runs at twice the sample rate.

use super::onepole::coefficient;
use super::oversample::Over2;
use super::saturate::{flush, soft, soft_pair};

/// The loop's phase passes 180° at 1.195 times the stage frequency
/// (measured from the model), so the stages are set below the cutoff
/// asked for.
const TUNE: f32 = 1.0 / 1.195;
/// The loop gain at that point is 1/18.4, so resonance 1.0 is a little
/// past self-oscillation.
const MAX_FEEDBACK: f32 = 22.0;
/// Where the feedback diodes clip, relative to full scale: the feedback
/// signal is limited to this, which sets how loud the resonance can get.
const CLIP: f32 = 4.0;

#[derive(Clone, Copy, Debug, Default)]
struct Core {
    s: [f32; 4],
    /// The feedback high-pass's state.
    sh: f32,
    g: f32,
    /// The high-pass's `G`.
    gh: f32,
    k: f32,
    gain: f32,
    /// The last stage's output for the states alone, and per unit of
    /// input, each sample.
    y_zero: [f32; 4],
    y_unit: [f32; 4],
    /// The elimination's reciprocal pivots and multipliers, which depend
    /// only on `g`.
    inv: [f32; 4],
    mul: [f32; 4],
}

impl Core {
    /// Works out the elimination's pivots and multipliers for `g`.
    fn prepare(&mut self, g: f32) {
        // Rows: (1+2g) y1 - g y2 = s1 + g u; -g y_{i-1} + (1+2g) y_i - g y_{i+1} = s_i;
        // -g y3 + (1+g) y4 = s4.
        let mut d = [1.0 + 2.0 * g, 1.0 + 2.0 * g, 1.0 + 2.0 * g, 1.0 + g];
        self.mul[0] = 0.0;
        for i in 1..4 {
            let m = -g / d[i - 1];
            d[i] -= m * -g;
            self.mul[i] = m;
        }
        for (inv, d) in self.inv.iter_mut().zip(d) {
            *inv = 1.0 / d;
        }
        self.g = g;
    }

    /// Solves the ladder for the stage outputs, given its states `s` and
    /// an input `u` (the tridiagonal system by elimination, with the
    /// pivots prepared).
    #[inline]
    fn solve(&self, s: [f32; 4], u: f32) -> [f32; 4] {
        let g = self.g;
        let mut r = [s[0] + g * u, s[1], s[2], s[3]];
        for i in 1..4 {
            r[i] -= self.mul[i] * r[i - 1];
        }
        let mut y = [0.0; 4];
        y[3] = r[3] * self.inv[3];
        for i in (0..3).rev() {
            y[i] = (r[i] + g * y[i + 1]) * self.inv[i];
        }
        y
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let x = soft(x * self.gain);
        self.y_zero = self.solve(self.s, 0.0);
        let (y0, yu) = (self.y_zero[3], self.y_unit[3]);
        // The feedback is the diode-clipped high-pass of the last stage:
        // hp(y4) = (1 − Gh)(y4 − sh). Solve y4 = y0 + yu (x − soft(k hp)).
        let hp_of = |y4: f32| (1.0 - self.gh) * (y4 - self.sh);
        let clip = |a: f32| CLIP * soft(a / CLIP);
        let mut y4 = y0 + yu * (x - clip(self.k * hp_of(y0)));
        for _ in 0..2 {
            let (sat, slope) = soft_pair(self.k * hp_of(y4) / CLIP);
            let f = y4 - y0 - yu * (x - CLIP * sat);
            let df = 1.0 + yu * self.k * (1.0 - self.gh) * slope;
            y4 -= f / df;
        }
        let u = x - clip(self.k * hp_of(y4));
        for i in 0..4 {
            let y = self.y_zero[i] + self.y_unit[i] * u;
            self.s[i] = 2.0 * y - self.s[i];
        }
        let y4 = self.y_zero[3] + yu * u;
        let lp = self.gh * y4 + (1.0 - self.gh) * self.sh;
        self.sh = 2.0 * lp - self.sh;
        y4
    }
}

/// The diode ladder, oversampled.
#[derive(Clone, Copy, Debug, Default)]
pub struct Diode {
    core: Core,
    over: Over2,
}

impl Diode {
    pub fn new() -> Diode {
        Diode::default()
    }

    /// Sets the cutoff (where it resonates) in Hz, the resonance (0 to 1,
    /// singing from about 0.95), the drive into the ladder, the corner
    /// of the high-pass in the feedback path in Hz (the circuit's is
    /// low; higher thins the bass more as resonance rises) and the bass
    /// compensation (0 as the circuit is, 1 all of it).
    pub fn set(
        &mut self,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        feedback_hp: f32,
        compensation: f32,
        rate: f32,
    ) {
        let rate2 = rate * 2.0;
        let c = &mut self.core;
        let hp = feedback_hp.max(1.0);
        c.gh = coefficient(hp, rate2).1;
        // The high-pass in the loop adds phase lead at the cutoff, which
        // lifts the point the ladder oscillates at by 1 + 0.73 lead and
        // the feedback it takes by 1 + 1.55 lead (from the model). Both
        // are put back, so low notes sing where and as readily as high.
        let fc = cutoff.max(5.0);
        let lead = (hp / fc).atan();
        let (g, _) = coefficient(fc * TUNE / (1.0 + 0.73 * lead), rate2);
        c.prepare(g);
        c.k = MAX_FEEDBACK * resonance.clamp(0.0, 1.0) * (1.0 + 1.55 * lead);
        c.s.iter_mut().for_each(flush);
        flush(&mut c.sh);
        c.gain = drive.max(0.0) * (1.0 + compensation.clamp(0.0, 1.0) * c.k * 0.5);
        c.y_unit = c.solve([0.0; 4], 1.0);
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let core = &mut self.core;
        self.over.run(x, |v| core.tick(v))
    }

    pub fn reset(&mut self) {
        self.core.s = [0.0; 4];
        self.core.sh = 0.0;
        self.over.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    const RATE: f32 = 48_000.0;

    fn diode(cutoff: f32, res: f32) -> Diode {
        let mut d = Diode::new();
        d.set(cutoff, res, 1.0, 20.0, 0.0, RATE);
        d
    }

    #[test]
    fn it_starts_gently_and_ends_at_twenty_four_decibels_an_octave() {
        let at = |hz| {
            let mut d = diode(1000.0, 0.0);
            measure::db(hz, RATE, |x| d.run(x * 0.1)) - measure::db(hz, RATE, |x| x * 0.1)
        };
        assert!(at(30.0).abs() < 0.5, "{}", at(30.0));
        let near = at(2000.0) - at(4000.0);
        let far = at(8000.0) - at(16_000.0);
        assert!((16.0..21.0).contains(&near), "{near}");
        assert!(far > 21.5, "{far}");
    }

    #[test]
    fn its_resonance_sits_at_the_cutoff_and_sings_at_the_top() {
        for cutoff in [100.0, 1000.0, 5000.0] {
            let mut d = diode(cutoff, 1.0);
            let out: Vec<f32> = (0..RATE as usize)
                .map(|i| d.run(if i == 0 { 0.5 } else { 0.0 }))
                .collect();
            let hz = measure::ring_hz(&out, RATE);
            assert!((hz / cutoff - 1.0).abs() < 0.08, "{cutoff}: rings at {hz}");
            assert!(measure::peak(&out[40_000..]) > 0.05, "{cutoff}: died away");
        }
        let mut d = diode(1000.0, 0.5);
        let out: Vec<f32> = (0..RATE as usize)
            .map(|i| d.run(if i == 0 { 0.5 } else { 0.0 }))
            .collect();
        assert!(measure::peak(&out[40_000..]) < 1e-3);
    }

    #[test]
    fn the_feedback_high_pass_keeps_the_lowest_bass_but_thins_above_it() {
        let lost = |hz: f32| {
            let mut flat = Diode::new();
            flat.set(1000.0, 0.0, 1.0, 100.0, 0.0, RATE);
            let mut res = Diode::new();
            res.set(1000.0, 0.8, 1.0, 100.0, 0.0, RATE);
            measure::db(hz, RATE, |x| flat.run(x * 0.1))
                - measure::db(hz, RATE, |x| res.run(x * 0.1))
        };
        let (low, mid) = (lost(10.0), lost(300.0));
        assert!(low < 8.0, "{low}");
        assert!(mid > 12.0 && mid > low + 6.0, "{low} {mid}");
    }
}
