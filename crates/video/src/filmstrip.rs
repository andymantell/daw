// SPDX-License-Identifier: GPL-3.0-or-later
//! Which frames a video clip shows along the timeline (roadmap 3.10's
//! filmstrip): pictures laid end to end, each the frame at its left
//! edge. Zoomed out, frames are skipped so the pictures don't squash;
//! zoomed in until a frame is wider than a picture, every frame gets a
//! tile of its own, as long as the frame lasts, so cuts show where they
//! happen.

use crate::Info;

/// A video clip on the timeline, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filmstrip {
    pub info: Info,
    /// Where the clip starts on the timeline.
    pub start: f64,
    /// How far into the video it starts.
    pub offset: f64,
    /// How long it plays.
    pub len: f64,
}

/// One picture of the strip: frame `frame` of the video, drawn from
/// `from` to `to` on the timeline, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    pub frame: u64,
    pub from: f64,
    pub to: f64,
}

impl Filmstrip {
    /// Where frame `n` starts on the timeline.
    pub fn frame_start(&self, n: u64) -> f64 {
        self.start + self.info.frame_time(n) - self.offset
    }

    /// The frame showing at timeline time `t`, if the clip is playing
    /// then.
    pub fn frame_at(&self, t: f64) -> Option<u64> {
        (t >= self.start && t < self.start + self.len)
            .then(|| self.info.frame_at(self.offset + t - self.start))
    }

    /// How many frames apart neighbouring pictures are at `pps` points
    /// per second, for pictures `tile` points wide: 1 once a frame is
    /// as wide as a picture.
    pub fn step(&self, pps: f64, tile: f64) -> u64 {
        let frame_px = pps / self.info.fps().max(1e-6);
        (tile / frame_px.max(1e-9)).ceil().max(1.0) as u64
    }

    /// The pictures to draw between timeline times `from` and `to`.
    /// Pictures are on frames that are multiples of the step, so they
    /// stay put as the view scrolls.
    pub fn tiles(&self, from: f64, to: f64, pps: f64, tile: f64) -> Vec<Tile> {
        let (lo, hi) = (from.max(self.start), to.min(self.start + self.len));
        if hi <= lo || self.len <= 0.0 {
            return Vec::new();
        }
        let step = self.step(pps, tile);
        let first = self.info.frame_at(self.offset + lo - self.start);
        let mut n = first / step * step;
        let last = self.info.frames() - 1;
        let end = self.start + self.len;
        let mut out = Vec::new();
        while n <= last {
            let a = self.frame_start(n).max(self.start);
            if a >= hi {
                break;
            }
            let b = self.frame_start(n + step).min(end);
            if b > lo {
                out.push(Tile {
                    frame: n,
                    from: a,
                    to: b,
                });
            }
            n += step;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Filmstrip {
        Filmstrip {
            info: Info {
                width: 160,
                height: 90,
                rate: (25, 1),
                seconds: 60.0,
            },
            start: 10.0,
            offset: 2.0,
            len: 20.0,
        }
    }

    #[test]
    fn zoomed_in_every_frame_has_its_own_tile() {
        let s = strip();
        // 25 frames a second at 2500 points a second: 100 points each.
        let t = s.tiles(10.0, 10.2, 2500.0, 80.0);
        assert_eq!(t.len(), 5);
        assert_eq!(t[0].frame, 50, "two seconds in");
        assert!(t.windows(2).all(|w| w[1].frame == w[0].frame + 1));
        assert!(
            (t[0].to - t[0].from - 0.04).abs() < 1e-9,
            "as long as the frame"
        );
        assert_eq!(s.frame_at(10.0), Some(50));
        assert_eq!(s.frame_at(9.99), None);
        assert_eq!(s.frame_at(30.0), None, "the clip has ended");
    }

    #[test]
    fn zoomed_out_frames_are_skipped_and_stay_put() {
        let s = strip();
        // 40 points a second: a frame is 1.6 points, so 50 frames a tile.
        assert_eq!(s.step(40.0, 80.0), 50);
        let t = s.tiles(0.0, 100.0, 40.0, 80.0);
        assert!(t.iter().all(|t| t.frame % 50 == 0));
        assert_eq!(t[0].frame, 50);
        assert_eq!(t[0].from, 10.0);
        assert_eq!(t.last().unwrap().to, 30.0, "held to the clip's end");
        // Scrolled a little, the same frames are where they were.
        let moved = s.tiles(13.1, 100.0, 40.0, 80.0);
        let same = t.iter().find(|x| x.frame == moved[0].frame).unwrap();
        assert_eq!(same.from, moved[0].from);
        // Zooming in shows more frames.
        assert!(s.tiles(0.0, 100.0, 400.0, 80.0).len() > t.len());
    }

    #[test]
    fn nothing_outside_the_clip() {
        let s = strip();
        assert!(s.tiles(0.0, 9.0, 40.0, 80.0).is_empty());
        assert!(s.tiles(31.0, 40.0, 40.0, 80.0).is_empty());
    }
}
