// SPDX-License-Identifier: GPL-3.0-or-later
//! A streaming sample-rate converter for the audio thread, for running
//! an amp capture at the rate it was made at.
//!
//! Each output is the input convolved with a windowed sinc centred on
//! the output's time (band-limited interpolation, as described by J. O.
//! Smith, "Digital Audio Resampling", CCRMA). The kernel is tabulated at
//! [`PHASES`] offsets between two input samples, with straight-line
//! interpolation between them, and windowed by a Kaiser window. Outputs
//! are exactly in time with the input, and each is made once the
//! [`TAPS`] / 2 input samples after it have arrived.

/// Input samples each output is made from.
pub const TAPS: usize = 64;
/// Offsets between two input samples the kernel is tabulated at.
const PHASES: usize = 256;
/// The Kaiser window's shape: about 85 dB down outside the band.
const BETA: f64 = 8.6;
/// The share of the lower rate's half that is kept; above it fades out
/// so nothing folds back into what is heard.
const BAND: f64 = 0.85;

pub struct Resampler {
    /// Input samples per output.
    step: f64,
    /// The kernel at each phase, `TAPS` values each, for phases 0 to
    /// `PHASES` inclusive.
    table: Vec<f32>,
    /// The input, twice over, so the last `TAPS` are read in one slice.
    hist: Vec<f32>,
    at: usize,
    /// Input samples taken so far.
    count: u64,
    /// The next output's time, in input samples: a whole and a part.
    next: u64,
    frac: f64,
}

impl Resampler {
    /// Converts from `from` to `to` (in Hz). Allocates.
    pub fn new(from: f64, to: f64) -> Resampler {
        let fc = BAND * (to / from).min(1.0);
        let half = (TAPS / 2) as f64;
        let i0b = bessel_i0(BETA);
        let mut table = Vec::with_capacity((PHASES + 1) * TAPS);
        for p in 0..=PHASES {
            let f = p as f64 / PHASES as f64;
            // Tap `k` holds input `i - TAPS/2 + 1 + k` for an output at
            // `i + f`, so it is `d` samples from the output's time.
            for k in 0..TAPS {
                let d = f + half - 1.0 - k as f64;
                let x = fc * d;
                let sinc = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                };
                let r = d / half;
                let w = if r.abs() >= 1.0 {
                    0.0
                } else {
                    bessel_i0(BETA * (1.0 - r * r).sqrt()) / i0b
                };
                table.push((fc * sinc * w) as f32);
            }
        }
        Resampler {
            step: from / to,
            table,
            hist: vec![0.0; 2 * TAPS],
            at: 0,
            count: 0,
            next: 0,
            frac: 0.0,
        }
    }

    /// How late, in input samples, the last output is made after the
    /// input at its time.
    pub fn lookahead() -> usize {
        TAPS / 2
    }

    /// The most outputs `frames` inputs can make.
    pub fn most_out(&self, frames: usize) -> usize {
        (frames as f64 / self.step).ceil() as usize + 1
    }

    /// Takes `input` and writes what it can make to the start of `out`,
    /// returning how many. `out` must hold [`Resampler::most_out`]
    /// values; any more are dropped.
    pub fn process(&mut self, input: &[f32], out: &mut [f32]) -> usize {
        let mut n = 0;
        for &x in input {
            self.hist[self.at] = x;
            self.hist[self.at + TAPS] = x;
            self.at = (self.at + 1) % TAPS;
            self.count += 1;
            // An output at `next` needs inputs up to `next + TAPS/2`.
            while self.next + (TAPS as u64 / 2) < self.count {
                let y = self.at_time();
                if let Some(o) = out.get_mut(n) {
                    *o = y;
                    n += 1;
                }
                self.frac += self.step;
                let whole = self.frac.floor();
                self.next += whole as u64;
                self.frac -= whole;
            }
        }
        n
    }

    /// The output at `next + frac`.
    fn at_time(&self) -> f32 {
        // The oldest input it needs is `next - TAPS/2 + 1`; the newest
        // taken is `count - 1`, at `at - 1` in the ring.
        let behind = (self.count - 1 - (self.next + TAPS as u64 / 2)) as usize;
        let start = (self.at + TAPS - behind) % TAPS;
        let x = &self.hist[start..start + TAPS];
        let pos = self.frac * PHASES as f64;
        let p = (pos as usize).min(PHASES - 1);
        let t = (pos - p as f64) as f32;
        let a = &self.table[p * TAPS..(p + 1) * TAPS];
        let b = &self.table[(p + 1) * TAPS..(p + 2) * TAPS];
        let mut sum = 0f32;
        for k in 0..TAPS {
            sum += x[k] * (a[k] + (b[k] - a[k]) * t);
        }
        sum
    }
}

/// The modified Bessel function of the first kind, order zero, by its
/// power series.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-16 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn run(r: &mut Resampler, input: &[f32], block: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0f32; r.most_out(block)];
        for chunk in input.chunks(block) {
            let n = r.process(chunk, &mut buf);
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    /// A sine comes out as the same sine at the new rate, in time.
    #[test]
    fn a_sine_keeps_its_shape_and_time() {
        for (from, to) in [
            (44_100.0, 48_000.0),
            (48_000.0, 44_100.0),
            (96_000.0, 48_000.0),
        ] {
            let hz = 1000.0;
            let input: Vec<f32> = (0..20_000)
                .map(|n| (2.0 * PI * hz * n as f64 / from).sin() as f32 * 0.5)
                .collect();
            let mut r = Resampler::new(from, to);
            let out = run(&mut r, &input, 29);
            let want = ((20_000 - TAPS) as f64 * to / from) as usize;
            assert!(out.len() >= want, "{from}->{to}: {}", out.len());
            let err = (100..out.len())
                .map(|m| {
                    let t = m as f64 / to;
                    (out[m] - (2.0 * PI * hz * t).sin() as f32 * 0.5).abs()
                })
                .fold(0f32, f32::max);
            assert!(err < 1e-3, "{from}->{to}: {err}");
        }
    }

    /// Going down a rate, what the lower rate can't hold is taken out
    /// rather than folded back.
    #[test]
    fn going_down_a_rate_takes_out_what_would_fold_back() {
        let (from, to) = (48_000.0, 44_100.0);
        // 23 kHz would fold to 21.1 kHz at 44.1 kHz.
        let input: Vec<f32> = (0..20_000)
            .map(|n| (2.0 * PI * 23_000.0 * n as f64 / from).sin() as f32)
            .collect();
        let mut r = Resampler::new(from, to);
        let out = run(&mut r, &input, 32);
        let peak = out[200..].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak < 1e-3, "{peak}");
    }
}
