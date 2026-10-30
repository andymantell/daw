// SPDX-License-Identifier: GPL-3.0-or-later
//! Sounds the samplers play, held in memory, and a player that reads
//! one at any speed.

use std::sync::{Arc, Mutex};

/// A sound in memory: one or two channels at a rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub left: Vec<f32>,
    /// The same as `left` for a mono sound.
    pub right: Vec<f32>,
    pub rate: u32,
}

impl Sample {
    pub fn mono(samples: Vec<f32>, rate: u32) -> Sample {
        Sample {
            right: samples.clone(),
            left: samples,
            rate,
        }
    }

    /// From interleaved samples of `channels` channels (only the first
    /// two are kept).
    pub fn interleaved(samples: &[f32], channels: usize, rate: u32) -> Sample {
        let k = channels.max(1);
        let left: Vec<f32> = samples.iter().step_by(k).copied().collect();
        let right = if k > 1 {
            samples.iter().skip(1).step_by(k).copied().collect()
        } else {
            left.clone()
        };
        Sample { left, right, rate }
    }

    pub fn len(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn seconds(&self) -> f64 {
        self.len() as f64 / self.rate.max(1) as f64
    }

    /// The sound at `rate`, converted if it is at another.
    pub fn at_rate(&self, rate: u32) -> Result<Sample, String> {
        if rate == self.rate || self.is_empty() {
            return Ok(Sample {
                rate,
                ..self.clone()
            });
        }
        let inter: Vec<f32> = (0..self.len())
            .flat_map(|i| [self.left[i], self.right[i]])
            .collect();
        let out = daw_engine::import::resample(&inter, 2, self.rate, rate)?;
        Ok(Sample::interleaved(&out, 2, rate))
    }

    /// The left and right samples at `pos`, between frames by cubic
    /// (Hermite) interpolation; silence outside the sound.
    #[inline]
    pub fn read(&self, pos: f64) -> (f32, f32) {
        let n = self.len();
        if pos < 0.0 || n == 0 || pos >= n as f64 {
            return (0.0, 0.0);
        }
        let i = pos as usize;
        let t = (pos - i as f64) as f32;
        let at = |c: &[f32], j: isize| -> f32 {
            if j < 0 || j as usize >= n {
                0.0
            } else {
                c[j as usize]
            }
        };
        let i = i as isize;
        let h = |c: &[f32]| hermite(at(c, i - 1), at(c, i), at(c, i + 1), at(c, i + 2), t);
        (h(&self.left), h(&self.right))
    }

    /// The highest level, for showing and for finding the start.
    pub fn peak(&self) -> f32 {
        (self.left.iter().chain(&self.right)).fold(0.0f32, |m, v| m.max(v.abs()))
    }
}

/// Four-point, third-order Hermite interpolation between `x0` and `x1`.
#[inline]
fn hermite(xm: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x1 - xm);
    let c2 = xm - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

/// A sound an instrument uses, by where it comes from: a file (an asset
/// name, or a full path until the project is saved) or a sound made
/// here ([`crate::made`]). Copies at other rates are made once and kept.
pub struct Loaded {
    pub reference: String,
    /// `None` when it couldn't be read: the reference is kept so saving
    /// doesn't lose it, and nothing plays.
    pub sample: Option<Arc<Sample>>,
    rates: Mutex<Vec<Arc<Sample>>>,
}

impl Loaded {
    pub fn new(reference: String, sample: Option<Arc<Sample>>) -> Loaded {
        Loaded {
            reference,
            sample,
            rates: Mutex::new(Vec::new()),
        }
    }

    /// The sound at `rate`. Converting allocates; not for the audio
    /// thread.
    pub fn at(&self, rate: u32) -> Option<Arc<Sample>> {
        let s = self.sample.as_ref()?;
        if s.rate == rate {
            return Some(s.clone());
        }
        let mut cache = self.rates.lock().ok()?;
        if let Some(c) = cache.iter().find(|c| c.rate == rate) {
            return Some(c.clone());
        }
        let c = Arc::new(s.at_rate(rate).ok()?);
        cache.push(c.clone());
        Some(c)
    }
}

/// Reads a sample at any speed, from a start to an end, forwards or
/// backwards, with an optional loop.
#[derive(Clone, Copy, Debug, Default)]
pub struct Player {
    pub pos: f64,
    pub step: f64,
    /// Plays from `start` towards `end`; backwards when `end < start`.
    pub start: f64,
    pub end: f64,
    /// A loop between these, forwards only; `None` for none.
    pub looping: Option<(f64, f64)>,
    pub done: bool,
}

/// Frames over which a sound fades in and out where it is cut, so a
/// start or end inside the sound doesn't click.
const EDGE: f64 = 64.0;

impl Player {
    pub fn new(start: f64, end: f64, step: f64) -> Player {
        Player {
            pos: start,
            step: step.abs().max(1e-6),
            start,
            end,
            looping: None,
            done: end == start,
        }
    }

    /// The next frame, and moves on.
    #[inline]
    pub fn next(&mut self, s: &Sample) -> (f32, f32) {
        if self.done {
            return (0.0, 0.0);
        }
        let forwards = self.end >= self.start;
        let (l, r) = s.read(self.pos);
        // Fades at a cut start and at the end.
        let from_start = (self.pos - self.start).abs();
        let to_end = (self.end - self.pos).abs();
        let mut g = 1.0;
        if self.start > 0.0 && from_start < EDGE {
            g = from_start / EDGE;
        }
        if self.looping.is_none() && to_end < EDGE && (self.end as usize) < s.len() {
            g = g.min(to_end / EDGE);
        }
        let g = g as f32;
        if forwards {
            self.pos += self.step;
            if let Some((a, b)) = self.looping
                && b > a
                && self.pos >= b
            {
                self.pos = a + (self.pos - b) % (b - a);
            }
            if self.pos >= self.end {
                self.done = true;
            }
        } else {
            self.pos -= self.step;
            if self.pos <= self.end {
                self.done = true;
            }
        }
        (l * g, r * g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_between_frames_and_at_another_rate() {
        let s = Sample::mono((0..100).map(|i| i as f32).collect(), 1000);
        let (l, r) = s.read(10.5);
        assert!((l - 10.5).abs() < 1e-4);
        assert_eq!(l, r);
        assert_eq!(s.read(-1.0), (0.0, 0.0));
        let t = Sample::mono(vec![0.25; 4800], 48_000)
            .at_rate(44_100)
            .unwrap();
        assert_eq!(t.len(), 4410);
        assert!((t.left[2000] - 0.25).abs() < 0.01);
    }

    #[test]
    fn a_player_runs_to_its_end_or_round_its_loop() {
        let s = Sample::mono(vec![1.0; 1000], 1000);
        let mut p = Player::new(0.0, 1000.0, 2.0);
        let n = (0..1000)
            .take_while(|_| !p.done && p.next(&s).0 >= 0.0)
            .count();
        assert_eq!(n, 500);
        let mut p = Player::new(0.0, 1000.0, 1.0);
        p.looping = Some((100.0, 200.0));
        for _ in 0..5000 {
            p.next(&s);
        }
        assert!(!p.done && (100.0..200.0).contains(&p.pos));
        let mut p = Player::new(999.0, 0.0, 1.0);
        let n = (0..2000)
            .take_while(|_| !p.done && p.next(&s).0 >= 0.0)
            .count();
        assert_eq!(n, 999);
    }
}
