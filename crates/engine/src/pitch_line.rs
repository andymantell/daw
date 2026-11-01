// SPDX-License-Identifier: GPL-3.0-or-later
//! The pitch line of a single voice or single-note line, and the notes
//! in it (plan B2, pitch editing): the pitch every 10 ms, found with
//! the tuner's YIN detector ([`crate::tuner::Detector`]) on audio
//! low-passed and taken down to about 16 kHz so a long take is quick,
//! then cut into notes where the pitch moves to another note and
//! stays, where the voice stops, or where the level dips and comes back
//! (the same note sung twice).
//!
//! Each note's pitch is the median of its line. The line is smoothed
//! over about a vibrato cycle before cutting, so vibrato doesn't split a
//! note, and a five-point median removes single-hop octave slips.

use crate::bend::PitchLine;
use crate::tuner::Detector;

/// Time between pitch readings.
pub const HOP_SECONDS: f64 = 0.01;
/// The range of pitches looked for: below a bass voice's low E (82 Hz)
/// and a bass guitar's low E (41 Hz), to above a soprano's top C.
pub const LOWEST_HZ: f32 = 40.0;
pub const HIGHEST_HZ: f32 = 1_400.0;
/// Readings this far under the take's loudest are no pitch at all.
const QUIET_DB: f32 = -40.0;
/// A note at least this long, in seconds; shorter pieces are dropped
/// or joined to their neighbour.
const MIN_NOTE_SECONDS: f64 = 0.06;
/// The line moving this far (in semitones) from the note's pitch, and
/// staying for [`MOVE_SECONDS`], starts a new note.
const NEW_NOTE_SEMITONES: f32 = 0.7;
const MOVE_SECONDS: f64 = 0.04;
/// Gaps without pitch this short don't end a note: a consonant inside
/// a word, a reading missed.
const BRIDGE_SECONDS: f64 = 0.03;
/// The smoothing used to tell a new note from vibrato.
const SMOOTH_SECONDS: f64 = 0.12;

/// A note found in a pitch line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineNote {
    /// Frames of the audio it covers, at the audio's rate.
    pub start: i64,
    pub end: i64,
    /// Its pitch, as a MIDI number with cents as the fraction.
    pub midi: f32,
}

/// The pitch line of `samples` (mono) at `rate`.
pub fn line(samples: &[f32], rate: u32) -> PitchLine {
    let down = (rate / 16_000).max(1) as usize;
    let low_rate = rate / down as u32;
    let low = decimate(samples, down);
    let hop_low = ((low_rate as f64 * HOP_SECONDS).round() as usize).max(1);
    let hop = (hop_low * down) as u32;
    let mut det = Detector::with_range(low_rate, LOWEST_HZ, HIGHEST_HZ);
    let w = det.window();
    let n = samples.len() / hop as usize + 1;
    // Each reading is centred on its hop.
    let mut level = vec![0.0f32; n];
    let mut hz = vec![None; n];
    let mut padded = vec![0.0f32; w];
    for (i, (lv, h)) in level.iter_mut().zip(hz.iter_mut()).enumerate() {
        let c = (i * hop_low) as i64;
        let from = c - (w / 2) as i64;
        for (k, p) in padded.iter_mut().enumerate() {
            let at = from + k as i64;
            *p = if at >= 0 {
                low.get(at as usize).copied().unwrap_or(0.0)
            } else {
                0.0
            };
        }
        *lv = (padded.iter().map(|v| v * v).sum::<f32>() / w as f32).sqrt();
        *h = det.detect(&padded);
    }
    let loudest = level.iter().fold(0.0f32, |a, &b| a.max(b));
    let floor = loudest * 10f32.powf(QUIET_DB / 20.0);
    let raw: Vec<f32> = (hz.iter().zip(&level))
        .map(|(h, &lv)| match h {
            Some(h) if lv > floor && lv > 1e-4 => 12.0 * (h / 440.0).log2() + 69.0,
            _ => f32::NAN,
        })
        .collect();
    PitchLine {
        rate,
        hop,
        midi: median5(&raw),
    }
}

