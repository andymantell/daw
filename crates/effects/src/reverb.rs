// SPDX-License-Identifier: GPL-3.0-or-later
//! An algorithmic reverb: a feedback delay network as described by
//! J.-M. Jot and A. Chaigne ("Digital delay networks for designing
//! artificial reverberators", AES 90th Convention, 1991). Eight delay
//! lines feed back through a Hadamard matrix, which keeps energy
//! unchanged, with each line's gain set from the decay time and its
//! length, and a low-pass filter in each for the high frequencies dying
//! away first. Series all-pass filters (after M. R. Schroeder, "Natural
//! sounding artificial reverberation", JAES 10(3), 1962) spread the input
//! out first. The delay lengths are our own.

use std::f64::consts::PI;

use crate::{Dsp, Param, Unit, flush};

pub const PARAMS: &[Param] = &[
    Param::new("decay", "Decay", 0.2, 10.0, 1.8, Unit::Seconds).log(),
    Param::new("predelay", "Pre-delay", 0.0, 200.0, 10.0, Unit::Ms),
    Param::new("tone", "Tone", 1000.0, 20000.0, 7000.0, Unit::Hz)
        .log()
        .off_at(20000.0),
    Param::new("mix", "Mix", 0.0, 100.0, 25.0, Unit::Percent),
    Param::new("width", "Width", 0.0, 100.0, 100.0, Unit::Percent),
];

const LINES: usize = 8;
/// The network's delay lengths, in milliseconds before rounding to a
/// prime number of frames.
const LINE_MS: [f64; LINES] = [31.7, 37.3, 41.9, 47.7, 53.3, 59.9, 66.1, 73.7];
/// The diffusers' lengths, in milliseconds, and how much each feeds back.
const DIFFUSE_MS: [f64; 4] = [4.7, 3.6, 12.7, 9.3];
const DIFFUSE_GAIN: f32 = 0.6;
/// Brings the network's output to about the level of its input.
const OUT_GAIN: f32 = 0.3;
const MAX_PREDELAY_SECONDS: f64 = 0.2;

/// The first prime at or above `n`.
fn prime_from(n: usize) -> usize {
    let is_prime = |k: usize| {
        k >= 2
            && (2..)
                .take_while(|d| d * d <= k)
                .all(|d| !k.is_multiple_of(d))
    };
    (n..).find(|&k| is_prime(k)).unwrap_or(n)
}

/// A delay line that is read where it is about to be written.
struct Line {
    buf: Vec<f32>,
    at: usize,
}

impl Line {
    fn new(len: usize) -> Line {
        Line {
            buf: vec![0.0; len.max(1)],
            at: 0,
        }
    }

    #[inline]
    fn out(&self) -> f32 {
        self.buf[self.at]
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.at] = flush(x);
        self.at += 1;
        if self.at == self.buf.len() {
            self.at = 0;
        }
    }
}

/// A Schroeder all-pass filter.
struct AllPass(Line);

impl AllPass {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let d = self.0.out();
        let w = x + DIFFUSE_GAIN * d;
        self.0.push(w);
        d - DIFFUSE_GAIN * w
    }
}

/// The in-place fast Walsh–Hadamard transform, scaled to keep energy.
#[inline]
fn hadamard(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        for i in (0..LINES).step_by(h * 2) {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
        }
        h *= 2;
    }
    let s = 1.0 / (LINES as f32).sqrt();
    for x in v.iter_mut() {
        *x *= s;
    }
}

pub struct Reverb {
    rate: f64,
    predelay: Vec<f32>,
    pre_at: usize,
    pre_frames: f64,
    diffusers: Vec<AllPass>,
    lines: Vec<Line>,
    /// Each line's gain per pass, and its low-pass state.
    gains: [f32; LINES],
    damp: f32,
    lows: [f32; LINES],
    wet: f32,
    width: f32,
}

impl Reverb {
    pub fn new(rate: f64) -> Reverb {
        let frames = |ms: f64| (ms / 1000.0 * rate).round() as usize;
        Reverb {
            rate,
            predelay: vec![0.0; (MAX_PREDELAY_SECONDS * rate) as usize + 4],
            pre_at: 0,
            pre_frames: 0.0,
            diffusers: (DIFFUSE_MS.iter())
                .map(|&ms| AllPass(Line::new(frames(ms))))
                .collect(),
            lines: (LINE_MS.iter())
                .map(|&ms| Line::new(prime_from(frames(ms))))
                .collect(),
            gains: [0.0; LINES],
            damp: 0.0,
            lows: [0.0; LINES],
            wet: 0.0,
            width: 1.0,
        }
    }
}

