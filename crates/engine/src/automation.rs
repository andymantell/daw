// SPDX-License-Identifier: GPL-3.0-or-later
//! Automation (roadmap 2.6): a setting moving along a line of points as
//! the song plays. The graph reads each track's lanes once a block and
//! sets the track's volume, pan, a send's level or one of a built-in
//! effect's settings from them, so playback and export follow the same
//! lines. Plugins' parameters are set from the UI thread instead (the
//! plugin's own controls must be told on that thread).

use std::sync::Arc;

/// Points joined by straight lines, held level before the first and
/// after the last.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    /// `(frame, value)`, in frame order.
    points: Vec<(i64, f32)>,
}

impl Curve {
    /// From points in any order; two at one frame keep the later.
    pub fn new(mut points: Vec<(i64, f32)>) -> Curve {
        points.sort_by_key(|p| p.0);
        points.dedup_by(|b, a| {
            if a.0 == b.0 {
                a.1 = b.1;
                true
            } else {
                false
            }
        });
        Curve { points }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn points(&self) -> &[(i64, f32)] {
        &self.points
    }

    /// The value at `at`, or `None` with no points.
    pub fn at(&self, at: i64) -> Option<f32> {
        let p = &self.points;
        let first = p.first()?;
        if at <= first.0 {
            return Some(first.1);
        }
        let last = p.last()?;
        if at >= last.0 {
            return Some(last.1);
        }
        let i = p.partition_point(|q| q.0 <= at);
        let (a, b) = (p[i - 1], p[i]);
        let k = (at - a.0) as f64 / (b.0 - a.0) as f64;
        Some(a.1 + (b.1 - a.1) * k as f32)
    }
}

/// What a lane sets on its track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The volume fader, in dB.
    Gain,
    /// Pan, −1 to +1.
    Pan,
    /// A send's level in dB, by its index in the track's sends.
    Send(usize),
    /// A setting of the insert with this id, by the setting's index.
    Insert { id: u64, param: u32 },
}

/// One lane as the engine plays it.
#[derive(Clone, Debug)]
pub struct LaneSpec {
    pub target: Target,
    pub curve: Arc<Curve>,
    /// The setting is being written: play the control as it is set, not
    /// the line (see `Command::SetAutoLive`).
    pub live: bool,
}

impl LaneSpec {
    pub fn new(target: Target, curve: Arc<Curve>) -> LaneSpec {
        LaneSpec {
            target,
            curve,
            live: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curve_holds_its_ends_and_joins_points_with_lines() {
        let c = Curve::new(vec![(100, 0.0), (0, -12.0), (200, 6.0)]);
        assert_eq!(c.at(-50), Some(-12.0));
        assert_eq!(c.at(0), Some(-12.0));
        assert_eq!(c.at(50), Some(-6.0));
        assert_eq!(c.at(100), Some(0.0));
        assert_eq!(c.at(150), Some(3.0));
        assert_eq!(c.at(1000), Some(6.0));
        assert_eq!(Curve::default().at(0), None);
    }

    #[test]
    fn two_points_at_one_frame_keep_the_later() {
        let c = Curve::new(vec![(10, 1.0), (10, 2.0)]);
        assert_eq!(c.points(), &[(10, 2.0)]);
    }
}
