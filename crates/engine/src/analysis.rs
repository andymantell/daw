// SPDX-License-Identifier: GPL-3.0-or-later
//! Analysis of rendered audio on demand (roadmap 2.11): what an
//! assistant asks for to judge a mix, such as "is the bass too loud
//! around 100 Hz?". Takes a whole stretch of stereo audio at once, not
//! a live stream as the meters do, and uses the meters' loudness and
//! true peak so the numbers agree with them.

use realfft::RealFftPlanner;

use crate::meter::{Loudness, SPECTRUM_SIZE, TruePeak};

/// Centres of the third-octave bands reported, 20 Hz to 20 kHz (the
/// nominal centres are 1 kHz × 2^(n/3), rounded in the usual way).
pub fn band_centres() -> Vec<f32> {
    (-17..=13)
        .map(|n| 1000.0 * 2f32.powf(n as f32 / 3.0))
        .collect()
}

/// What a stretch of audio is like.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub seconds: f64,
    /// Highest sample, in dBFS.
    pub sample_peak_db: f32,
    /// Highest level between samples too, in dBTP.
    pub true_peak_db: f32,
    /// Root mean square over the whole stretch, both channels, in dBFS.
    pub rms_db: f32,
    /// Integrated loudness (BS.1770), in LUFS; −∞ when too quiet or
    /// shorter than 400 ms.
    pub integrated_lufs: f64,
    /// The loudest 3 seconds, in LUFS.
    pub short_term_max_lufs: f64,
    /// Loudness range (EBU Tech 3342), in LU.
    pub loudness_range_lu: f64,
    /// How alike left and right are over the whole stretch: 1 the same,
    /// 0 unrelated, −1 opposite; `None` for silence.
    pub correlation: Option<f32>,
    /// Average level in each third-octave band of [`band_centres`], in
    /// dB where a full-scale sine reads about 0 in its band.
    pub bands_db: Vec<f32>,
}

/// Analyses `left` and `right`, the same length, at `rate`.
pub fn analyse(left: &[f32], right: &[f32], rate: u32) -> Report {
    let n = left.len().min(right.len());
    let (left, right) = (&left[..n], &right[..n]);
    let mut loud = Loudness::new(rate);
    let mut tp = TruePeak::default();
    let mut peak = 0.0f32;
    let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
    let mut frame = Vec::with_capacity(2 * 4096);
    for (lc, rc) in left.chunks(4096).zip(right.chunks(4096)) {
        frame.clear();
        for (&l, &r) in lc.iter().zip(rc) {
            frame.extend([l, r]);
            peak = peak.max(l.abs()).max(r.abs());
            ll += (l as f64).powi(2);
            rr += (r as f64).powi(2);
            lr += l as f64 * r as f64;
        }
        loud.push(&frame);
        tp.push(&frame);
    }
    let (tl, tr) = tp.max();
    let db = |x: f64| (20.0 * x.max(1e-10).log10()) as f32;
    let d = (ll * rr).sqrt();
    Report {
        seconds: n as f64 / rate as f64,
        sample_peak_db: db(peak as f64),
        true_peak_db: db(tl.max(tr) as f64),
        rms_db: db(((ll + rr) / (2 * n.max(1)) as f64).sqrt()),
        integrated_lufs: loud.integrated(),
        short_term_max_lufs: loud.max_short_term(),
        loudness_range_lu: loud.range(),
        correlation: (d > 1e-10).then(|| (lr / d).clamp(-1.0, 1.0) as f32),
        bands_db: bands(left, right, rate),
    }
}

