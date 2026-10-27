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
