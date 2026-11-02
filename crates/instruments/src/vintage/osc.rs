// SPDX-License-Identifier: GPL-3.0-or-later
//! Oscillators as the old circuits had them: band-limited saw, pulse,
//! triangle and shark-tooth; hard sync that resets a slave on the
//! master's cycle with the jump band-limited; a pulse width that any
//! source can move; sub-oscillators divided down from the master so
//! they never drift apart; and drift, a fixed offset per voice plus a
//! slow wander, so voices never line up exactly.
//!
//! Jumps and corners are rounded by polyBLEP and polyBLAMP (Välimäki
//! and Huovilainen, 2007), placed at the exact fraction of a sample
//! where they fall. To round the sample before a jump, the output is
//! delayed by one sample.

use crate::dsp::Noise;

/// An oscillator's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Wave {
    Sine,
    Triangle,
    /// A triangle with a saw's step in it, between the two.
    Shark,
    #[default]
    Saw,
    /// A square of any width.
    Pulse,
}

impl Wave {
    pub const NAMES: &[&str] = &["Sine", "Triangle", "Shark tooth", "Saw", "Pulse"];

    pub fn from_index(i: f32) -> Wave {
        match i as u32 {
            0 => Wave::Sine,
            1 => Wave::Triangle,
            2 => Wave::Shark,
            4 => Wave::Pulse,
            _ => Wave::Saw,
        }
    }
}

/// The value of `wave` at `phase` with no rounding.
#[inline]
fn naive(wave: Wave, phase: f32, width: f32) -> f32 {
    match wave {
        Wave::Sine => (phase * std::f32::consts::TAU).sin(),
        Wave::Triangle => tri(phase),
        Wave::Shark => 0.5 * tri(phase) + 0.5 * (2.0 * phase - 1.0),
        Wave::Saw => 2.0 * phase - 1.0,
        Wave::Pulse => {
            if phase < width {
                1.0
            } else {
                -1.0
            }
        }
    }
}

#[inline]
fn tri(phase: f32) -> f32 {
    let x = 4.0 * phase;
    if phase < 0.5 { x - 1.0 } else { 3.0 - x }
}

/// Rounding for a jump of `amount` that happened `u` samples ago (0 to
/// 1): what to add to the sample before it, and to the one after.
#[inline]
fn step(amount: f32, u: f32) -> (f32, f32) {
    (0.5 * amount * u * u, -0.5 * amount * (1.0 - u) * (1.0 - u))
}

/// Rounding for a change of slope of `amount` a sample, `u` samples ago.
#[inline]
fn corner(amount: f32, u: f32) -> (f32, f32) {
    let v = 1.0 - u;
    (amount * u * u * u / 6.0, amount * v * v * v / 6.0)
}

/// One oscillator. Set `dt` (cycles a sample) and `width` as often as
/// you like; call [`Osc::tick`] once a sample.
#[derive(Clone, Copy, Debug)]
pub struct Osc {
    pub wave: Wave,
    pub phase: f32,
    /// Cycles a sample.
    pub dt: f32,
    /// The pulse's width, 0 to 1.
    pub width: f32,
    /// The last naive sample with its own rounding, waiting to go out.
    prev: f32,
    /// Rounding owed to the sample waiting to go out.
    pending: f32,
    /// How many samples ago the phase wrapped in the last tick, if it
    /// did: what sync slaves and sub-oscillators follow.
    pub wrapped: Option<f32>,
}

impl Default for Osc {
    fn default() -> Osc {
        Osc::new(Wave::Saw)
    }
}

impl Osc {
    pub fn new(wave: Wave) -> Osc {
        Osc {
            wave,
            phase: 0.0,
            dt: 0.0,
            width: 0.5,
            prev: 0.0,
            pending: 0.0,
            wrapped: None,
        }
    }

    pub fn set_hz(&mut self, hz: f32, rate: f32) {
        self.dt = (hz / rate).clamp(0.0, 0.5);
    }

    /// Starts at `phase`, forgetting any rounding owed.
    pub fn reset(&mut self, phase: f32) {
        self.phase = phase.rem_euclid(1.0);
        self.prev = naive(self.wave, self.phase, self.width);
        self.pending = 0.0;
        self.wrapped = None;
    }

    /// Adds the rounding for a jump from `before` to `after` in value,
    /// `u` samples ago.
    #[inline]
    fn jump(&mut self, before: f32, after: f32, u: f32, now: &mut f32) {
        let (b, a) = step(after - before, u);
        self.pending += b;
        *now += a;
    }

