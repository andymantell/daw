// SPDX-License-Identifier: GPL-3.0-or-later
//! A transient shaper: makes the start of each note or hit louder or
//! softer, and separately what rings on after it, whatever the level.
//!
//! It follows the level twice, once quickly and once slowly, the method
//! widely described for these (for example in U. Zölzer (ed.), "DAFX",
//! chapter on dynamics processing). While a note starts, the quick
//! follower is above the slow one; while it rings on, below. How far
//! apart they are, in dB, scaled by the attack or sustain setting, is
//! the change of gain. Both sides share one gain, so the stereo image
//! stays put.

use crate::{Dsp, Param, Unit, coef, db, gain};

pub const PARAMS: &[Param] = &[
    Param::new("attack", "Attack", -100.0, 100.0, 0.0, Unit::Percent),
    Param::new("sustain", "Sustain", -100.0, 100.0, 0.0, Unit::Percent),
    Param::new("speed", "Attack length", 5.0, 100.0, 25.0, Unit::Ms).log(),
    Param::new("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

/// The most it turns the level up or down, dB.
const MOST_DB: f64 = 18.0;
/// Levels below this count as this, so near-silence isn't pumped.
const FLOOR_DB: f64 = -70.0;

pub struct Transient {
    rate: f64,
    attack: f64,
    sustain: f64,
    /// The quick follower's rise and fall, and the slow one's.
    fast: (f64, f64),
    slow: (f64, f64),
    out: f64,
    /// The two followers' levels now, as gains.
    fast_env: f64,
    slow_env: f64,
    /// The gain now, smoothed, in dB.
    gain_db: f64,
    smooth: f64,
}

impl Transient {
    pub fn new(rate: f64) -> Transient {
        Transient {
            rate,
            attack: 0.0,
            sustain: 0.0,
            fast: (0.0, 0.0),
            slow: (0.0, 0.0),
            out: 1.0,
            fast_env: 0.0,
            slow_env: 0.0,
            gain_db: 0.0,
            smooth: coef(0.001, rate),
        }
    }
}

#[inline]
fn follow(env: &mut f64, x: f64, (up, down): (f64, f64)) {
    let c = if x > *env { up } else { down };
    *env = x + (*env - x) * c;
}

impl Dsp for Transient {
    fn update(&mut self, v: &[f64]) {
        self.attack = v[0] / 100.0;
        self.sustain = v[1] / 100.0;
        let len = v[2] / 1000.0;
        // The slow follower rises over about the attack's length; the
        // quick one falls over it, so the attack ends where it says.
        self.fast = (coef(0.0005, self.rate), coef(len, self.rate));
        self.slow = (coef(len, self.rate), coef(len * 6.0, self.rate));
        self.out = gain(v[3]);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let floor = gain(FLOOR_DB);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x = (l.abs().max(r.abs()) as f64).max(floor);
            follow(&mut self.fast_env, x, self.fast);
            follow(&mut self.slow_env, x, self.slow);
            let apart = db(self.fast_env.max(floor)) - db(self.slow_env.max(floor));
            let want = if apart > 0.0 {
                self.attack * apart
            } else {
                self.sustain * -apart
            };
            let want = want.clamp(-MOST_DB, MOST_DB);
            self.gain_db = want + (self.gain_db - want) * self.smooth;
            let g = (gain(self.gain_db) * self.out) as f32;
            *l *= g;
            *r *= g;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;
    use std::f64::consts::TAU;

    const RATE: f64 = 48_000.0;

    /// A hit: a 200 Hz tone that starts at once and dies away over about
    /// half a second, then silence.
    fn hit() -> Vec<f32> {
        (0..RATE as usize)
            .map(|n| {
                let t = n as f64 / RATE;
                (0.5 * (-t * 6.0).exp() * (TAU * 200.0 * t).sin()) as f32
            })
            .collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, v| m.max(v.abs()))
    }

    fn play(e: &Effect) -> Vec<f32> {
        let mut p = e.processor(RATE as u32);
        let mut l = hit();
        let mut r = l.clone();
        for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(l, r);
        }
        l
    }

    /// The first 10 ms and the stretch from 200 ms to 400 ms, in dB
    /// relative to the hit as it came in.
    fn shape(e: &Effect) -> (f64, f64) {
        let (a, b) = (480, 9600..19_200);
        let dry = hit();
        let out = play(e);
        let rel = |x: &[f32], y: &[f32]| crate::db((peak(x) / peak(y)) as f64);
        (rel(&out[..a], &dry[..a]), rel(&out[b.clone()], &dry[b]))
    }

    #[test]
    fn at_zero_it_changes_nothing() {
        let e = Effect::new(Kind::Transient);
        assert_eq!(play(&e), hit());
    }

    #[test]
    fn attack_lifts_the_start_and_leaves_the_tail() {
        let e = Effect::new(Kind::Transient);
        e.set(0, 100.0);
        let (start, tail) = shape(&e);
        assert!(start > 3.0, "start {start:.1} dB");
        assert!(tail.abs() < 1.5, "tail {tail:.1} dB");
        e.set(0, -100.0);
        let (start, _) = shape(&e);
        assert!(start < -3.0, "start {start:.1} dB");
    }

    #[test]
    fn sustain_lifts_the_tail_and_leaves_the_start() {
        let e = Effect::new(Kind::Transient);
        e.set(1, 100.0);
        let (start, tail) = shape(&e);
        assert!(start.abs() < 1.5, "start {start:.1} dB");
        assert!(tail > 2.0, "tail {tail:.1} dB");
        e.set(1, -100.0);
        let (_, tail) = shape(&e);
        assert!(tail < -2.0, "tail {tail:.1} dB");
    }
}
