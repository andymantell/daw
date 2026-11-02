// SPDX-License-Identifier: GPL-3.0-or-later
//! Twice-rate oversampling around a nonlinear part: the input is raised
//! to twice the sample rate through a half-band low-pass, the part runs
//! twice, and the result comes back down through the same low-pass, so
//! the harmonics the part adds above the old Nyquist frequency are
//! removed rather than folded back as aliasing.
//!
//! The half-band filter is a windowed sinc designed here (47 taps, a
//! Blackman window); every other tap is zero, which is what makes it
//! cheap.

/// Taps either side of the centre.
const L: usize = 23;
/// Odd-numbered taps above the centre: the only ones that are not zero.
const ODD: usize = L.div_ceil(2);
/// Half the span, where the output sits.
const D: usize = (L - 1) / 2;

/// The odd taps of the interpolating filter, `2 h[2j + 1]` for `j` in
/// `0..ODD`; the centre tap is 1 and the even ones zero.
fn taps() -> [f32; ODD] {
    let mut t = [0.0f32; ODD];
    for (j, tap) in t.iter_mut().enumerate() {
        let i = (2 * j + 1) as f32;
        let sinc = (std::f32::consts::FRAC_PI_2 * i).sin() / (std::f32::consts::FRAC_PI_2 * i);
        // Blackman window over −L..L.
        let w = 0.42
            + 0.5 * (std::f32::consts::PI * i / L as f32).cos()
            + 0.08 * (2.0 * std::f32::consts::PI * i / L as f32).cos();
        *tap = sinc * w;
    }
    t
}

/// Ring buffers of this size (a power of two over `L + 1`) hold the
/// histories, so nothing is shifted each sample.
const RING: usize = 32;
const MASK: usize = RING - 1;

/// A part run at twice the rate, with the filters around it.
#[derive(Clone, Copy, Debug)]
pub struct Over2 {
    taps: [f32; ODD],
    /// The inputs; `at` is where the newest went.
    x: [f32; RING],
    /// The part's even and odd outputs.
    even: [f32; RING],
    odd: [f32; RING],
    at: usize,
}

impl Default for Over2 {
    fn default() -> Over2 {
        Over2::new()
    }
}

impl Over2 {
    pub fn new() -> Over2 {
        Over2 {
            taps: taps(),
            x: [0.0; RING],
            even: [0.0; RING],
            odd: [0.0; RING],
            at: 0,
        }
    }

    /// Runs `part` twice at the doubled rate for the input `x` and
    /// returns the output at the original rate, delayed by `L` of its
    /// samples.
    #[inline]
    pub fn run(&mut self, x: f32, mut part: impl FnMut(f32) -> f32) -> f32 {
        let at = (self.at + 1) & MASK;
        self.at = at;
        // `back(k)` is the entry k samples ago.
        let back = |k: usize| (at + RING - k) & MASK;
        // Up: the even sample is an input; the odd one between is the
        // interpolation of its neighbours, taps 2j+1 either side of the
        // point between inputs D+1 and D ago.
        self.x[at] = x;
        let a = self.x[back(D + 1)];
        let mut b = 0.0;
        for (j, t) in self.taps.iter().enumerate() {
            b += t * (self.x[back(D - j)] + self.x[back(D + 1 + j)]);
        }
        let ya = part(a);
        let yb = part(b);
        // Down: the same filter at half the gain, centred on an odd
        // output with the taps on the even ones, dropping every other.
        self.even[at] = ya;
        self.odd[at] = yb;
        let mut y = 0.5 * self.odd[back(D + 1)];
        for (j, t) in self.taps.iter().enumerate() {
            y += 0.5 * t * (self.even[back(D - j)] + self.even[back(D + 1 + j)]);
        }
        y
    }

    pub fn reset(&mut self) {
        self.x = [0.0; RING];
        self.even = [0.0; RING];
        self.odd = [0.0; RING];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vintage::measure;

    #[test]
    fn a_clean_part_passes_the_audio_band_and_is_delayed() {
        let rate = 48_000.0;
        let mut o = Over2::new();
        assert!((measure::level(1000.0, rate, |x| o.run(x, |v| v)) - 1.0).abs() < 0.01);
        let mut o = Over2::new();
        assert!((measure::level(15_000.0, rate, |x| o.run(x, |v| v)) - 1.0).abs() < 0.05);
        // A step comes out L samples later, whole.
        let mut o = Over2::new();
        let out: Vec<f32> = (0..80).map(|_| o.run(1.0, |v| v)).collect();
        let at = out.iter().position(|&v| v >= 0.5).unwrap();
        assert!(at.abs_diff(L) <= 1, "{at}");
        assert!((out[79] - 1.0).abs() < 0.01, "{}", out[79]);
    }

    #[test]
    fn clipping_at_twice_the_rate_aliases_less() {
        // A hard-clipped 10 kHz sine: its third harmonic at 30 kHz folds
        // to 18 kHz at 48 kHz. Oversampled, that product is removed.
        let rate = 48_000.0;
        let alias_at = |mut f: Box<dyn FnMut(f32) -> f32>| {
            let n = 48_000;
            let out: Vec<f32> = (0..n)
                .map(|i| f((std::f32::consts::TAU * 10_000.0 * i as f32 / rate).sin()))
                .collect();
            // The level at 21 kHz by correlation over the second half.
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, &v) in out.iter().enumerate().skip(n / 2) {
                let p = std::f32::consts::TAU * 18_000.0 * i as f32 / rate;
                re += v * p.cos();
                im += v * p.sin();
            }
            (re * re + im * im).sqrt() * 2.0 / (n / 2) as f32
        };
        let plain = alias_at(Box::new(|x: f32| (x * 4.0).clamp(-1.0, 1.0)));
        let mut o = Over2::new();
        let over = alias_at(Box::new(move |x: f32| {
            o.run(x, |v| (v * 4.0).clamp(-1.0, 1.0))
        }));
        assert!(plain > 0.1, "{plain}");
        assert!(over < plain * 0.1, "{over} vs {plain}");
    }
}