    /// Adds the rounding for a change of slope `u` samples ago, from
    /// `before` to `after` (per cycle).
    #[inline]
    fn bend(&mut self, before: f32, after: f32, u: f32, now: &mut f32) {
        let (b, a) = corner((after - before) * self.dt, u);
        self.pending += b;
        *now += a;
    }

    /// The slope of the wave per cycle at `phase`, where it has one.
    #[inline]
    fn slope(&self, phase: f32) -> f32 {
        match self.wave {
            Wave::Triangle => {
                if phase < 0.5 {
                    4.0
                } else {
                    -4.0
                }
            }
            Wave::Shark => {
                if phase < 0.5 {
                    3.0
                } else {
                    -1.0
                }
            }
            _ => 0.0,
        }
    }

    /// The discontinuities of the wave between `from` (exclusive) and
    /// `to` (inclusive) within one cycle, as rounding on `now`. `at`
    /// gives how many samples ago a phase inside the span was passed.
    #[inline]
    fn edges(&mut self, from: f32, to: f32, at: impl Fn(f32) -> f32, now: &mut f32) {
        match self.wave {
            Wave::Pulse if from < self.width && self.width <= to => {
                self.jump(1.0, -1.0, at(self.width), now);
            }
            Wave::Triangle | Wave::Shark if from < 0.5 && 0.5 <= to => {
                self.bend(self.slope(0.4), self.slope(0.6), at(0.5), now);
            }
            _ => {}
        }
    }

    /// The rounding for the wrap at phase 0, `u` samples ago.
    #[inline]
    fn wrap(&mut self, u: f32, now: &mut f32) {
        match self.wave {
            Wave::Saw => self.jump(1.0, -1.0, u, now),
            Wave::Pulse => self.jump(-1.0, 1.0, u, now),
            Wave::Triangle => self.bend(-4.0, 4.0, u, now),
            Wave::Shark => {
                self.jump(0.5, -0.5, u, now);
                self.bend(-1.0, 3.0, u, now);
            }
            Wave::Sine => {}
        }
    }

    /// Advances one sample and returns the output (one sample late).
    /// `sync` is the master's `wrapped` when this oscillator is its
    /// slave: the phase restarts where the master did.
    #[inline]
    pub fn tick(&mut self, sync: Option<f32>) -> f32 {
        let dt = self.dt;
        let old = self.phase;
        let mut p = old + dt;
        let mut now_phase_wrap = None;
        let mut now = 0.0;
        if let Some(us) = sync.filter(|_| dt > 0.0) {
            // Where this oscillator was when the master wrapped, us
            // samples ago, after any edges before that point.
            let reset_at = old + dt * (1.0 - us);
            if reset_at >= 1.0 {
                let u = (reset_at - 1.0) / dt + us;
                self.edges(old, 1.0, |ph| (p - ph) / dt, &mut now);
                self.wrap(u.min(1.0), &mut now);
                self.edges(0.0, reset_at - 1.0, |ph| (p - 1.0 - ph) / dt, &mut now);
            } else {
                self.edges(old, reset_at, |ph| (p - ph) / dt, &mut now);
            }
            let at_reset = reset_at.rem_euclid(1.0);
            let before = naive(self.wave, at_reset, self.width);
            let after = naive(self.wave, 0.0, self.width);
            self.jump(before, after, us, &mut now);
            let (sb, sa) = (self.slope(at_reset), self.slope(0.0));
            if sb != sa {
                self.bend(sb, sa, us, &mut now);
            }
            p = us * dt;
            self.edges(0.0, p, |ph| (p - ph) / dt, &mut now);
            now_phase_wrap = Some(us);
        } else if p >= 1.0 {
            p -= 1.0;
            let u = p / dt;
            self.edges(old, 1.0, |ph| (p + 1.0 - ph) / dt, &mut now);
            self.wrap(u, &mut now);
            self.edges(0.0, p, |ph| (p - ph) / dt, &mut now);
            now_phase_wrap = Some(u);
        } else {
            self.edges(old, p, |ph| (p - ph) / dt, &mut now);
        }
        self.phase = p;
        self.wrapped = now_phase_wrap;
        let out = self.prev + self.pending;
        self.prev = naive(self.wave, p, self.width) + now;
        self.pending = 0.0;
        out
    }
}

/// A sub-oscillator's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SubShape {
    /// A square an octave down.
    #[default]
    OctaveDown,
    /// A square two octaves down.
    TwoDown,
    /// A narrow pulse two octaves down (one cycle in four).
    TwoDownNarrow,
}