/// The average power in each third-octave band, over Hann-windowed
/// frames half overlapping, left and right mixed.
fn bands(left: &[f32], right: &[f32], rate: u32) -> Vec<f32> {
    let size = SPECTRUM_SIZE;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
    let window: Vec<f32> = (0..size)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / size as f32).cos())
        .collect();
    let mut buf = vec![0.0; size];
    let mut out = fft.make_output_vec();
    let mut power = vec![0.0f64; out.len()];
    let mut frames = 0usize;
    let mut at = 0;
    while at < left.len() {
        for (i, b) in buf.iter_mut().enumerate() {
            let k = at + i;
            let x = if k < left.len() {
                0.5 * (left[k] + right[k])
            } else {
                0.0
            };
            *b = x * window[i];
        }
        if fft.process(&mut buf, &mut out).is_ok() {
            // A sine of amplitude A peaks at A × (sum of the window) / 2.
            let scale = 4.0 / size as f32;
            for (p, c) in power.iter_mut().zip(&out) {
                *p += ((c.norm() * scale) as f64).powi(2);
            }
            frames += 1;
        }
        at += size / 2;
    }
    let bin_hz = rate as f32 / size as f32;
    // The Hann window spreads a sine's power over 1.5 bins' worth.
    let enbw = 1.5;
    band_centres()
        .iter()
        .map(|&c| {
            let (lo, hi) = (c * 2f32.powf(-1.0 / 6.0), c * 2f32.powf(1.0 / 6.0));
            let first = (lo / bin_hz).ceil() as usize;
            let last = ((hi / bin_hz).floor() as usize).min(power.len() - 1);
            let sum: f64 = if first <= last {
                power[first..=last].iter().sum()
            } else {
                // Narrower than a bin: the bin it falls in, its share.
                let k = ((c / bin_hz).round() as usize).min(power.len() - 1);
                power[k] * ((hi - lo) / bin_hz) as f64
            };
            let mean = sum / frames.max(1) as f64 / enbw;
            (10.0 * mean.max(1e-20).log10()) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(secs * RATE as f32) as usize)
            .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn band_of(hz: f32) -> usize {
        let c = band_centres();
        (0..c.len())
            .min_by(|&a, &b| {
                let d = |i: usize| (c[i] / hz).ln().abs();
                d(a).total_cmp(&d(b))
            })
            .unwrap()
    }

    #[test]
    fn a_sine_shows_in_its_band_at_its_level() {
        let x = sine(1000.0, 0.5, 2.0);
        let r = analyse(&x, &x, RATE);
        let b = band_of(1000.0);
        // Half scale is −6 dB.
        assert!((r.bands_db[b] + 6.0).abs() < 1.0, "{}", r.bands_db[b]);
        assert!(r.bands_db[band_of(100.0)] < -60.0);
        assert!(r.bands_db[band_of(10_000.0)] < -60.0);
        assert!((r.sample_peak_db + 6.02).abs() < 0.05);
        // A sine's RMS is 3 dB under its peak.
        assert!((r.rms_db + 9.03).abs() < 0.05, "{}", r.rms_db);
        assert_eq!(r.correlation, Some(1.0));
        assert!((r.seconds - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_low_note_is_found_low() {
        let x = sine(100.0, 0.25, 2.0);
        let r = analyse(&x, &x, RATE);
        let loudest = (0..r.bands_db.len())
            .max_by(|&a, &b| r.bands_db[a].total_cmp(&r.bands_db[b]))
            .unwrap();
        assert_eq!(loudest, band_of(100.0));
    }

    #[test]
    fn opposite_channels_read_minus_one_and_silence_reads_nothing() {
        let x = sine(440.0, 0.5, 1.0);
        let y: Vec<f32> = x.iter().map(|v| -v).collect();
        assert_eq!(analyse(&x, &y, RATE).correlation, Some(-1.0));
        let z = vec![0.0; RATE as usize];
        let r = analyse(&z, &z, RATE);
        assert_eq!(r.correlation, None);
        assert!(r.integrated_lufs.is_infinite());
    }

    #[test]
    fn loudness_matches_the_meter() {
        // A 1 kHz sine at −20 dBFS in both channels reads about
        // −20 LUFS (stereo adds 3 dB; 1 kHz is not weighted, almost).
        let x = sine(1000.0, 0.1, 5.0);
        let r = analyse(&x, &x, RATE);
        assert!(
            (r.integrated_lufs + 20.0).abs() < 0.5,
            "{}",
            r.integrated_lufs
        );
    }
}
