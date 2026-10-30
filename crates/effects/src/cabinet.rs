// SPDX-License-Identifier: GPL-3.0-or-later
//! Speaker cabinets: what a guitar amp's speaker and its box do to the
//! sound, heard through a microphone in front of it. An amp's output
//! straight is harsh and fizzy; the speaker rolls off the top steeply,
//! the box adds a low thump and the cone its own peaks and dips.
//!
//! Each cabinet made here is an impulse response built from our own
//! filter shapes (second-order sections from the cookbook formulas, see
//! `biquad`), with a short reflection or two for the box, and played by
//! the convolution engine the room reverb uses. A response of the user's
//! own (a recorded cabinet) can be used instead.

use crate::biquad::{Biquad, Coefs};
use crate::convolution::Convolution;
use crate::{Dsp, Param, Unit};

pub const CABINETS: &[&str] = &[
    "Small open back, one 10\"",
    "Open back, one 12\"",
    "Open back, two 12\"",
    "Closed back, four 12\"",
    "Bass, one 15\"",
];

pub const PARAMS: &[Param] = &[
    Param::new(
        "cabinet",
        "Cabinet",
        0.0,
        (CABINETS.len() - 1) as f64,
        1.0,
        Unit::Choice,
    )
    .choices(CABINETS),
    Param::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    Param::new("level", "Level", -24.0, 12.0, 0.0, Unit::Db),
];

/// How a cabinet made here sounds.
struct Shape {
    /// The low cut from the box: where, and how much it rings there.
    low: (f64, f64),
    /// Peaks (or, with a negative gain, dips): Hz, Q and dB.
    peaks: &'static [(f64, f64, f64)],
    /// The high cut from the cone: where, and how many sections deep.
    high: (f64, usize),
    /// Reflections from the box or the floor: ms after, and their level.
    echoes: &'static [(f64, f64)],
}

const SHAPES: [Shape; 5] = [
    Shape {
        low: (120.0, 0.9),
        peaks: &[(900.0, 1.0, -2.0), (1900.0, 1.2, 3.0), (3300.0, 2.0, 4.0)],
        high: (4600.0, 2),
        // An open back lets the back of the cone be heard, late and turned
        // over.
        echoes: &[(1.1, -0.22)],
    },
    Shape {
        low: (95.0, 1.0),
        peaks: &[
            (125.0, 1.5, 2.0),
            (700.0, 1.0, -3.0),
            (2500.0, 1.5, 4.0),
            (4100.0, 3.0, 3.0),
        ],
        high: (5200.0, 2),
        echoes: &[(1.4, -0.18)],
    },
    Shape {
        low: (85.0, 1.2),
        peaks: &[
            (110.0, 1.8, 3.0),
            (500.0, 1.0, -3.0),
            (2200.0, 1.2, 3.0),
            (3800.0, 2.5, 4.0),
        ],
        high: (5500.0, 2),
        // Two speakers a little apart reach the microphone a little apart.
        echoes: &[(0.12, 0.45), (1.6, -0.12)],
    },
    Shape {
        low: (80.0, 1.6),
        peaks: &[
            (105.0, 2.0, 4.0),
            (450.0, 0.9, -4.0),
            (1600.0, 1.5, 2.0),
            (3000.0, 2.0, 5.0),
            (4400.0, 4.0, -4.0),
        ],
        high: (5000.0, 3),
        echoes: &[(0.09, 0.3), (0.21, 0.2)],
    },
    Shape {
        low: (45.0, 1.0),
        peaks: &[(80.0, 1.2, 3.0), (800.0, 1.0, -2.0), (2000.0, 1.0, 2.0)],
        high: (3600.0, 3),
        echoes: &[],
    },
];

/// Seconds of response kept.
const SECONDS: f64 = 0.045;

