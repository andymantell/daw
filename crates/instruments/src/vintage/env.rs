// SPDX-License-Identifier: GPL-3.0-or-later
//! Envelopes with the curves and quirks of the analogue ones: a
//! capacitor charging towards a point above the peak (so the attack
//! starts fast and lands softly), exponential decay and release, an
//! attack that restarts from the level it is at, a decay shared by the
//! release (the monosynth's ADS), and the two-channel synth's ADR with
//! an initial level and an attack level. Also the rate/level envelope of
//! the FM synth: four rates and four levels, with the fourth pair the
//! release.

/// The curve of an envelope's attack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Curve {
    /// A capacitor charging: fast at first, soft at the top.
    #[default]
    Analog,
    /// A straight line.
    Linear,
}

/// What an envelope does, in seconds and levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Where it rests and starts from.
    pub initial: f32,
    /// Where the attack ends.
    pub peak: f32,
    pub attack: f32,
    pub decay: f32,
    /// Where the decay settles while the key is held.
    pub sustain: f32,
    pub release: f32,
    /// Where the release ends.
    pub release_to: f32,
    pub curve: Curve,
}

impl Shape {
    /// The usual four stages, 0 to 1.
    pub fn adsr(attack: f32, decay: f32, sustain: f32, release: f32) -> Shape {
        Shape {
            initial: 0.0,
            peak: 1.0,
            attack,
            decay,
            sustain: sustain.clamp(0.0, 1.0),
            release,
            release_to: 0.0,
            curve: Curve::Analog,
        }
    }

    /// The monosynth's contour: attack, decay, sustain, and a switch
    /// that makes the release take the decay's time; off, it is quick.
    pub fn ads(attack: f32, decay: f32, sustain: f32, release_on: bool) -> Shape {
        Shape::adsr(
            attack,
            decay,
            sustain,
            if release_on { decay } else { 0.004 },
        )
    }

    /// The two-channel synth's shape: from an initial level up to an
    /// attack level, then decaying back to the initial level while held,
    /// and released to it as well.
    pub fn adr(initial: f32, attack_level: f32, attack: f32, decay: f32, release: f32) -> Shape {
        Shape {
            initial,
            peak: attack_level,
            attack,
            decay,
            sustain: initial,
            release,
            release_to: initial,
            curve: Curve::Analog,
        }
    }

    pub fn linear(mut self) -> Shape {
        self.curve = Curve::Linear;
        self
    }
}

/// How far above the peak the attack charges towards, as a fraction of
/// the climb: what gives the analogue attack its shape.
const OVERSHOOT: f32 = 0.35;

/// A shape worked into per-sample steps at a sample rate; make one a
/// block, not a sample.
#[derive(Clone, Copy, Debug)]
pub struct Steps {
    shape: Shape,
    /// The attack's coefficient (analogue) or step (linear).
    attack: f32,
    target: f32,
    decay: f32,
    release: f32,
}

/// The per-sample factor that closes 99.9% of a gap in `seconds`.
fn approach(seconds: f32, rate: f32) -> f32 {
    1.0 - (-6.9078 / (seconds.max(0.0005) * rate)).exp()
}

