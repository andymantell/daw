// SPDX-License-Identifier: GPL-3.0-or-later
//! The bucket-brigade stereo chorus of the 1980s polysynths: one short
//! delay line whose length is swept by a slow triangle, read for the
//! left and right sides at opposite points of the sweep, mixed equally
//! with the dry signal. The bucket-brigade device's clock limits its
//! band and adds a little hiss, which is modelled here with low-passes
//! before and after the delay and a touch of noise. Two sweeps, I (slow)
//! and II (faster), and both at once.

use super::onepole::{OnePole, coefficient};
use crate::dsp::Noise;

/// Which sweep, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChorusMode {
    #[default]
    Off,
    One,
    Two,
    Both,
}

impl ChorusMode {
    pub const NAMES: &[&str] = &["Off", "I", "II", "I + II"];

    pub fn from_index(i: f32) -> ChorusMode {
        match i as u32 {
            1 => ChorusMode::One,
            2 => ChorusMode::Two,
            3 => ChorusMode::Both,
            _ => ChorusMode::Off,
        }
    }
}

/// The sweeps' rates and the delay they sweep over.
const RATE_ONE: f32 = 0.51;
const RATE_TWO: f32 = 0.86;
const CENTRE_MS: f32 = 3.5;
const DEPTH_MS: f32 = 1.6;
/// The bucket-brigade's band.
const BAND_HZ: f32 = 9_000.0;
const HISS: f32 = 2.5e-4;

pub struct Chorus {
    buf: Vec<f32>,
    at: usize,
    rate: f32,
    phase1: f32,
    phase2: f32,
    pre: [OnePole; 2],
    post_l: [OnePole; 2],
    post_r: [OnePole; 2],
    big_g: f32,
    noise: Noise,
    pub mode: ChorusMode,
}

impl Chorus {
    pub fn new(rate: u32) -> Chorus {
        let rate = rate.max(1000) as f32;
        let len = ((CENTRE_MS + DEPTH_MS) * 1.5 * rate / 1000.0) as usize + 4;
        Chorus {
            buf: vec![0.0; len],
            at: 0,
            rate,
            phase1: 0.0,
            phase2: 0.25,
            pre: [OnePole::default(); 2],
            post_l: [OnePole::default(); 2],
            post_r: [OnePole::default(); 2],
            big_g: coefficient(BAND_HZ, rate).1,
            noise: Noise::new(0x5eed_c0de),
            mode: ChorusMode::Off,
        }
    }

    fn triangle(phase: f32) -> f32 {
        1.0 - 4.0 * (phase - 0.5).abs()
    }

    /// Reads the line `ms` back, between samples.
    #[inline]
    fn read(&self, ms: f32) -> f32 {
        let d = ms * self.rate / 1000.0;
        let n = self.buf.len();
        let i = d.floor();
        let f = d - i;
        let a = self.buf[(self.at + n - 1 - i as usize) % n];
        let b = self.buf[(self.at + n - 2 - i as usize) % n];
        a + (b - a) * f
    }

    /// One mono sample in, the left and right out.
    #[inline]
    pub fn run(&mut self, x: f32) -> (f32, f32) {
        if self.mode == ChorusMode::Off {
            return (x, x);
        }
        let mut v = x;
        for p in &mut self.pre {
            v = p.lp(v, self.big_g);
        }
        let n = self.buf.len();
        self.buf[self.at] = v;
        self.at = (self.at + 1) % n;
        self.phase1 = (self.phase1 + RATE_ONE / self.rate).fract();
        self.phase2 = (self.phase2 + RATE_TWO / self.rate).fract();
        let sweep = match self.mode {
            ChorusMode::One => Chorus::triangle(self.phase1),
            ChorusMode::Two => Chorus::triangle(self.phase2),
            _ => 0.5 * (Chorus::triangle(self.phase1) + Chorus::triangle(self.phase2)),
        };
        let mut l = self.read(CENTRE_MS + DEPTH_MS * sweep) + HISS * self.noise.tick();
        let mut r = self.read(CENTRE_MS - DEPTH_MS * sweep) + HISS * self.noise.tick();
        for p in &mut self.post_l {
            l = p.lp(l, self.big_g);
        }
        for p in &mut self.post_r {
            r = p.lp(r, self.big_g);
        }
        (0.5 * (x + l), 0.5 * (x + r))
    }

    pub fn reset(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
        self.pre = [OnePole::default(); 2];
        self.post_l = [OnePole::default(); 2];
        self.post_r = [OnePole::default(); 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_widens_a_tone_and_leaves_it_off_when_off() {
        let rate = 48_000.0;
        let mut c = Chorus::new(48_000);
        assert_eq!(c.run(0.3), (0.3, 0.3));
        c.mode = ChorusMode::One;
        let mut diff = 0.0f32;
        let mut level = 0.0f32;
        for i in 0..96_000 {
            let x = (std::f32::consts::TAU * 440.0 * i as f32 / rate).sin();
            let (l, r) = c.run(x);
            if i > 48_000 {
                diff = diff.max((l - r).abs());
                level = level.max(l.abs());
            }
        }
        assert!(diff > 0.1, "{diff}");
        assert!(level > 0.5 && level < 1.1, "{level}");
    }

    #[test]
    fn the_wet_path_is_band_limited() {
        let rate = 48_000.0;
        let mut c = Chorus::new(48_000);
        c.mode = ChorusMode::Two;
        let mut low = 0.0f32;
        let mut high = 0.0f32;
        for i in 0..48_000 {
            let t = i as f32 / rate;
            let (l, _) = c.run((std::f32::consts::TAU * 18_000.0 * t).sin());
            if i > 24_000 {
                high = high.max(l.abs());
            }
        }
        let mut c = Chorus::new(48_000);
        c.mode = ChorusMode::Two;
        for i in 0..48_000 {
            let t = i as f32 / rate;
            let (l, _) = c.run((std::f32::consts::TAU * 200.0 * t).sin());
            if i > 24_000 {
                low = low.max(l.abs());
            }
        }
        // The dry half always passes; the wet half is gone at 18 kHz.
        assert!(high < 0.62 && low > 0.75, "{high} {low}");
    }
}