/// The notes in `line`, in order, not overlapping. `samples` and
/// `rate` are the audio it came from, for the level dips between
/// repeated notes.
pub fn notes(line: &PitchLine, samples: &[f32]) -> Vec<LineNote> {
    let hop = line.hop.max(1) as usize;
    let secs = |s: f64| ((s * line.rate as f64) / hop as f64).round().max(1.0) as usize;
    let n = line.midi.len();
    let level = levels(samples, hop, n);
    let voiced: Vec<bool> = line.midi.iter().map(|v| v.is_finite()).collect();
    // Runs of pitch, bridging short gaps.
    let bridge = secs(BRIDGE_SECONDS);
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if !voiced[i] {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        let mut j = i + 1;
        while j < n {
            if voiced[j] {
                end = j + 1;
                j += 1;
            } else if j - end < bridge {
                j += 1;
            } else {
                break;
            }
        }
        runs.push((start, end));
        i = end.max(j);
    }
    let smooth = smoothed(&line.midi, secs(SMOOTH_SECONDS) / 2);
    let hold = secs(MOVE_SECONDS);
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    for (a, b) in runs {
        let mut cuts = vec![a];
        // Where the pitch moves to another note and stays.
        let mut start = a;
        let mut k = a;
        while k < b {
            let here = note_pitch(&smooth[start..=k.max(start)]);
            let off = |m: usize| {
                smooth[m].is_finite()
                    && here.is_some_and(|p| (smooth[m] - p).abs() > NEW_NOTE_SEMITONES)
            };
            if k + hold <= b && (k..k + hold).all(off) {
                cuts.push(k);
                start = k;
                k += hold;
            } else {
                k += 1;
            }
        }
        // Where the level dips and comes back: the same note again.
        let span = secs(0.15);
        for m in a + 1..b.saturating_sub(1) {
            let lv = level[m];
            if lv > level[m - 1] || lv > level[m + 1] {
                continue;
            }
            let before = level[m.saturating_sub(span)..m]
                .iter()
                .fold(0.0f32, |x, &y| x.max(y));
            let after = level[m + 1..(m + 1 + span).min(b)]
                .iter()
                .fold(0.0f32, |x, &y| x.max(y));
            if lv < 0.35 * before.min(after) && cuts.iter().all(|&c| c.abs_diff(m) > hold) {
                cuts.push(m);
            }
        }
        cuts.sort_unstable();
        cuts.push(b);
        for w in cuts.windows(2) {
            if w[1] > w[0] {
                pieces.push((w[0], w[1]));
            }
        }
    }
    // Drop or join pieces too short to be notes.
    let min = secs(MIN_NOTE_SECONDS);
    let mut out: Vec<LineNote> = Vec::new();
    for (a, b) in pieces {
        let Some(p) = note_pitch(&line.midi[a..b]) else {
            continue;
        };
        let (start, end) = ((a * hop) as i64, (b * hop) as i64);
        if b - a < min {
            // Joined to the note just before when it runs straight on
            // at about its pitch; otherwise dropped.
            if let Some(last) = out.last_mut()
                && last.end == start
                && (last.midi - p).abs() < NEW_NOTE_SEMITONES
            {
                last.end = end;
            }
            continue;
        }
        out.push(LineNote {
            start,
            end,
            midi: p,
        });
    }
    out
}

/// The pitch a stretch of line sits on: the median of its readings.
fn note_pitch(v: &[f32]) -> Option<f32> {
    let mut x: Vec<f32> = v.iter().copied().filter(|v| v.is_finite()).collect();
    if x.is_empty() {
        return None;
    }
    x.sort_by(f32::total_cmp);
    Some(x[x.len() / 2])
}

/// The level (RMS) of each hop of `samples`.
fn levels(samples: &[f32], hop: usize, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let a = (i * hop).saturating_sub(hop / 2).min(samples.len());
            let b = (a + hop).min(samples.len());
            if b <= a {
                return 0.0;
            }
            (samples[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a) as f32).sqrt()
        })
        .collect()
}

/// A moving average over `half` readings either side, of readings with
/// a pitch; NaN stays NaN.
pub fn smoothed(v: &[f32], half: usize) -> Vec<f32> {
    (0..v.len())
        .map(|i| {
            if !v[i].is_finite() {
                return f32::NAN;
            }
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(v.len()));
            let (sum, k) = v[a..b]
                .iter()
                .filter(|x| x.is_finite())
                .fold((0.0f32, 0usize), |(s, k), &x| (s + x, k + 1));
            sum / k as f32
        })
        .collect()
}

/// A five-point median over readings with a pitch, taking out single
/// readings an octave off.
fn median5(v: &[f32]) -> Vec<f32> {
    (0..v.len())
        .map(|i| {
            if !v[i].is_finite() {
                return f32::NAN;
            }
            let (a, b) = (i.saturating_sub(2), (i + 3).min(v.len()));
            note_pitch(&v[a..b]).unwrap_or(f32::NAN)
        })
        .collect()
}