impl Steps {
    pub fn new(shape: Shape, rate: f32) -> Steps {
        let climb = (shape.peak - shape.initial).max(1e-3);
        let (attack, target) = match shape.curve {
            Curve::Analog => {
                // Reaches the peak in the time by charging towards a
                // point OVERSHOOT of the climb above it.
                let left = OVERSHOOT / (1.0 + OVERSHOOT);
                let k = 1.0 - (left.ln() / (shape.attack.max(0.0005) * rate)).exp();
                (k, shape.peak + OVERSHOOT * climb)
            }
            Curve::Linear => (climb / (shape.attack.max(0.0005) * rate), shape.peak),
        };
        Steps {
            shape,
            attack,
            target,
            decay: approach(shape.decay, rate),
            release: approach(shape.release, rate),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

/// An envelope with analogue curves.
#[derive(Clone, Copy, Debug)]
pub struct Analog {
    stage: Stage,
    pub level: f32,
}

impl Default for Analog {
    fn default() -> Analog {
        Analog::IDLE
    }
}

impl Analog {
    pub const IDLE: Analog = Analog {
        stage: Stage::Idle,
        level: 0.0,
    };

    /// Rests at a shape's initial level.
    pub fn at_rest(shape: &Shape) -> Analog {
        Analog {
            stage: Stage::Idle,
            level: shape.release_to,
        }
    }

    /// Starts the attack from the level it is at.
    pub fn trigger(&mut self) {
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

    pub fn held(&self) -> bool {
        matches!(self.stage, Stage::Attack | Stage::Decay)
    }

    #[inline]
    pub fn next(&mut self, s: &Steps) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                match s.shape.curve {
                    Curve::Analog => self.level += (s.target - self.level) * s.attack,
                    Curve::Linear => self.level += s.attack,
                }
                if self.level >= s.shape.peak {
                    self.level = s.shape.peak;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level += (s.shape.sustain - self.level) * s.decay;
            }
            Stage::Release => {
                self.level += (s.shape.release_to - self.level) * s.release;
                if (self.level - s.shape.release_to).abs() < 1e-4 {
                    self.level = s.shape.release_to;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

/// The FM synth's envelope: four rates and four levels. From the fourth
/// level it moves to the first, second and third in turn, holds the
/// third while the key is down, and on release moves to the fourth.
/// Rates are the time to travel the whole range, so a short move is
/// quick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateLevel {
    /// Seconds to cross from 0 to 1 at each rate.
    pub rate: [f32; 4],
    pub level: [f32; 4],
}

/// The FM envelope's progress.
#[derive(Clone, Copy, Debug, Default)]
pub struct RateLevelEnv {
    /// 0 to 2 while held, 3 releasing, 4 idle.
    segment: u8,
    pub level: f32,
}

impl RateLevelEnv {
    pub fn trigger(&mut self, e: &RateLevel) {
        self.level = e.level[3];
        self.segment = 0;
    }

    pub fn release(&mut self) {
        if self.segment < 3 {
            self.segment = 3;
        }
    }

    pub fn active(&self) -> bool {
        self.segment < 4
    }

    #[inline]
    pub fn next(&mut self, e: &RateLevel, rate: f32) -> f32 {
        let seg = self.segment as usize;
        if seg >= 4 {
            return self.level;
        }
        let target = e.level[seg];
        let step = 1.0 / (e.rate[seg].max(0.0005) * rate);
        if (target - self.level).abs() <= step {
            self.level = target;
            // Segment 2 holds; 3 ends.
            if seg != 2 {
                self.segment += 1;
            }
            if self.segment == 3 && seg == 3 {
                self.segment = 4;
            }
        } else if target > self.level {
            self.level += step;
        } else {
            self.level -= step;
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 1000.0;

    fn run(e: &mut Analog, s: &Steps, n: usize) -> Vec<f32> {
        (0..n).map(|_| e.next(s)).collect()
    }

    #[test]
    fn the_analogue_attack_is_fast_first_then_soft_and_lands_on_time() {
        let s = Steps::new(Shape::adsr(0.1, 0.1, 0.5, 0.1), RATE);
        let mut e = Analog::IDLE;
        e.trigger();
        let x = run(&mut e, &s, 100);
        // Well past a straight line's third after a third of the time;
        // at the peak at the end.
        assert!(x[33] > 0.45, "{}", x[33]);
        assert!((x[99] - 1.0).abs() < 0.02, "{}", x[99]);
        assert!(x[98] < 1.0);
        let lin = Steps::new(Shape::adsr(0.1, 0.1, 0.5, 0.1).linear(), RATE);
        let mut e = Analog::IDLE;
        e.trigger();
        let y = run(&mut e, &lin, 100);
        assert!((y[49] - 0.5).abs() < 0.02);
    }

    #[test]
    fn it_decays_to_sustain_releases_to_rest_and_retriggers_from_where_it_is() {
        let s = Steps::new(Shape::adsr(0.01, 0.1, 0.5, 0.1), RATE);
        let mut e = Analog::IDLE;
        e.trigger();
        run(&mut e, &s, 10);
        run(&mut e, &s, 100);
        assert!((e.level - 0.5).abs() < 0.01, "{}", e.level);
        e.release();
        run(&mut e, &s, 50);
        let mid = e.level;
        assert!(mid > 0.0 && mid < 0.5);
        e.trigger();
        e.next(&s);
        assert!(e.level > mid);
        e.release();
        run(&mut e, &s, 150);
        assert!(!e.active());
        assert_eq!(e.level, 0.0);
    }

    #[test]
    fn ads_shares_the_decay_and_adr_rests_at_its_initial_level() {
        let on = Shape::ads(0.01, 0.3, 0.2, true);
        let off = Shape::ads(0.01, 0.3, 0.2, false);
        assert_eq!(on.release, 0.3);
        assert!(off.release < 0.01);
        let adr = Shape::adr(0.3, 0.9, 0.01, 0.05, 0.05);
        let s = Steps::new(adr, RATE);
        let mut e = Analog::at_rest(&adr);
        assert!((e.level - 0.3).abs() < 1e-6);
        e.trigger();
        let x = run(&mut e, &s, 12);
        let top = x.iter().fold(0.0f32, |m, v| m.max(*v));
        assert!((top - 0.9).abs() < 0.01, "{top}");
        run(&mut e, &s, 100);
        assert!((e.level - 0.3).abs() < 0.01);
        e.release();
        run(&mut e, &s, 100);
        assert!(!e.active());
        assert!((e.level - 0.3).abs() < 1e-3);
    }

    #[test]
    fn the_rate_level_envelope_walks_its_levels() {
        let e = RateLevel {
            rate: [0.1, 0.1, 0.1, 0.2],
            level: [1.0, 0.6, 0.8, 0.0],
        };
        let mut v = RateLevelEnv::default();
        v.trigger(&e);
        let x: Vec<f32> = (0..400).map(|_| v.next(&e, RATE)).collect();
        assert!((x[99] - 1.0).abs() < 0.02, "{}", x[99]);
        assert!((x[140] - 0.6).abs() < 0.02, "{}", x[140]);
        assert!((x[160] - 0.8).abs() < 0.02, "{}", x[160]);
        assert!((x[399] - 0.8).abs() < 1e-6);
        v.release();
        for _ in 0..159 {
            v.next(&e, RATE);
        }
        assert!(v.active());
        v.next(&e, RATE);
        v.next(&e, RATE);
        assert!(!v.active());
        assert_eq!(v.level, 0.0);
    }
}
