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
    /// The key it sounds in, if it has enough pitched sound to say.
    pub key: Option<KeyGuess>,
    /// The beat it keeps, if it has enough attacks to say.
    pub tempo: Option<TempoGuess>,
}

/// A key heard: the root (0 for C up to 11 for B), major or minor, and
/// how far ahead of the next best key it is (0 to 2; under about 0.05
/// is a close call, often with its relative major or minor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyGuess {
    pub root: u8,
    pub minor: bool,
    pub margin: f32,
}

/// A beat heard, in beats per minute, and how strongly it stands out
/// (the beat's share of the rhythm, 0 to 1). Half or double the tempo
/// played is a common answer for any detector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoGuess {
    pub bpm: f64,
    pub strength: f32,
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
        key: key(left, right, rate),
        tempo: tempo(left, right, rate),
    }
}

/// The major and minor key profiles of Krumhansl and Kessler: how well
/// each of the twelve notes from the tonic fits the key, as listeners
/// rated them.
const MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let (x, y) = (a[i] - ma, b[i] - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa <= 0.0 || bb <= 0.0 {
        return 0.0;
    }
    ab / (aa * bb).sqrt()
}

/// How much of each of the twelve notes sounds, from the spectrum
/// between 55 Hz and 4 kHz, over Hann-windowed frames.
///
/// Each spectral peak counts once, for the note at its frequency
/// worked out between bins. Low down a bin is wider than a semitone, so
/// counting every bin gave a loud bass's spill into the bins beside it
/// to the notes either side: an A minor song with its bass read as A♯
/// minor.
fn chroma(left: &[f32], right: &[f32], rate: u32) -> [f32; 12] {
    let size = SPECTRUM_SIZE;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
    let window: Vec<f32> = (0..size)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / size as f32).cos())
        .collect();
    let bin_hz = rate as f32 / size as f32;
    let mut buf = vec![0.0; size];
    let mut out = fft.make_output_vec();
    let mut mag = vec![0.0f32; out.len()];
    let mut c = [0.0f32; 12];
    let mut at = 0;
    while at + size <= left.len() {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = 0.5 * (left[at + i] + right[at + i]) * window[i];
        }
        if fft.process(&mut buf, &mut out).is_ok() {
            for (m, v) in mag.iter_mut().zip(&out) {
                *m = v.norm();
            }
            for k in 1..mag.len() - 1 {
                let (a, b, n) = (mag[k - 1], mag[k], mag[k + 1]);
                if !(b > a && b >= n) {
                    continue;
                }
                // Parabolic interpolation on the log magnitudes.
                let (la, lb, ln) = (a.max(1e-12).ln(), b.ln(), n.max(1e-12).ln());
                let den = la - 2.0 * lb + ln;
                let shift = if den.abs() > f32::EPSILON {
                    (0.5 * (la - ln) / den).clamp(-0.5, 0.5)
                } else {
                    0.0
                };
                let f = (k as f32 + shift) * bin_hz;
                if !(55.0..4000.0).contains(&f) {
                    continue;
                }
                let pitch = 69.0 + 12.0 * (f / 440.0).log2();
                c[(pitch.round() as i64).rem_euclid(12) as usize] += a + b + n;
            }
        }
        at += size;
    }
    c
}

/// The key that fits the notes heard best, by the key profiles.
fn key(left: &[f32], right: &[f32], rate: u32) -> Option<KeyGuess> {
    key_of(&chroma(left, right, rate))
}

/// The key that fits `c`, how much of each of the twelve notes (C
/// first) sounds, best by the key profiles.
pub fn key_of(c: &[f32; 12]) -> Option<KeyGuess> {
    if c.iter().sum::<f32>() < 1e-3 {
        return None;
    }
    let mut scores: Vec<(f32, u8, bool)> = Vec::with_capacity(24);
    for root in 0..12 {
        for (minor, profile) in [(false, &MAJOR), (true, &MINOR)] {
            let mut rotated = [0.0; 12];
            for (i, v) in rotated.iter_mut().enumerate() {
                *v = profile[(i + 12 - root) % 12];
            }
            scores.push((pearson(c, &rotated), root as u8, minor));
        }
    }
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, root, minor) = scores[0];
    (best > 0.3).then(|| KeyGuess {
        root,
        minor,
        margin: best - scores[1].0,
    })
}