impl SubShape {
    pub const NAMES: &[&str] = &["1 octave down", "2 octaves down", "2 down, narrow"];

    pub fn from_index(i: f32) -> SubShape {
        match i as u32 {
            1 => SubShape::TwoDown,
            2 => SubShape::TwoDownNarrow,
            _ => SubShape::OctaveDown,
        }
    }
}

/// A sub-oscillator divided down from a master: it counts the master's
/// wraps, so it stays locked to it, with its edges rounded like the
/// master's.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sub {
    pub shape: SubShape,
    count: u8,
    level: f32,
    prev: f32,
    pending: f32,
}

impl Sub {
    pub fn new(shape: SubShape) -> Sub {
        Sub {
            shape,
            count: 0,
            level: Sub::level_for(shape, 0),
            prev: 0.0,
            pending: 0.0,
        }
    }

    fn level_for(shape: SubShape, count: u8) -> f32 {
        match shape {
            SubShape::OctaveDown => {
                if count.is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                }
            }
            SubShape::TwoDown => {
                if count % 4 < 2 {
                    1.0
                } else {
                    -1.0
                }
            }
            // Centred so it carries no offset into the filter.
            SubShape::TwoDownNarrow => {
                if count.is_multiple_of(4) {
                    1.5
                } else {
                    -0.5
                }
            }
        }
    }

    /// One sample on, given the master's `wrapped`.
    #[inline]
    pub fn tick(&mut self, wrapped: Option<f32>) -> f32 {
        let mut now = 0.0;
        if let Some(u) = wrapped {
            self.count = self.count.wrapping_add(1);
            let next = Sub::level_for(self.shape, self.count);
            if next != self.level {
                let (b, a) = step(next - self.level, u);
                self.pending += b;
                now += a;
                self.level = next;
            }
        }
        let out = self.prev + self.pending;
        self.prev = self.level + now;
        self.pending = 0.0;
        out
    }

    pub fn reset(&mut self) {
        *self = Sub::new(self.shape);
    }
}

/// The drift of one voice's oscillator: a fixed offset (no two voices
/// were ever trimmed alike) and a slow wander (warmth, supply ripple).
/// The amount is the synth's "vintage" control: 1.0 is a well-worn
/// instrument, about ±4 cents fixed and ±3 cents wandering.
#[derive(Clone, Copy, Debug)]
pub struct Drift {
    offset: f32,
    walk: f32,
    noise: Noise,
}

impl Drift {
    pub fn new(seed: u32) -> Drift {
        let mut noise = Noise::new(seed.wrapping_mul(2_654_435_761).wrapping_add(1));
        let offset = noise.tick();
        Drift {
            offset,
            walk: 0.0,
            noise,
        }
    }

    /// Advances `samples` and returns the drift in cents at `amount`.
    #[inline]
    pub fn next(&mut self, amount: f32, samples: f32, rate: f32) -> f32 {
        // A random walk pulled back by a two-second time constant, so
        // it wanders about ±1 whatever the block size.
        let a = (samples / (rate * 2.0)).min(1.0);
        self.walk += a.sqrt() * 0.7 * self.noise.tick() - a * self.walk;
        self.walk = self.walk.clamp(-1.5, 1.5);
        amount * (4.0 * self.offset + 3.0 * self.walk)
    }

    /// The slow wander alone, −1 to 1, for a digitally controlled
    /// oscillator whose pitch holds but whose width and level breathe.
    pub fn slow(&self) -> f32 {
        self.walk
    }

    /// The fixed offset, −1 to 1.
    pub fn offset(&self) -> f32 {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// The level of harmonic `h` of `hz` in `x`.
    fn harmonic(x: &[f32], hz: f32, h: u32) -> f32 {
        let f = hz * h as f32;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &v) in x.iter().enumerate() {
            let p = std::f32::consts::TAU * f * i as f32 / RATE;
            re += v * p.cos();
            im += v * p.sin();
        }
        (re * re + im * im).sqrt() * 2.0 / x.len() as f32
    }

    fn render(wave: Wave, hz: f32, n: usize) -> Vec<f32> {
        let mut o = Osc::new(wave);
        o.set_hz(hz, RATE);
        (0..n).map(|_| o.tick(None)).collect()
    }

