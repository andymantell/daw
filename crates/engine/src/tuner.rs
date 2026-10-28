// SPDX-License-Identifier: GPL-3.0-or-later
//! Pitch detection for the chromatic tuner (roadmap 1.4).
//!
//! Uses the YIN method: de Cheveigné and Kawahara, "YIN, a fundamental
//! frequency estimator for speech and music", JASA 111(4), 2002. Runs on
//! the UI thread over the last few thousand input samples, which the
//! engine passes out through [`Controller::take_tuner_samples`]
//! (crate::engine::Controller::take_tuner_samples).

/// Lowest pitch looked for: below a five-string bass's low B (30.9 Hz).
pub const LOWEST_HZ: f32 = 30.0;
/// Highest: well above a guitar's 24th fret on the top string (1319 Hz).
pub const HIGHEST_HZ: f32 = 2000.0;

/// Samples [`Detector::detect`] wants at `rate`.
pub fn window(rate: u32) -> usize {
    // Two periods of the lowest pitch: one to compare, one to slide.
    2 * (rate as f32 / LOWEST_HZ).ceil() as usize
}

/// Finds the pitch of a block of audio. Holds its work buffers, so
/// repeated calls don't allocate.
pub struct Detector {
    rate: u32,
    diff: Vec<f32>,
}

/// Below this level (RMS) there is nothing to tune.
const QUIET: f32 = 1e-3;
/// YIN's absolute threshold on the normalised difference.
const THRESHOLD: f32 = 0.15;

