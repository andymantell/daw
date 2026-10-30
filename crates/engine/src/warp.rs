// SPDX-License-Identifier: GPL-3.0-or-later
//! Stretching a clip between markers (plan A7, the first step of audio
//! that follows tempo): a [`Warp`] ties frames of a source to frames of
//! the output, and a [`Warper`] plays the source through a stretcher so
//! that each tied frame lands where the warp says, with the audio
//! between stretched evenly. Pitch editing (B2) renders through the same
//! calls.
//!
//! The stretcher sits behind [`Stretch`]: our own phase vocoder for now,
//! and whichever library the owner picks after the spike 0.6 listening
//! would go behind the same calls.
//!
//! A warp is in frames at the project's rate on both sides. A clip in
//! musical time is turned into one by the app: each stretch marker's
//! beat position becomes a frame through the tempo map, with extra
//! points inside tempo glides, so the engine never needs the map.

use std::fmt;

use crate::stretch::{FULL_SPEED_RANGE, Feed, Stretcher};

/// The calls a stretcher offers, behind which the library choice sits.
pub trait Stretch: Send {
    /// Sets speed (source frames per output frame) and transpose in
    /// semitones, from the next frame made.
    fn set(&mut self, speed: f64, semitones: f64);
    /// Starts again so that the next output frame plays input `u0`.
    fn reset(&mut self, u0: f64);
    /// Fills `l` and `r` at the current settings, reading input from
    /// `feed`. Must not allocate.
    fn process(&mut self, feed: &mut dyn Feed, l: &mut [f32], r: &mut [f32]);
    /// Input position the next output frame plays.
    fn position(&self) -> f64;
    /// Fills `l` and `r` so that output frame `o` of them plays input
    /// `at(o)`, as nearly as the stretcher can; `at` may be asked about
    /// frames past the block. A stretcher driven only by speed plays the
    /// block at its average speed, taking up any drift from where it is.
    fn process_along(
        &mut self,
        feed: &mut dyn Feed,
        at: &dyn Fn(f64) -> f64,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        let n = l.len().min(r.len());
        if n == 0 {
            return;
        }
        let speed = (at(n as f64) - self.position()) / n as f64;
        self.set(
            speed.clamp(FULL_SPEED_RANGE.0, FULL_SPEED_RANGE.1),
            self.semitones(),
        );
        self.process(feed, l, r);
    }
    /// The transpose last set.
    fn semitones(&self) -> f64;
}

impl Stretch for Stretcher {
    fn set(&mut self, speed: f64, semitones: f64) {
        Stretcher::set(self, speed, semitones)
    }

    fn reset(&mut self, u0: f64) {
        Stretcher::reset(self, u0)
    }

    fn process(&mut self, mut feed: &mut dyn Feed, l: &mut [f32], r: &mut [f32]) {
        Stretcher::process(self, &mut feed, l, r)
    }

    fn position(&self) -> f64 {
        Stretcher::position(self)
    }

    fn process_along(
        &mut self,
        mut feed: &mut dyn Feed,
        at: &dyn Fn(f64) -> f64,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        Stretcher::process_along(self, &mut feed, at, l, r)
    }

    fn semitones(&self) -> f64 {
        Stretcher::semitones(self)
    }
}

/// The stretcher in use, for blocks of up to `max_block` output frames.
/// Allocates; nothing after does.
pub fn stretcher(max_block: usize) -> Box<dyn Stretch> {
    Box::new(Stretcher::new(max_block))
}

/// A source frame tied to an output frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarpPoint {
    pub src: i64,
    pub out: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WarpError {
    /// A warp needs at least two points.
    TooFew,
    /// Points must go forwards in both the source and the output.
    OutOfOrder,
    /// A stretch steeper than the stretcher takes.
    TooSteep,
}

impl fmt::Display for WarpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            WarpError::TooFew => "a warp needs at least two points",
            WarpError::OutOfOrder => "warp points must go forwards in the source and the output",
            WarpError::TooSteep => "a stretch that steep is not supported",
        })
    }
}

impl std::error::Error for WarpError {}

