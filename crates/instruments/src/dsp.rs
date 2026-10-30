// SPDX-License-Identifier: GPL-3.0-or-later
//! Building blocks the instruments share: an envelope, a filter, noise
//! and band-limited oscillators. Each follows a published method, noted
//! where it is used.

use std::f32::consts::{PI, TAU};

/// An attack/decay/sustain/release envelope, 0 to 1. Attack rises in a
/// straight line; decay and release fall exponentially, reaching
/// −60 dB in the time set, which is how they sound even.
#[derive(Clone, Copy, Debug)]
pub struct Env {
    stage: Stage,
    pub level: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

/// Times in seconds and the sustain level, worked into per-sample steps
/// at a sample rate.
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
}

/// The factor a level falls by each sample to drop 60 dB in `seconds`.
pub fn fall(seconds: f32, rate: f32) -> f32 {
    (-6.9078 / (seconds.max(0.0005) * rate)).exp()
}

impl Adsr {
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32, rate: f32) -> Adsr {
        Adsr {
            attack: 1.0 / (attack.max(0.0005) * rate),
            decay: fall(decay, rate),
            sustain: sustain.clamp(0.0, 1.0),
            release: fall(release, rate),
        }
    }
}

impl Env {
    pub const IDLE: Env = Env {
        stage: Stage::Idle,
        level: 0.0,
    };

    /// Starts again from where it is, so a retriggered voice doesn't
    /// click.
    pub fn start(&mut self) {
        self.stage = Stage::Attack;
    }

    pub fn release(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    pub fn active(&self) -> bool {
        self.stage != Stage::Idle
    }

    pub fn releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    #[inline]
    pub fn next(&mut self, a: &Adsr) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level += a.attack;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                // Falls towards the sustain level.
                self.level = a.sustain + (self.level - a.sustain) * a.decay;
            }
            Stage::Release => {
                self.level *= a.release;
                if self.level < 1e-4 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

/// Which response of the filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Low,
    Band,
    High,
}

impl Mode {
    pub fn from_index(i: f32) -> Mode {
        match i as u32 {
            1 => Mode::Band,
            2 => Mode::High,
            _ => Mode::Low,
        }
    }
}

/// A state-variable filter, by the trapezoidal-integration form Andrew
/// Simper published ("Linear Trapezoidal Integrated SVF", Cytomic,
/// 2013): stable while its cutoff moves, which a synth's filter does
/// all the time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

/// A filter at rest, before [`Svf::set`].
pub const SVF_ZERO: Svf = Svf {
    ic1: 0.0,
    ic2: 0.0,
    a1: 0.0,
    a2: 0.0,
    a3: 0.0,
    k: 2.0,
};

impl Svf {
    /// Sets the cutoff (Hz) and resonance (0 to 1; near 1 it rings).
    pub fn set(&mut self, cutoff: f32, resonance: f32, rate: f32) {
        let fc = cutoff.clamp(10.0, rate * 0.49);
        let g = (PI * fc / rate).tan();
        // Q from 0.5 to about 25.
        let k = 2.0 - 1.96 * resonance.clamp(0.0, 1.0);
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    pub fn run(&mut self, v0: f32, mode: Mode) -> f32 {
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        match mode {
            Mode::Low => v2,
            Mode::Band => v1,
            Mode::High => v0 - self.k * v1 - v2,
        }
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

/// White noise from a 32-bit xorshift generator (Marsaglia, 2003).
#[derive(Clone, Copy, Debug)]
pub struct Noise(u32);

impl Noise {
    pub fn new(seed: u32) -> Noise {
        Noise(seed | 1)
    }

    /// −1 to 1.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// An oscillator's waveform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Triangle,
    Saw,
    Square,
    /// A square of any width.
    Pulse,
}

impl Wave {
    pub const NAMES: &[&str] = &["Sine", "Triangle", "Saw", "Square", "Pulse"];

    pub fn from_index(i: f32) -> Wave {
        match i as u32 {
            0 => Wave::Sine,
            1 => Wave::Triangle,
            3 => Wave::Square,
            4 => Wave::Pulse,
            _ => Wave::Saw,
        }
    }
}

/// One sample of `wave` at `phase` (0 to 1) stepping `dt` a sample, with
/// the saw's and pulse's jumps smoothed by polyBLEP (Välimäki and
/// Huovilainen, 2007) so high notes don't alias badly. `width` is the
/// pulse's, 0 to 1.
#[inline]
pub fn osc(wave: Wave, phase: f32, dt: f32, width: f32) -> f32 {
    match wave {
        Wave::Sine => (phase * TAU).sin(),
        Wave::Triangle => {
            let x = 4.0 * phase;
            if phase < 0.5 { x - 1.0 } else { 3.0 - x }
        }
        Wave::Saw => 2.0 * phase - 1.0 - blep(phase, dt),
        Wave::Square => pulse(phase, dt, 0.5),
        Wave::Pulse => pulse(phase, dt, width.clamp(0.02, 0.98)),
    }
}

#[inline]
fn pulse(phase: f32, dt: f32, w: f32) -> f32 {
    let s = if phase < w { 1.0 } else { -1.0 };
    let down = (phase - w).rem_euclid(1.0);
    // Centred, so a narrow pulse doesn't shift the level.
    s + blep(phase, dt) - blep(down, dt) - (2.0 * w - 1.0)
}

/// The correction that rounds off a jump at phase 0.
#[inline]
pub fn blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Left and right gains for a pan of −1 to 1, at constant power.
#[inline]
pub fn pan(p: f32) -> (f32, f32) {
    let a = (p.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    // Scaled so the centre leaves the level as it is.
    (
        a.cos() * std::f32::consts::SQRT_2,
        a.sin() * std::f32::consts::SQRT_2,
    )
}

pub fn db(v: f32) -> f32 {
    if v <= -60.0 {
        0.0
    } else {
        10f32.powf(v / 20.0)
    }
}

/// A frequency for a pitch in semitones (69 is 440 Hz).
#[inline]
pub fn hz(pitch: f32) -> f32 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_envelope_rises_holds_and_falls_away() {
        let a = Adsr::new(0.01, 0.1, 0.5, 0.1, 1000.0);
        let mut e = Env::IDLE;
        e.start();
        for _ in 0..10 {
            e.next(&a);
        }
        assert!((e.level - 1.0).abs() < 1e-3);
        for _ in 0..1000 {
            e.next(&a);
        }
        assert!((e.level - 0.5).abs() < 1e-3);
        e.release();
        for _ in 0..200 {
            e.next(&a);
        }
        assert!(!e.active());
    }

    #[test]
    fn the_filter_passes_its_band_and_cuts_the_rest() {
        let rate = 48_000.0;
        let level = |mode: Mode, f: f32| {
            let mut s = Svf::default();
            s.set(1000.0, 0.0, rate);
            let mut peak = 0.0f32;
            for i in 0..48_000 {
                let y = s.run((TAU * f * i as f32 / rate).sin(), mode);
                if i > 24_000 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };
        assert!(level(Mode::Low, 100.0) > 0.9);
        assert!(level(Mode::Low, 10_000.0) < 0.05);
        assert!(level(Mode::High, 100.0) < 0.05);
        assert!(level(Mode::High, 10_000.0) > 0.9);
        assert!(level(Mode::Band, 1000.0) > 0.45);
    }

    #[test]
    fn centre_pan_leaves_the_level() {
        let (l, r) = pan(0.0);
        assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        let (l, r) = pan(-1.0);
        assert!(r.abs() < 1e-6 && l > 1.0);
    }
}
