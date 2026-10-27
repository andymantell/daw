// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.4: finding a known test pulse in a loopback recording.
//!
//! The pulse is a short swept sine. Cross-correlating the recording with
//! it gives a sharp peak at the pulse's arrival, which survives noise,
//! level changes and a polarity flip in the interface.

use std::f32::consts::PI;

/// A 20 ms sweep from 200 Hz to 8 kHz with a raised-cosine envelope.
pub fn probe(rate: u32) -> Vec<f32> {
    let n = (rate as f32 * 0.02) as usize;
    let (f0, f1) = (200.0f32, 8_000.0f32);
    let dur = n as f32 / rate as f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let phase = 2.0 * PI * (f0 * t + 0.5 * (f1 - f0) / dur * t * t);
            let env = 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos();
            0.5 * env * phase.sin()
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    /// Index in the recording where the pulse starts, to a fraction of a
    /// sample.
    pub at: f64,
    /// Correlation peak over the median correlation level.
    pub snr: f32,
    /// The interface flipped the signal's polarity.
    pub inverted: bool,
}

/// Searches `rec[from..to]` for the start of `probe`.
pub fn find(rec: &[f32], probe: &[f32], from: usize, to: usize) -> Option<Found> {
    let to = to.min(rec.len().saturating_sub(probe.len()));
    if from + 2 >= to {
        return None;
    }
    let corr: Vec<f32> = (from..to)
        .map(|s| {
            rec[s..s + probe.len()]
                .iter()
                .zip(probe)
                .map(|(a, b)| a * b)
                .sum()
        })
        .collect();
    let (i, &peak) = corr
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    let mut mags: Vec<f32> = corr.iter().map(|c| c.abs()).collect();
    mags.sort_by(f32::total_cmp);
    let median = mags[mags.len() / 2].max(1e-12);
    // Parabolic interpolation on the magnitude for a fractional position.
    let frac = if i > 0 && i + 1 < corr.len() {
        let (a, b, c) = (corr[i - 1].abs(), peak.abs(), corr[i + 1].abs());
        let d = a - 2.0 * b + c;
        if d.abs() > 1e-12 {
            0.5 * (a - c) / d
        } else {
            0.0
        }
    } else {
        0.0
    };
    Some(Found {
        at: (from + i) as f64 + frac as f64,
        snr: peak.abs() / median,
        inverted: peak < 0.0,
    })
}

pub fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, level: f32) -> Vec<f32> {
        let mut s = 0x9e37_79b9u32;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                level * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    #[test]
    fn finds_whole_sample_delay_through_noise_and_inversion() {
        let p = probe(48_000);
        let mut rec = noise(48_000, 0.05);
        for (i, v) in p.iter().enumerate() {
            rec[10_123 + i] -= 0.3 * v;
        }
        let f = find(&rec, &p, 8_000, 20_000).unwrap();
        assert!((f.at - 10_123.0).abs() < 0.05, "{f:?}");
        assert!(f.inverted);
        assert!(f.snr > 10.0, "{f:?}");
    }

    #[test]
    fn finds_fractional_delay() {
        let p = probe(48_000);
        // Half-sample delay by averaging neighbours (a crude but known one).
        let mut rec = vec![0.0f32; 20_000];
        for i in 0..p.len() {
            rec[5_000 + i] += 0.5 * p[i];
            rec[5_001 + i] += 0.5 * p[i];
        }
        let f = find(&rec, &p, 4_000, 6_000).unwrap();
        assert!((f.at - 5_000.5).abs() < 0.2, "{f:?}");
    }

    #[test]
    fn silence_has_low_snr() {
        let p = probe(48_000);
        let rec = noise(20_000, 0.05);
        let f = find(&rec, &p, 0, 10_000).unwrap();
        assert!(f.snr < 8.0, "{f:?}");
    }
}