impl Detector {
    pub fn new(rate: u32) -> Detector {
        Detector {
            rate,
            diff: Vec::new(),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The fundamental of the most recent [`window`] samples of
    /// `samples`, in Hz, or `None` if it is quiet or unclear.
    pub fn detect(&mut self, samples: &[f32]) -> Option<f32> {
        let rate = self.rate as f32;
        let w = window(self.rate).min(samples.len());
        let x = &samples[samples.len() - w..];
        let rms = (x.iter().map(|v| v * v).sum::<f32>() / w.max(1) as f32).sqrt();
        if rms < QUIET {
            return None;
        }
        let max_tau = ((rate / LOWEST_HZ) as usize).min(w / 2);
        let min_tau = ((rate / HIGHEST_HZ) as usize).max(2);
        if max_tau <= min_tau + 2 {
            return None;
        }
        let len = w - max_tau;
        // Difference function, cumulative-mean normalised as it goes.
        self.diff.clear();
        self.diff.resize(max_tau + 1, 1.0);
        let mut running = 0.0f32;
        for tau in 1..=max_tau {
            let mut d = 0.0f32;
            for j in 0..len {
                let e = x[j] - x[j + tau];
                d += e * e;
            }
            running += d;
            self.diff[tau] = if running > 0.0 {
                d * tau as f32 / running
            } else {
                1.0
            };
        }
        // The first dip under the threshold, followed to its bottom.
        let d = &self.diff;
        let mut tau = (min_tau..max_tau).find(|&t| d[t] < THRESHOLD)?;
        while tau + 1 < max_tau && d[tau + 1] < d[tau] {
            tau += 1;
        }
        // Parabolic interpolation between neighbours.
        let (a, b, c) = (d[tau - 1], d[tau], d[tau + 1]);
        let den = a - 2.0 * b + c;
        let shift = if den.abs() > f32::EPSILON {
            (0.5 * (a - c) / den).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        Some(rate / (tau as f32 + shift))
    }
}

/// A pitch as the nearest note of equal temperament.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    /// MIDI note number: 69 is the A above middle C.
    pub midi: i32,
    /// How far the pitch is from that note, −50 to +50.
    pub cents: f32,
}

const NAMES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

impl Note {
    /// The nearest note to `hz` when A4 is `a4` Hz.
    pub fn from_hz(hz: f32, a4: f32) -> Note {
        let semis = 12.0 * (hz / a4).log2() + 69.0;
        let midi = semis.round();
        Note {
            midi: midi as i32,
            cents: (semis - midi) * 100.0,
        }
    }

    pub fn name(&self) -> &'static str {
        NAMES[self.midi.rem_euclid(12) as usize]
    }

    /// Scientific pitch octave: middle C is C4.
    pub fn octave(&self) -> i32 {
        self.midi.div_euclid(12) - 1
    }
}

/// Smooths a stream of readings so the needle doesn't jitter, and
/// forgets the note once the string has stopped.
#[derive(Default)]
pub struct Smoother {
    hz: Option<f32>,
    misses: u32,
}

impl Smoother {
    /// Takes one reading; returns the pitch to show.
    pub fn push(&mut self, reading: Option<f32>) -> Option<f32> {
        match (reading, self.hz) {
            (Some(r), Some(h)) if (r / h).log2().abs() < 0.5 / 12.0 => {
                // Same note: ease towards it.
                self.hz = Some(h + 0.3 * (r - h));
                self.misses = 0;
            }
            (Some(r), _) => {
                self.hz = Some(r);
                self.misses = 0;
            }
            (None, _) => {
                self.misses += 1;
                if self.misses > 8 {
                    self.hz = None;
                }
            }
        }
        self.hz
    }
}

/// How far a recording sits from concert pitch at reference `a4`, in
/// cents (−50 to +50), judged from the spectral peaks of many frames: a
/// song recorded at A = 432 Hz reads about −32. Whole-semitone tunings
/// (E♭ standard, say) read as in tune, since every note still lands on a
/// semitone. `None` if there is too little steady pitch to tell.
///
/// Each peak's distance from the nearest semitone is averaged on a circle
/// (so +49 and −49 cents count as close), weighted by its level; the
/// spread of the result says how sure it is.
pub fn tuning_offset(samples: &[f32], rate: u32, a4: f32) -> Option<f32> {
    use realfft::RealFftPlanner;
    const N: usize = 8192;
    const HOP: usize = 4096;
    if samples.len() < N {
        return None;
    }
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos())
        .collect();
    let mut frame = vec![0.0f32; N];
    let mut spec = fft.make_output_vec();
    let mut mag = vec![0.0f32; spec.len()];
    let bin_hz = rate as f32 / N as f32;
    let (lo, hi) = (
        (80.0 / bin_hz) as usize,
        ((2000.0 / bin_hz) as usize).min(mag.len() - 2),
    );
    let (mut sx, mut sy, mut total) = (0.0f64, 0.0f64, 0.0f64);
    for start in (0..samples.len() - N).step_by(HOP) {
        for (f, (x, w)) in frame.iter_mut().zip(samples[start..].iter().zip(&window)) {
            *f = x * w;
        }
        fft.process(&mut frame, &mut spec).ok()?;
        for (m, c) in mag.iter_mut().zip(&spec) {
            *m = c.norm();
        }
        let loudest = mag[lo..hi].iter().fold(0.0f32, |a, &b| a.max(b));
        if loudest < 1e-3 {
            continue;
        }
        for k in lo.max(2)..hi {
            let m = mag[k];
            // Clear local peaks only: louder than both neighbours either side.
            if m < loudest * 0.05
                || m <= mag[k - 1]
                || m < mag[k + 1]
                || m <= mag[k - 2]
                || m < mag[k + 2]
            {
                continue;
            }
            // Parabolic interpolation on log magnitude.
            let (a, b, c) = (mag[k - 1].ln(), m.ln(), mag[k + 1].ln());
            let d = a - 2.0 * b + c;
            let shift = if d.abs() > 1e-9 {
                0.5 * (a - c) / d
            } else {
                0.0
            };
            let hz = (k as f32 + shift) * bin_hz;
            let semis = 12.0 * (hz / a4).log2();
            let dev = (semis - semis.round()) as f64; // −0.5..0.5
            let angle = dev * std::f64::consts::TAU;
            let w = m as f64;
            sx += w * angle.cos();
            sy += w * angle.sin();
            total += w;
        }
    }
    if total <= 0.0 {
        return None;
    }
    // How tightly the deviations agree: 1 is all the same.
    let r = (sx * sx + sy * sy).sqrt() / total;
    if r < 0.3 {
        return None;
    }
    Some((sy.atan2(sx) / std::f64::consts::TAU * 100.0) as f32)
}