    #[test]
    fn the_saw_has_the_harmonics_of_a_saw_and_little_aliasing() {
        // 1000 Hz divides 48 kHz, so the harmonics land on bins.
        let x = render(Wave::Saw, 1000.0, 48_000);
        let f1 = harmonic(&x, 1000.0, 1);
        assert!((f1 - 2.0 / std::f32::consts::PI).abs() < 0.02, "{f1}");
        assert!((harmonic(&x, 1000.0, 2) / f1 - 0.5).abs() < 0.03);
        assert!((harmonic(&x, 1000.0, 5) / f1 - 0.2).abs() < 0.03);
        // Between harmonics there is almost nothing.
        assert!(harmonic(&x, 1500.0, 1) < 0.01 * f1);
        // A naive saw at a high pitch puts energy between harmonics.
        let x = render(Wave::Saw, 5000.0, 48_000);
        let naive: Vec<f32> = (0..48_000)
            .map(|i| 2.0 * (i as f32 * 5000.0 / RATE).fract() - 1.0)
            .collect();
        assert!(harmonic(&x, 1000.0, 1) < 0.1 * harmonic(&naive, 1000.0, 1));
    }

    #[test]
    fn the_pulse_and_triangle_are_shaped_right() {
        let x = render(Wave::Pulse, 1000.0, 48_000);
        let f1 = harmonic(&x, 1000.0, 1);
        assert!((f1 - 4.0 / std::f32::consts::PI).abs() < 0.03, "{f1}");
        assert!(harmonic(&x, 1000.0, 2) < 0.02 * f1);
        assert!((harmonic(&x, 1000.0, 3) / f1 - 1.0 / 3.0).abs() < 0.02);
        let x = render(Wave::Triangle, 1000.0, 48_000);
        let f1 = harmonic(&x, 1000.0, 1);
        assert!((harmonic(&x, 1000.0, 3) / f1 - 1.0 / 9.0).abs() < 0.01);
        assert!(harmonic(&x, 1000.0, 2) < 0.01 * f1);
        let mut o = Osc::new(Wave::Pulse);
        o.set_hz(100.0, RATE);
        o.width = 0.25;
        let x: Vec<f32> = (0..4800).map(|_| o.tick(None)).collect();
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        assert!((mean + 0.5).abs() < 0.02, "{mean}");
    }

    #[test]
    fn a_synced_slave_rings_at_the_masters_pitch() {
        let mut m = Osc::new(Wave::Saw);
        let mut s = Osc::new(Wave::Saw);
        m.set_hz(200.0, RATE);
        s.set_hz(530.0, RATE);
        let x: Vec<f32> = (0..48_000)
            .map(|_| {
                m.tick(None);
                s.tick(m.wrapped)
            })
            .collect();
        // Energy at the master's harmonics, not the slave's own pitch.
        assert!(harmonic(&x, 200.0, 1) > 0.1);
        assert!(harmonic(&x, 200.0, 3) > 0.1);
        assert!(harmonic(&x, 530.0, 1) < 0.1);
        // Band-limited: between the master's harmonics, little.
        assert!(harmonic(&x, 100.0, 1) < 0.01);
        assert!(harmonic(&x, 300.0, 1) < 0.01);
    }

    #[test]
    fn subs_divide_the_master_down() {
        for (shape, div) in [
            (SubShape::OctaveDown, 2),
            (SubShape::TwoDown, 4),
            (SubShape::TwoDownNarrow, 4),
        ] {
            let mut m = Osc::new(Wave::Saw);
            m.set_hz(400.0, RATE);
            let mut sub = Sub::new(shape);
            let x: Vec<f32> = (0..48_000)
                .map(|_| {
                    m.tick(None);
                    sub.tick(m.wrapped)
                })
                .collect();
            let hz = 400.0 / div as f32;
            assert!(harmonic(&x, hz, 1) > 0.4, "{shape:?}");
            let mean = x.iter().sum::<f32>() / x.len() as f32;
            assert!(mean.abs() < 0.02, "{shape:?} {mean}");
        }
    }

    #[test]
    fn drift_differs_by_voice_and_wanders_within_bounds() {
        let mut a = Drift::new(1);
        let mut b = Drift::new(2);
        assert_ne!(a.offset(), b.offset());
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for _ in 0..10_000 {
            let c = a.next(1.0, 64.0, RATE);
            b.next(1.0, 64.0, RATE);
            lo = lo.min(c);
            hi = hi.max(c);
        }
        assert!(hi - lo > 0.5, "{lo} {hi}");
        assert!(lo > -7.5 && hi < 7.5);
        assert_eq!(a.next(0.0, 64.0, RATE), 0.0);
    }
}