/// `x` low-passed and kept one sample in `down`.
fn decimate(x: &[f32], down: usize) -> Vec<f32> {
    if down <= 1 {
        return x.to_vec();
    }
    // A Hann-windowed sinc at 90% of the new Nyquist.
    const TAPS: usize = 31;
    let fc = 0.9 / down as f32;
    let half = (TAPS / 2) as f32;
    let k: Vec<f32> = (0..TAPS)
        .map(|j| {
            let t = j as f32 - half;
            let s = if t == 0.0 {
                fc
            } else {
                (std::f32::consts::PI * fc * t).sin() / (std::f32::consts::PI * t)
            };
            let w = 0.5 + 0.5 * (std::f32::consts::PI * t / (half + 1.0)).cos();
            s * w
        })
        .collect();
    (0..x.len().div_ceil(down))
        .map(|o| {
            let c = (o * down) as i64;
            k.iter()
                .enumerate()
                .map(|(j, w)| {
                    let at = c + j as i64 - half as i64;
                    if at >= 0 && (at as usize) < x.len() {
                        x[at as usize] * w
                    } else {
                        0.0
                    }
                })
                .sum()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: u32 = 48_000;

    /// A sung-like line: each note `(midi, seconds)` with a few
    /// harmonics, a vibrato of `vib` semitones at 5.5 Hz, and a short
    /// gap of silence before each note when `gaps`.
    fn sing(notes: &[(f32, f32)], vib: f32, gaps: bool) -> (Vec<f32>, Vec<(i64, i64)>) {
        let mut out = Vec::new();
        let mut spans = Vec::new();
        let mut phase = 0.0f32;
        for &(midi, secs) in notes {
            if gaps {
                out.extend(std::iter::repeat_n(0.0, (RATE as f32 * 0.08) as usize));
            }
            let start = out.len() as i64;
            let n = (secs * RATE as f32) as usize;
            for t in 0..n {
                let s = t as f32 / RATE as f32;
                let m = midi + vib * (TAU * 5.5 * s).sin();
                let hz = 440.0 * 2f32.powf((m - 69.0) / 12.0);
                phase += TAU * hz / RATE as f32;
                // Soft attack and release.
                let env = (s / 0.02).min(1.0) * ((secs - s) / 0.02).clamp(0.0, 1.0);
                out.push(
                    env * (0.4 * phase.sin()
                        + 0.2 * (2.0 * phase).sin()
                        + 0.1 * (3.0 * phase).sin()),
                );
            }
            spans.push((start, out.len() as i64));
        }
        out.extend(std::iter::repeat_n(0.0, RATE as usize / 10));
        (out, spans)
    }

    #[test]
    fn a_steady_tone_reads_as_its_pitch() {
        let (x, _) = sing(&[(57.0, 0.5)], 0.0, true);
        let l = line(&x, RATE);
        assert_eq!(l.hop, 480);
        let mid = l.hop_of(((0.08 + 0.25) * RATE as f64) as i64);
        let v = l.get(mid).expect("a pitch");
        assert!((v - 57.0).abs() < 0.05, "{v}");
        // Silence has none.
        assert!(l.get(1).is_none());
    }

    #[test]
    fn notes_with_gaps_and_vibrato_are_found_one_each() {
        let tune = [(60.0, 0.4), (64.0, 0.3), (62.0, 0.5), (67.0, 0.35)];
        let (x, spans) = sing(&tune, 0.4, true);
        let l = line(&x, RATE);
        let found = notes(&l, &x);
        assert_eq!(found.len(), tune.len(), "{found:?}");
        for ((f, (midi, _)), (a, b)) in found.iter().zip(tune).zip(spans) {
            assert!((f.midi - midi).abs() < 0.25, "{f:?} want {midi}");
            assert!((f.start - a).abs() < 1_500, "start {} want {a}", f.start);
            assert!((f.end - b).abs() < 1_500, "end {} want {b}", f.end);
        }
    }

    #[test]
    fn a_legato_line_is_cut_where_the_pitch_moves() {
        let tune = [(55.0, 0.4), (57.0, 0.4), (59.0, 0.4)];
        let (x, spans) = sing(&tune, 0.2, false);
        let l = line(&x, RATE);
        let found = notes(&l, &x);
        assert_eq!(found.len(), 3, "{found:?}");
        for (f, (a, _)) in found.iter().zip(spans).skip(1) {
            assert!((f.start - a).abs() < 2_400, "start {} want {a}", f.start);
        }
    }

    #[test]
    fn the_same_note_twice_is_two_notes() {
        let tune = [(62.0, 0.3), (62.0, 0.3)];
        // No gap, but each note fades in and out.
        let (x, _) = sing(&tune, 0.0, false);
        let found = notes(&line(&x, RATE), &x);
        assert_eq!(found.len(), 2, "{found:?}");
    }
}