/// How sharply each short step of the audio gets louder, at
/// `STEPS_PER_SECOND`: its rise above the loudest of the steps just
/// before it. A sound that dips and comes back (notes beating against
/// each other in a held chord) only returns to where it was, so it
/// scores nothing; a new hit rises above what came before.
pub(crate) fn attack_curve(left: &[f32], right: &[f32], rate: u32) -> Vec<f32> {
    const BEFORE: usize = 12; // 60 ms
    let hop = (rate / STEPS_PER_SECOND).max(1) as usize;
    let levels: Vec<f32> = (left.chunks(hop).zip(right.chunks(hop)))
        .map(|(l, r)| {
            let e: f32 = l.iter().zip(r).map(|(a, b)| a * a + b * b).sum::<f32>() / hop as f32;
            10.0 * (e + 1e-10).log10()
        })
        .collect();
    (0..levels.len())
        .map(|i| {
            // Leaving out the last two steps, so a hit that takes a
            // few steps to arrive counts all of its rise.
            let before = levels[i.saturating_sub(BEFORE)..i.saturating_sub(2)]
                .iter()
                .copied();
            before
                .reduce(f32::max)
                .map_or(0.0, |p| (levels[i] - p).max(0.0))
        })
        .collect()
}

pub(crate) const STEPS_PER_SECOND: u32 = 200;