impl Dsp for Reverb {
    fn update(&mut self, v: &[f64]) {
        let decay = v[0];
        for (g, l) in self.gains.iter_mut().zip(&self.lines) {
            // Down 60 dB after `decay` seconds.
            *g = 10f64.powf(-3.0 * l.buf.len() as f64 / (decay * self.rate)) as f32;
        }
        self.pre_frames = (v[1] / 1000.0 * self.rate).min(self.predelay.len() as f64 - 3.0);
        self.damp = if PARAMS[2].off_at == Some(v[2]) {
            0.0
        } else {
            (-2.0 * PI * v[2] / self.rate).exp() as f32
        };
        self.wet = (v[3] / 100.0) as f32;
        self.width = (v[4] / 100.0) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = self.predelay.len();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.predelay[self.pre_at] = (*l + *r) * 0.5;
            let mut x = crate::delay_read(&self.predelay, self.pre_at, self.pre_frames);
            self.pre_at = (self.pre_at + 1) % n;
            for d in &mut self.diffusers {
                x = d.run(x);
            }
            let mut v = [0f32; LINES];
            let (mut ol, mut or) = (0f32, 0f32);
            for (i, (line, low)) in self.lines.iter().zip(&mut self.lows).enumerate() {
                let o = line.out();
                // Output taps with two orthogonal sign patterns, so left
                // and right are unalike.
                ol += if i % 2 == 0 { o } else { -o };
                or += if (i / 2) % 2 == 0 { o } else { -o };
                *low = o * (1.0 - self.damp) + *low * self.damp;
                v[i] = *low * self.gains[i];
            }
            hadamard(&mut v);
            for (i, line) in self.lines.iter_mut().enumerate() {
                let sign = if i % 3 == 0 { -1.0 } else { 1.0 };
                line.push(v[i] + x * sign);
            }
            let (ol, or) = (ol * OUT_GAIN, or * OUT_GAIN);
            let mid = (ol + or) * 0.5;
            let side = (ol - or) * 0.5 * self.width;
            *l = *l * (1.0 - self.wet) + (mid + side) * self.wet;
            *r = *r * (1.0 - self.wet) + (mid - side) * self.wet;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;

    /// The energy of the reverb's answer to a click, all wet, in each of
    /// `n` windows of 100 ms.
    fn decay_curve(decay: f64, n: usize) -> Vec<f64> {
        let e = Effect::new(Kind::Reverb);
        e.set(0, decay);
        e.set(1, 0.0);
        e.set(2, 20000.0);
        e.set(3, 100.0);
        let mut p = e.processor(48_000);
        let mut l = vec![0f32; 4800 * n];
        l[0] = 1.0;
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        l.chunks(4800)
            .map(|c| c.iter().map(|x| (*x as f64).powi(2)).sum::<f64>())
            .collect()
    }

    #[test]
    fn decays_at_the_rate_asked_for() {
        for decay in [0.5, 2.0] {
            let c = decay_curve(decay, 30);
            // From the second window on, the network is full; compare the
            // energy one decay time apart: about 60 dB less.
            let a = 2;
            let b = a + (decay * 10.0) as usize;
            let drop = 10.0 * (c[a] / c[b]).log10();
            assert!((drop - 60.0).abs() < 6.0, "{decay}: {drop}");
        }
    }

    #[test]
    fn its_level_is_near_the_input() {
        // A click's energy is 1; the reverb's whole answer is near it.
        let total: f64 = decay_curve(1.8, 40).iter().sum();
        assert!(total > 0.25 && total < 4.0, "{total}");
    }

    #[test]
    fn left_and_right_differ() {
        let e = Effect::new(Kind::Reverb);
        e.set(3, 100.0);
        let mut p = e.processor(48_000);
        let mut l = vec![0f32; 48_000];
        l[0] = 1.0;
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        let (mut lr, mut ll, mut rr) = (0f64, 0f64, 0f64);
        for (a, b) in l.iter().zip(&r) {
            lr += (*a * *b) as f64;
            ll += (*a * *a) as f64;
            rr += (*b * *b) as f64;
        }
        let corr = lr / (ll * rr).sqrt();
        assert!(corr.abs() < 0.5, "{corr}");
    }
}
