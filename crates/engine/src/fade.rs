// SPDX-License-Identifier: GPL-3.0-or-later
//! Clip fades and gain (roadmap 2.1): how loud a clip plays at each of
//! its frames, from its gain, its gain envelope and the fades at its two
//! ends.

use std::sync::Arc;

use crate::graph::db_to_gain;

/// The shape of a fade, as it rises from silence to full level. A fade
/// out is the same shape played backwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    /// Level rises evenly. A crossfade of two straight fades keeps the
    /// level of the same sound steady.
    Linear,
    /// A quarter sine: the power stays steady through a crossfade
    /// between different sounds, so the join doesn't dip.
    #[default]
    EqualPower,
    /// Starts and ends gently, steepest in the middle.
    SCurve,
    /// Rises quickly, then levels off.
    Fast,
    /// Rises slowly, then quickly at the end.
    Slow,
}

impl Curve {
    pub const ALL: [Curve; 5] = [
        Curve::Linear,
        Curve::EqualPower,
        Curve::SCurve,
        Curve::Fast,
        Curve::Slow,
    ];

    /// The level at `x` of the way through a fade in, 0 to 1.
    #[inline]
    pub fn gain(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Linear => x,
            Curve::EqualPower => (x * std::f32::consts::FRAC_PI_2).sin(),
            Curve::SCurve => 0.5 - 0.5 * (x * std::f32::consts::PI).cos(),
            Curve::Fast => 1.0 - (1.0 - x) * (1.0 - x),
            Curve::Slow => x * x,
        }
    }
}

/// A fade at one end of a clip: how many frames it lasts, and its shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fade {
    pub len: usize,
    pub curve: Curve,
}

impl Fade {
    pub fn new(len: usize, curve: Curve) -> Fade {
        Fade { len, curve }
    }
}

/// A point on a clip's gain envelope: a level at a frame of its source,
/// so the line stays with the audio when the clip is trimmed, split or
/// slipped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainPoint {
    pub at: usize,
    pub db: f32,
}

/// The level, in dB, of an envelope (points in order) at source frame
/// `at`: straight lines in dB between points, and level before the first
/// and after the last. 0 dB with no points.
pub fn envelope_db(points: &[GainPoint], at: f64) -> f32 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    if at <= first.at as f64 {
        return first.db;
    }
    // The first point after `at`.
    let i = points.partition_point(|p| (p.at as f64) <= at);
    match points.get(i) {
        None => points[points.len() - 1].db,
        Some(b) => {
            let a = points[i - 1];
            let span = (b.at - a.at).max(1) as f64;
            let k = ((at - a.at as f64) / span) as f32;
            a.db + (b.db - a.db) * k
        }
    }
}

/// Everything that sets a clip's level, ready for the audio thread.
#[derive(Clone, Debug, Default)]
pub struct Shape {
    /// The clip's own gain, as a factor.
    pub gain: f32,
    pub fade_in: Fade,
    pub fade_out: Fade,
    /// In source frames, in order.
    pub envelope: Arc<[GainPoint]>,
}

impl Shape {
    /// A clip at `gain_db` with nothing else.
    pub fn flat(gain_db: f32) -> Shape {
        Shape {
            gain: db_to_gain(gain_db),
            ..Shape::default()
        }
    }

    /// Whether every frame plays at the clip's own gain.
    pub fn is_flat(&self) -> bool {
        self.fade_in.len == 0 && self.fade_out.len == 0 && self.envelope.is_empty()
    }

    /// The level of frame `rel` of a clip `len` frames long whose first
    /// frame is source frame `offset`.
    #[inline]
    pub fn at(&self, rel: usize, len: usize, offset: usize) -> f32 {
        let mut g = self.gain;
        if rel < self.fade_in.len {
            g *= self
                .fade_in
                .curve
                .gain(rel as f32 / self.fade_in.len as f32);
        }
        let left = len - rel;
        if left <= self.fade_out.len {
            // The last frame is the one before silence.
            g *= self
                .fade_out
                .curve
                .gain((left - 1) as f32 / self.fade_out.len as f32);
        }
        if !self.envelope.is_empty() {
            g *= db_to_gain(envelope_db(&self.envelope, (offset + rel) as f64));
        }
        g
    }

    /// Fades shortened so the two fit in a clip `len` frames long
    /// together, each keeping its share.
    pub fn fitted(mut self, len: usize) -> Shape {
        let (a, b) = (self.fade_in.len, self.fade_out.len);
        if a + b > len {
            let a2 = (a as f64 * len as f64 / (a + b) as f64).round() as usize;
            self.fade_in.len = a2.min(len);
            self.fade_out.len = len - self.fade_in.len;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_run_from_silence_to_full() {
        for c in Curve::ALL {
            assert_eq!(c.gain(0.0), 0.0, "{c:?}");
            assert!((c.gain(1.0) - 1.0).abs() < 1e-6, "{c:?}");
            let mut last = 0.0;
            for i in 1..=100 {
                let g = c.gain(i as f32 / 100.0);
                assert!(g >= last, "{c:?} rises");
                last = g;
            }
        }
    }

    #[test]
    fn crossfades_keep_level_or_power_steady() {
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            let (a, b) = (Curve::Linear.gain(x), Curve::Linear.gain(1.0 - x));
            assert!((a + b - 1.0).abs() < 1e-6);
            let (a, b) = (Curve::EqualPower.gain(x), Curve::EqualPower.gain(1.0 - x));
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
            let (a, b) = (Curve::SCurve.gain(x), Curve::SCurve.gain(1.0 - x));
            assert!((a + b - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn a_fade_starts_silent_and_ends_on_its_last_frame() {
        let s = Shape {
            gain: 1.0,
            fade_in: Fade::new(10, Curve::Linear),
            fade_out: Fade::new(10, Curve::Linear),
            envelope: Arc::new([]),
        };
        assert_eq!(s.at(0, 100, 0), 0.0);
        assert!((s.at(5, 100, 0) - 0.5).abs() < 1e-6);
        assert_eq!(s.at(50, 100, 0), 1.0);
        assert!((s.at(90, 100, 0) - 0.9).abs() < 1e-6);
        assert_eq!(s.at(99, 100, 0), 0.0);
    }

    #[test]
    fn envelopes_are_straight_lines_in_db_between_points() {
        let p = [
            GainPoint { at: 100, db: -6.0 },
            GainPoint { at: 200, db: 0.0 },
            GainPoint { at: 300, db: -12.0 },
        ];
        assert_eq!(envelope_db(&p, 0.0), -6.0);
        assert_eq!(envelope_db(&p, 150.0), -3.0);
        assert_eq!(envelope_db(&p, 200.0), 0.0);
        assert_eq!(envelope_db(&p, 250.0), -6.0);
        assert_eq!(envelope_db(&p, 1000.0), -12.0);
        assert_eq!(envelope_db(&[], 5.0), 0.0);
    }

    #[test]
    fn fades_that_overlap_share_the_clip() {
        let s = Shape {
            fade_in: Fade::new(300, Curve::Linear),
            fade_out: Fade::new(100, Curve::Linear),
            ..Shape::flat(0.0)
        }
        .fitted(200);
        assert_eq!((s.fade_in.len, s.fade_out.len), (150, 50));
    }
}
