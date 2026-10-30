// SPDX-License-Identifier: GPL-3.0-or-later
//! Drum sounds made from oscillators and noise, the way analogue drum
//! machines made them: a kick is a sine falling fast in pitch, a snare
//! a pair of tones with filtered noise, a hi-hat a cluster of square
//! waves at unrelated pitches through a high-pass filter, and so on.
//! None of it is sampled, so they ship with the app freely.

use std::f32::consts::TAU;

use crate::dsp::{Mode, Noise, Svf, fall};

/// Which drum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drum {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Tom,
    Rim,
    Cymbal,
    Cowbell,
}

impl Drum {
    pub const ALL: [Drum; 9] = [
        Drum::Kick,
        Drum::Snare,
        Drum::Clap,
        Drum::ClosedHat,
        Drum::OpenHat,
        Drum::Tom,
        Drum::Rim,
        Drum::Cymbal,
        Drum::Cowbell,
    ];

    pub const NAMES: &[&str] = &[
        "Kick",
        "Snare",
        "Clap",
        "Closed hi-hat",
        "Open hi-hat",
        "Tom",
        "Rim",
        "Cymbal",
        "Cowbell",
    ];

    pub fn name(self) -> &'static str {
        Drum::NAMES[self as usize]
    }

    pub fn from_index(i: usize) -> Option<Drum> {
        Drum::ALL.get(i).copied()
    }

    /// How long it rings by default, in seconds, to −60 dB.
    pub fn decay(self) -> f32 {
        match self {
            Drum::Kick => 0.6,
            Drum::Snare => 0.3,
            Drum::Clap => 0.35,
            Drum::ClosedHat => 0.08,
            Drum::OpenHat => 0.6,
            Drum::Tom => 0.6,
            Drum::Rim => 0.06,
            Drum::Cymbal => 2.0,
            Drum::Cowbell => 0.35,
        }
    }
}

/// The ratios of the six square waves in a metallic sound, as analogue
/// machines tuned them: unrelated enough that no pitch is heard.
const METAL: [f32; 6] = [2.0, 3.0, 4.16, 5.43, 6.79, 8.21];

/// One drum hit sounding.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    drum: Drum,
    rate: f32,
    /// Samples since it started.
    t: u32,
    /// Pitch, as a factor on the drum's own.
    tune: f32,
    /// Per-sample fall of the main level, and of the noise.
    fall: f32,
    fall_noise: f32,
    /// 0 to 1: brighter, more snap.
    tone: f32,
    level: f32,
    noise_level: f32,
    phase: [f32; 6],
    noise: Noise,
    filter: Svf,
    filter2: Svf,
    hp: f32,
    pub done: bool,
}

impl Hit {
    /// A hit of `drum`, `tune` semitones from its own pitch, ringing for
    /// `decay` seconds (to −60 dB) with `tone` from 0 to 1.
    pub fn new(drum: Drum, rate: f32, tune: f32, decay: f32, tone: f32, seed: u32) -> Hit {
        let tune = (tune / 12.0).exp2();
        let tone = tone.clamp(0.0, 1.0);
        let mut filter = Svf::default();
        let mut filter2 = Svf::default();
        let (noise_decay, f1, f2) = match drum {
            Drum::Snare => (decay, 1800.0 + 4000.0 * tone, 0.0),
            Drum::Clap => (decay, 1200.0 * tune, 0.0),
            Drum::ClosedHat | Drum::OpenHat => (decay, 7000.0 + 3000.0 * tone, 10_000.0),
            Drum::Cymbal => (decay, 5000.0 + 3000.0 * tone, 9000.0),
            Drum::Rim => (decay, 1700.0 * tune, 0.0),
            Drum::Cowbell => (decay, 2600.0 * tune, 0.0),
            Drum::Kick | Drum::Tom => (0.01 + 0.02 * tone, 3000.0, 0.0),
        };
        let res = match drum {
            Drum::Clap | Drum::Rim | Drum::Cowbell => 0.5,
            _ => 0.2,
        };
        filter.set(f1, res, rate);
        if f2 > 0.0 {
            filter2.set(f2, 0.3, rate);
        }
        Hit {
            drum,
            rate,
            t: 0,
            tune,
            fall: fall(decay.max(0.005), rate),
            fall_noise: fall(noise_decay.max(0.005), rate),
            tone,
            level: 1.0,
            noise_level: 1.0,
            phase: [0.0; 6],
            noise: Noise::new(seed.wrapping_mul(2_654_435_761).wrapping_add(17)),
            filter,
            filter2,
            hp: 0.0,
            done: false,
        }
    }