/// The beat that repeats most in the attacks, from 60 to 200 bpm,
/// leaning towards the usual middle of that range.
fn tempo(left: &[f32], right: &[f32], rate: u32) -> Option<TempoGuess> {
    let mut a = attack_curve(left, right, rate);
    // At least a few seconds, with attacks all through them: sudden
    // rises of 3 dB or more above what came just before, at least one
    // every two seconds. A held note that swells and fades (a pad, a
    // tremolo) has none.
    let secs = a.len() / STEPS_PER_SECOND as usize;
    let attacks = (a.windows(2)).filter(|w| w[0] < 3.0 && w[1] >= 3.0).count();
    if secs < 4 || attacks < 8.max(secs / 2) {
        return None;
    }
    let mean = a.iter().sum::<f32>() / a.len() as f32;
    for v in &mut a {
        *v -= mean;
    }
    let zero: f32 = a.iter().map(|v| v * v).sum();
    if zero <= 1e-6 {
        return None;
    }
    let lag_score = |lag: f64| -> f32 {
        let (i, frac) = (lag.floor() as usize, (lag - lag.floor()) as f32);
        let at = |l: usize| -> f32 {
            (a.iter().zip(&a[l.min(a.len())..]))
                .map(|(x, y)| x * y)
                .sum()
        };
        (1.0 - frac) * at(i) + frac * at(i + 1)
    };
    let fps = STEPS_PER_SECOND as f64;
    let mut best = (0.0f32, 0.0f64, 0.0f32);
    let mut bpm = 60.0;
    while bpm <= 200.0 {
        let lag = 60.0 / bpm * fps;
        // A beat's multiples repeat too, so they count in its favour.
        let raw = lag_score(lag) + 0.5 * lag_score(2.0 * lag);
        let lean = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp() as f32;
        if raw * lean > best.0 {
            best = (raw * lean, bpm, raw);
        }
        bpm += 0.25;
    }
    let strength = (best.2 / (1.5 * zero)).clamp(0.0, 1.0);
    (best.0 > 0.0 && strength > 0.05).then(|| TempoGuess {
        bpm: (best.1 * 4.0).round() / 4.0,
        strength,
    })
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

    fn note(midi: f32) -> f32 {
        440.0 * 2f32.powf((midi - 69.0) / 12.0)
    }

    /// Notes one after another, each with its third and fifth.
    fn chords(roots: &[(f32, bool)], secs_each: f32) -> Vec<f32> {
        let mut out = Vec::new();
        for &(root, minor) in roots {
            let third = if minor { 3.0 } else { 4.0 };
            let n = (secs_each * RATE as f32) as usize;
            out.extend((0..n).map(|i| {
                let t = i as f32 / RATE as f32;
                [0.0, third, 7.0]
                    .iter()
                    .map(|s| 0.2 * (std::f32::consts::TAU * note(root + s) * t).sin())
                    .sum::<f32>()
            }));
        }
        out
    }

    #[test]
    fn the_key_of_chords_is_heard() {
        // I IV V I in C major.
        let x = chords(
            &[(60.0, false), (65.0, false), (67.0, false), (60.0, false)],
            1.0,
        );
        let k = analyse(&x, &x, RATE).key.unwrap();
        assert_eq!((k.root, k.minor), (0, false), "{k:?}");
        // i iv v i in A minor.
        let x = chords(
            &[(57.0, true), (62.0, true), (64.0, true), (57.0, true)],
            1.0,
        );
        let k = analyse(&x, &x, RATE).key.unwrap();
        assert_eq!((k.root, k.minor), (9, true), "{k:?}");
        let z = vec![0.0; RATE as usize * 2];
        assert_eq!(analyse(&z, &z, RATE).key, None);
    }

    #[test]
    fn a_loud_low_bass_does_not_pull_the_key_a_semitone_up() {
        // Song test 3: i iv v i in A minor over a loud bass an octave
        // or two down (A1 is 55 Hz, where a bin is wider than a
        // semitone) read as A♯ minor.
        let roots = [(57.0, true), (62.0, true), (64.0, true), (57.0, true)];
        let x = chords(&roots, 1.0);
        let n = RATE as usize;
        let bass: Vec<f32> = (roots.iter())
            .flat_map(|&(r, _)| {
                let hz = note(r - 24.0);
                (0..n)
                    .map(move |i| 0.8 * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin())
            })
            .collect();
        let x: Vec<f32> = x.iter().zip(&bass).map(|(a, b)| a + b).collect();
        let k = analyse(&x, &x, RATE).key.unwrap();
        assert_eq!((k.root, k.minor), (9, true), "{k:?}");
    }

    #[test]
    fn the_beat_of_clicks_is_heard() {
        for bpm in [90.0, 120.0, 150.0] {
            let beat = (60.0 / bpm * RATE as f64) as usize;
            let x: Vec<f32> = (0..RATE as usize * 10)
                .map(|i| {
                    let k = i % beat;
                    if k < 480 {
                        0.8 * (1.0 - k as f32 / 480.0) * (k as f32 * 0.7).sin()
                    } else {
                        0.0
                    }
                })
                .collect();
            let t = analyse(&x, &x, RATE).tempo.unwrap();
            assert!((t.bpm - bpm).abs() <= 1.0, "{bpm}: {t:?}");
        }
        let x = sine(440.0, 0.5, 6.0);
        assert_eq!(
            analyse(&x, &x, RATE).tempo,
            None,
            "a steady tone has no beat"
        );
        // Nor does a held chord that swells and fades, as a pad with a
        // slow wobble does.
        let x: Vec<f32> = (chords(&[(57.0, true)], 8.0).iter().enumerate())
            .map(|(i, v)| {
                let t = i as f32 / RATE as f32;
                v * (0.75 + 0.25 * (std::f32::consts::TAU * 2.2 * t).sin())
            })
            .collect();
        assert_eq!(
            analyse(&x, &x, RATE).tempo,
            None,
            "a swelling pad has no beat"
        );
        // Hits over that held chord still keep their beat.
        let pad = chords(&[(57.0, true)], 8.0);
        let beat = (60.0 / 100.0 * RATE as f64) as usize;
        let x: Vec<f32> = (pad.iter().enumerate())
            .map(|(i, v)| {
                let k = (i % beat) as f32;
                v + 1.0 * (-k / 2000.0).exp() * (k * 0.05).sin()
            })
            .collect();
        let t = analyse(&x, &x, RATE).tempo.unwrap();
        assert!((t.bpm - 100.0).abs() <= 1.0, "hits over a pad: {t:?}");
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