/// Frames of a source tied to frames of the output, in order, with the
/// audio between two points stretched evenly. Before the first point
/// and after the last, the end stretches carry on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warp {
    points: Vec<WarpPoint>,
}

impl Warp {
    /// A warp through `points`, which must be at least two and go
    /// forwards on both sides, with every stretch within
    /// [`FULL_SPEED_RANGE`].
    pub fn new(points: Vec<WarpPoint>) -> Result<Warp, WarpError> {
        if points.len() < 2 {
            return Err(WarpError::TooFew);
        }
        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b.src <= a.src || b.out <= a.out {
                return Err(WarpError::OutOfOrder);
            }
            let speed = (b.src - a.src) as f64 / (b.out - a.out) as f64;
            if !(FULL_SPEED_RANGE.0..=FULL_SPEED_RANGE.1).contains(&speed) {
                return Err(WarpError::TooSteep);
            }
        }
        Ok(Warp { points })
    }

    /// Source frames `src` played from output frame `out` at `speed`
    /// source frames per output frame.
    pub fn even(src: std::ops::Range<i64>, out: i64, speed: f64) -> Result<Warp, WarpError> {
        let len = src.end - src.start;
        if len <= 0 || speed <= 0.0 || speed.is_nan() {
            return Err(WarpError::OutOfOrder);
        }
        let out_len = ((len as f64 / speed).round() as i64).max(1);
        Warp::new(vec![
            WarpPoint {
                src: src.start,
                out,
            },
            WarpPoint {
                src: src.end,
                out: out + out_len,
            },
        ])
    }

    pub fn points(&self) -> &[WarpPoint] {
        &self.points
    }

    /// The output frames the points span: the first to the last.
    pub fn out_range(&self) -> std::ops::Range<i64> {
        self.points[0].out..self.points[self.points.len() - 1].out
    }

    /// The source frames the points span.
    pub fn src_range(&self) -> std::ops::Range<i64> {
        self.points[0].src..self.points[self.points.len() - 1].src
    }

    /// The stretch in force at output `out`: the one between the points
    /// either side of it (the first or last stretch beyond the ends).
    fn segment(&self, out: f64) -> (WarpPoint, WarpPoint) {
        let n = self.points.len();
        // The first point after `out`, kept within the points.
        let i = self
            .points
            .partition_point(|p| (p.out as f64) <= out)
            .clamp(1, n - 1);
        (self.points[i - 1], self.points[i])
    }

    /// The source frame output frame `out` plays.
    pub fn src_at(&self, out: f64) -> f64 {
        let (a, b) = self.segment(out);
        a.src as f64 + (out - a.out as f64) * self.speed_of(a, b)
    }

    /// Source frames per output frame at output `out`.
    pub fn speed_at(&self, out: f64) -> f64 {
        let (a, b) = self.segment(out);
        self.speed_of(a, b)
    }

    fn speed_of(&self, a: WarpPoint, b: WarpPoint) -> f64 {
        (b.src - a.src) as f64 / (b.out - a.out) as f64
    }

    /// The output frame source frame `src` lands on.
    pub fn out_at(&self, src: f64) -> f64 {
        let n = self.points.len();
        let i = self
            .points
            .partition_point(|p| (p.src as f64) <= src)
            .clamp(1, n - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        a.out as f64 + (src - a.src as f64) / self.speed_of(a, b)
    }
}

/// Plays a source through a stretcher so that it follows a [`Warp`]:
/// each frame the stretcher makes is centred where the warp says, so a
/// tied frame lands on its output frame and the audio between is
/// stretched evenly. Allocates in [`Warper::new`]; nothing after does.
pub struct Warper {
    st: Box<dyn Stretch>,
    semitones: f64,
    /// The output frame the next call continues from, if it does.
    next: Option<f64>,
}

impl Warper {
    /// A warper for blocks of up to `max_block` output frames.
    pub fn new(max_block: usize) -> Warper {
        Warper {
            st: stretcher(max_block),
            semitones: 0.0,
            next: None,
        }
    }

    /// Transposes the output by `semitones` (fractions for cents).
    pub fn set_pitch(&mut self, semitones: f64) {
        self.semitones = semitones;
    }