    /// The next sample.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.done {
            return 0.0;
        }
        let secs = self.t as f32 / self.rate;
        self.t += 1;
        let r = self.rate;
        let out = match self.drum {
            Drum::Kick => {
                // A sine sweeping from well above its pitch down to it.
                let f_end = 50.0 * self.tune;
                let f_start = f_end * (2.5 + 2.0 * self.tone);
                let f = f_end + (f_start - f_end) * (-secs / 0.035).exp();
                let s = self.sine(0, f);
                let click = self.noise.tick() * self.noise_level * 0.4 * self.tone;
                self.noise_level *= self.fall_noise;
                s * self.level + click
            }
            Drum::Tom => {
                let f_end = 110.0 * self.tune;
                let f = f_end * (1.0 + 0.6 * (-secs / 0.05).exp());
                let s = self.sine(0, f);
                let n = self.noise.tick() * self.noise_level * 0.15 * self.tone;
                self.noise_level *= self.fall_noise;
                s * self.level + n
            }
            Drum::Snare => {
                // Two tones that die quickly, and noise that rings on.
                let body =
                    0.6 * self.sine(0, 185.0 * self.tune) + 0.4 * self.sine(1, 330.0 * self.tune);
                let body = body * self.level.powi(3);
                let n = self.noise.tick();
                let n = self.filter.run(n, Mode::High) * self.noise_level;
                self.noise_level *= self.fall_noise;
                (body * (1.0 - 0.5 * self.tone) + n * (0.6 + 0.6 * self.tone)) * 0.65
            }
            Drum::Clap => {
                // Three quick bursts, then the tail.
                let n = self.filter.run(self.noise.tick(), Mode::Band) * 2.5;
                let burst = |at: f32| {
                    let d = secs - at;
                    if d >= 0.0 { (-d / 0.004).exp() } else { 0.0 }
                };
                let bursts = burst(0.0) + burst(0.011) + burst(0.022);
                let tail = if secs >= 0.03 { self.noise_level } else { 0.0 };
                if secs >= 0.03 {
                    self.noise_level *= self.fall_noise;
                }
                n * (bursts * 0.8 + tail)
            }
            Drum::ClosedHat | Drum::OpenHat | Drum::Cymbal => {
                let base = if self.drum == Drum::Cymbal {
                    60.0
                } else {
                    40.0
                } * self.tune;
                let mut m = 0.0;
                for (k, ratio) in METAL.iter().enumerate() {
                    m += self.square(k, base * ratio);
                }
                let m = m / 6.0 * (1.0 - 0.5 * self.tone) + self.noise.tick() * 0.5 * self.tone;
                let m = self.filter.run(m, Mode::High);
                let m = self.filter2.run(m, Mode::Low);
                self.noise_level *= self.fall_noise;
                let gain = if self.drum == Drum::Cymbal { 4.0 } else { 6.0 };
                m * self.noise_level * gain
            }
            Drum::Rim => {
                let s = self.sine(0, 480.0 * self.tune) * 0.5;
                let n = self.filter.run(self.noise.tick() + s, Mode::Band);
                self.noise_level *= self.fall_noise;
                n * self.level * 2.0
            }
            Drum::Cowbell => {
                let s = self.square(0, 540.0 * self.tune) + self.square(1, 800.0 * self.tune);
                let s = self.filter.run(s * 0.5, Mode::Band);
                // A sharp start, then a slower fall.
                let shape = 0.6 * (-secs / 0.02).exp() + 0.4;
                self.noise_level *= self.fall_noise;
                s * self.level * shape * 1.2
            }
        };
        self.level *= self.fall;
        // A gentle high-pass to keep the sweep's offset out.
        self.hp += (out - self.hp) * (20.0 * TAU / r).min(1.0);
        let out = out - self.hp;
        if self.level < 1e-4 && self.noise_level < 1e-4 {
            self.done = true;
        }
        out
    }

    #[inline]
    fn sine(&mut self, k: usize, f: f32) -> f32 {
        self.phase[k] = (self.phase[k] + f / self.rate).fract();
        (self.phase[k] * TAU).sin()
    }

    #[inline]
    fn square(&mut self, k: usize, f: f32) -> f32 {
        self.phase[k] = (self.phase[k] + f / self.rate).fract();
        if self.phase[k] < 0.5 { 1.0 } else { -1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drum_sounds_and_stops_within_its_decay() {
        let rate = 48_000.0;
        for d in Drum::ALL {
            let mut h = Hit::new(d, rate, 0.0, d.decay(), 0.5, 1);
            let x: Vec<f32> = (0..(rate * 4.0) as usize).map(|_| h.tick()).collect();
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.05 && peak < 4.0, "{d:?}: {peak}");
            assert!(h.done, "{d:?} rang on");
            assert!(x.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn tuning_a_kick_up_raises_its_pitch() {
        let crossings = |tune: f32| {
            let mut h = Hit::new(Drum::Kick, 48_000.0, tune, 1.0, 0.0, 1);
            let x: Vec<f32> = (0..24_000).map(|_| h.tick()).collect();
            x[4800..]
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };
        assert!(crossings(12.0) > crossings(0.0) * 3 / 2);
    }
}