/// The impulse response of cabinet `cab`, left and right, at `rate`.
pub(crate) fn response(cab: usize, rate: f64) -> [Vec<f32>; 2] {
    let s = &SHAPES[cab.min(SHAPES.len() - 1)];
    let mut chain: Vec<Biquad> = Vec::new();
    let mut add = |c: Coefs| chain.push(Biquad::new(c));
    add(Coefs::high_pass(s.low.0, s.low.1, rate));
    // A second, gentle low cut: below the box's ring, a speaker falls
    // away fast.
    add(Coefs::high_pass(s.low.0 * 0.7, 0.6, rate));
    for &(hz, q, db) in s.peaks {
        add(Coefs::peak(hz, q, db, rate));
    }
    for k in 0..s.high.1 {
        // Each section a little lower and softer, so the slope steepens
        // without a sharp edge.
        let hz = s.high.0 * (1.0 - 0.12 * k as f64);
        add(Coefs::low_pass(hz, 0.75 - 0.1 * k as f64, rate));
    }
    let len = (SECONDS * rate) as usize;
    let mut h: Vec<f32> = (0..len)
        .map(|n| {
            let x = if n == 0 { 1.0 } else { 0.0 };
            chain.iter_mut().fold(x, |x, f| f.run(x))
        })
        .collect();
    let dry = h.clone();
    for &(ms, g) in s.echoes {
        let at = (ms / 1000.0 * rate).round() as usize;
        for i in at..len {
            h[i] += dry[i - at] * g as f32;
        }
    }
    let fade = len / 5;
    for i in 0..fade {
        h[len - 1 - i] *= i as f32 / fade as f32;
    }
    let mut h = [h.clone(), h];
    level(&mut h, rate);
    h
}

/// Sets a cabinet's level so a guitar sounds about as loud through it
/// as without it: its gain, on average in dB across where a guitar's
/// notes and their first harmonics are (150 Hz to 4 kHz), is made 0 dB.
pub(crate) fn level(h: &mut [Vec<f32>; 2], rate: f64) {
    let points = 24;
    let mut sum = 0.0;
    for ch in h.iter() {
        for k in 0..points {
            let hz = 150.0 * (4000.0f64 / 150.0).powf(k as f64 / (points - 1) as f64);
            sum += gain_at(ch, hz, rate);
        }
    }
    let avg = sum / (2 * points) as f64;
    let scale = crate::gain(-avg) as f32;
    for x in h.iter_mut().flatten() {
        *x *= scale;
    }
}

/// The gain of `h` at `hz`, in dB.
fn gain_at(h: &[f32], hz: f64, rate: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    // A cabinet's response is short; what is after 50 ms hardly counts.
    for (n, x) in h.iter().take((0.05 * rate) as usize).enumerate() {
        let w = 2.0 * std::f64::consts::PI * hz * n as f64 / rate;
        re += *x as f64 * w.cos();
        im -= *x as f64 * w.sin();
    }
    crate::db(re.hypot(im))
}

/// Every cabinet made here, at `rate`.
pub(crate) fn all(rate: f64) -> Vec<[Vec<f32>; 2]> {
    (0..CABINETS.len()).map(|c| response(c, rate)).collect()
}

/// The cabinet effect: a speaker cabinet after an amp, or after an amp
/// capture made without one.
pub struct Cabinet {
    conv: Convolution,
    level: f32,
}

impl Cabinet {
    /// With the cabinets made here, and `loaded` (a response at `rate`)
    /// used instead if given.
    pub fn new(rate: f64, mut loaded: Option<[Vec<f32>; 2]>) -> Cabinet {
        // Loaded responses come at the rooms' level; brought to the
        // cabinets' here.
        if let Some(h) = &mut loaded {
            level(h, rate);
        }
        Cabinet {
            conv: Convolution::with(rate, all(rate), loaded),
            level: 1.0,
        }
    }
}

impl Dsp for Cabinet {
    fn update(&mut self, v: &[f64]) {
        self.conv.update(&[v[0], 0.0, v[1]]);
        self.level = crate::gain(v[2]) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.conv.process(left, right);
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x *= self.level;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cabinet passes a guitar's middle and cuts the fizz above the
    /// speaker and the rumble below the box.
    #[test]
    fn cabinets_cut_the_top_and_the_bottom() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for c in 0..CABINETS.len() {
                let [h, _] = response(c, rate);
                let mid = gain_at(&h, 1200.0, rate);
                assert!(
                    gain_at(&h, 12_000.0, rate) < mid - 24.0,
                    "cab {c} at {rate}"
                );
                assert!(gain_at(&h, 20.0, rate) < mid - 12.0, "cab {c} at {rate}");
                let tail = h[h.len() - 20..].iter().fold(0f32, |m, x| m.max(x.abs()));
                assert!(tail < 1e-3, "cab {c}: {tail}");
            }
        }
    }

    #[test]
    fn cabinets_differ() {
        let hs: Vec<_> = (0..CABINETS.len()).map(|c| response(c, 48_000.0)).collect();
        for a in 0..hs.len() {
            for b in a + 1..hs.len() {
                assert_ne!(hs[a][0], hs[b][0]);
            }
        }
    }
}