    /// Forget where playback was: the next call starts afresh.
    pub fn stop(&mut self) {
        self.next = None;
    }

    /// Whether the next call would carry on from output `out`.
    pub fn continues_at(&self, out: f64) -> bool {
        self.next.is_some_and(|n| (n - out).abs() < 0.5)
    }

    /// Fills `l` and `r` with output frames from `out` on, reading the
    /// source through `feed`. Carries on from the last call when `out`
    /// follows it; otherwise starts afresh there.
    pub fn process(
        &mut self,
        feed: &mut dyn Feed,
        warp: &Warp,
        out: f64,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        let n = l.len().min(r.len());
        if n == 0 {
            return;
        }
        if !self.continues_at(out) {
            self.st.set(warp.speed_at(out), self.semitones);
            self.st.reset(warp.src_at(out));
        } else if self.st.semitones() != self.semitones {
            self.st.set(warp.speed_at(out), self.semitones);
        }
        self.st.process_along(feed, &|o| warp.src_at(out + o), l, r);
        self.next = Some(out + n as f64);
    }
}

/// Renders output frames from `from` on, as many as `l` holds, of `feed`
/// warped by `warp` and transposed by `semitones`. Offline: allocates,
/// and takes a moment for a long stretch.
pub fn render(
    feed: &mut dyn Feed,
    warp: &Warp,
    semitones: f64,
    from: i64,
    l: &mut [f32],
    r: &mut [f32],
) {
    const BLOCK: usize = 256;
    let mut w = Warper::new(BLOCK);
    w.set_pitch(semitones);
    let n = l.len().min(r.len());
    let mut done = 0;
    while done < n {
        let run = BLOCK.min(n - done);
        let at = from + done as i64;
        w.process(
            feed,
            warp,
            at as f64,
            &mut l[done..done + run],
            &mut r[done..done + run],
        );
        done += run;
    }
}

/// Stereo audio in memory, read anywhere: silence outside it.
pub struct SliceFeed<'a> {
    pub left: &'a [f32],
    pub right: &'a [f32],
}

impl Feed for SliceFeed<'_> {
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]) {
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let t = start + i as i64;
            let at = (t >= 0).then_some(t as usize);
            *a = at.and_then(|t| self.left.get(t)).copied().unwrap_or(0.0);
            *b = at.and_then(|t| self.right.get(t)).copied().unwrap_or(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(src: i64, out: i64) -> WarpPoint {
        WarpPoint { src, out }
    }

    #[test]
    fn a_warp_maps_both_ways_and_carries_on_past_its_ends() {
        let w = Warp::new(vec![pt(0, 0), pt(1000, 2000), pt(2000, 2500)]).unwrap();
        assert_eq!(w.src_at(1000.0), 500.0);
        assert_eq!(w.src_at(2250.0), 1500.0);
        assert_eq!(w.out_at(500.0), 1000.0);
        assert_eq!(w.out_at(1500.0), 2250.0);
        assert_eq!(w.speed_at(100.0), 0.5);
        assert_eq!(w.speed_at(2000.0), 2.0);
        // Past the ends, the end stretches continue.
        assert_eq!(w.src_at(-100.0), -50.0);
        assert_eq!(w.src_at(3000.0), 3000.0);
        assert_eq!(w.out_range(), 0..2500);
        assert_eq!(w.src_range(), 0..2000);
    }

    #[test]
    fn bad_warps_are_refused() {
        assert_eq!(Warp::new(vec![pt(0, 0)]).unwrap_err(), WarpError::TooFew);
        assert_eq!(
            Warp::new(vec![pt(0, 0), pt(10, 0)]).unwrap_err(),
            WarpError::OutOfOrder
        );
        assert_eq!(
            Warp::new(vec![pt(0, 0), pt(0, 10)]).unwrap_err(),
            WarpError::OutOfOrder
        );
        assert_eq!(
            Warp::new(vec![pt(0, 0), pt(1000, 10)]).unwrap_err(),
            WarpError::TooSteep
        );
        let even = Warp::even(100..1100, 50, 0.5).unwrap();
        assert_eq!(even.points(), &[pt(100, 50), pt(1100, 2050)]);
    }
}
