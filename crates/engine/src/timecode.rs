// SPDX-License-Identifier: GPL-3.0-or-later
//! Timecode (roadmap 3.10): positions as hours:minutes:seconds:frames of
//! picture, beside bars and beats.
//!
//! Timecode is a way of showing sample time, not a third clock (decision
//! 0005): the picture frame at a position is the one whose span contains
//! that sample, worked out exactly from the frame rate as a fraction.
//!
//! Frame numbers are counted two ways. A frame's *index* is how many
//! picture frames it is after another. Its *label* is the number written
//! as timecode: the same as the index, except with drop-frame counting,
//! where the labels 0 and 1 are skipped at the start of every minute
//! except every tenth, so the count keeps up with the clock at 29.97
//! frames a second. No picture is dropped, only labels.

use std::fmt;

/// Frame rates of picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rate {
    /// 24 ÷ 1.001.
    Fps23_976,
    Fps24,
    #[default]
    Fps25,
    /// 30 ÷ 1.001, counted without dropping labels.
    Fps29_97,
    /// 30 ÷ 1.001, drop-frame.
    Fps29_97Drop,
    Fps30,
    /// 30, drop-frame.
    Fps30Drop,
    Fps50,
    Fps60,
}

impl Rate {
    pub const ALL: [Rate; 9] = [
        Rate::Fps23_976,
        Rate::Fps24,
        Rate::Fps25,
        Rate::Fps29_97,
        Rate::Fps29_97Drop,
        Rate::Fps30,
        Rate::Fps30Drop,
        Rate::Fps50,
        Rate::Fps60,
    ];

    /// As the project stores it. Never change one.
    pub fn id(self) -> &'static str {
        match self {
            Rate::Fps23_976 => "23.976",
            Rate::Fps24 => "24",
            Rate::Fps25 => "25",
            Rate::Fps29_97 => "29.97",
            Rate::Fps29_97Drop => "29.97df",
            Rate::Fps30 => "30",
            Rate::Fps30Drop => "30df",
            Rate::Fps50 => "50",
            Rate::Fps60 => "60",
        }
    }

    pub fn from_id(id: &str) -> Option<Rate> {
        Rate::ALL.into_iter().find(|r| r.id() == id)
    }

    /// For people.
    pub fn label(self) -> &'static str {
        match self {
            Rate::Fps23_976 => "23.976",
            Rate::Fps24 => "24",
            Rate::Fps25 => "25",
            Rate::Fps29_97 => "29.97",
            Rate::Fps29_97Drop => "29.97 drop-frame",
            Rate::Fps30 => "30",
            Rate::Fps30Drop => "30 drop-frame",
            Rate::Fps50 => "50",
            Rate::Fps60 => "60",
        }
    }

    /// Frames per second, as the exact fraction `(num, den)`.
    pub fn fps(self) -> (i64, i64) {
        match self {
            Rate::Fps23_976 => (24_000, 1001),
            Rate::Fps29_97 | Rate::Fps29_97Drop => (30_000, 1001),
            r => (r.nominal(), 1),
        }
    }

    pub fn as_f64(self) -> f64 {
        let (n, d) = self.fps();
        n as f64 / d as f64
    }

    /// Frame labels per second of timecode: 30 at 29.97.
    pub fn nominal(self) -> i64 {
        match self {
            Rate::Fps23_976 | Rate::Fps24 => 24,
            Rate::Fps25 => 25,
            Rate::Fps29_97 | Rate::Fps29_97Drop | Rate::Fps30 | Rate::Fps30Drop => 30,
            Rate::Fps50 => 50,
            Rate::Fps60 => 60,
        }
    }

    pub fn drop_frame(self) -> bool {
        matches!(self, Rate::Fps29_97Drop | Rate::Fps30Drop)
    }

    /// Labels skipped at the start of a minute.
    fn dropped(self) -> i64 {
        if self.drop_frame() { 2 } else { 0 }
    }

    /// Frames in 24 hours of timecode, after which it starts again from
    /// zero.
    pub fn frames_per_day(self) -> i64 {
        let ten_minutes = self.nominal() * 600 - 9 * self.dropped();
        ten_minutes * 6 * 24
    }

    /// The index of the frame containing sample `at`, counted from the
    /// frame starting at sample 0 (negative before it).
    pub fn frame_at(self, at: i64, sample_rate: u32) -> i64 {
        let (n, d) = self.fps();
        (at as i128 * n as i128).div_euclid(sample_rate as i128 * d as i128) as i64
    }

    /// The first sample in frame `index`: the one
    /// [`frame_at`](Rate::frame_at) puts in it.
    pub fn frame_start(self, index: i64, sample_rate: u32) -> i64 {
        let (n, d) = self.fps();
        let x = index as i128 * sample_rate as i128 * d as i128;
        // Rounded up.
        -((-x).div_euclid(n as i128)) as i64
    }

    /// The label of the frame `index` frames after label 0.
    pub fn label_of(self, index: i64) -> i64 {
        let drop = self.dropped();
        if drop == 0 {
            return index;
        }
        let ten_minutes = self.nominal() * 600 - 9 * drop;
        let minute = self.nominal() * 60 - drop;
        let (tens, rest) = (index.div_euclid(ten_minutes), index.rem_euclid(ten_minutes));
        // The first minute of ten keeps all its labels; each after it
        // skips `drop` at its start.
        let skipped = if rest < drop {
            0
        } else {
            (rest - drop) / minute
        };
        index + drop * (9 * tens + skipped)
    }

    /// The frame index of a label, or `None` for a label drop-frame
    /// counting skips.
    pub fn index_of(self, label: i64) -> Option<i64> {
        let drop = self.dropped();
        if drop == 0 {
            return Some(label);
        }
        let per_minute = self.nominal() * 60;
        let minutes = label.div_euclid(per_minute);
        let in_minute = label.rem_euclid(per_minute);
        if in_minute < drop && minutes % 10 != 0 {
            return None;
        }
        Some(label - drop * (minutes - minutes.div_euclid(10)))
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A timecode label: hours, minutes, seconds and frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timecode {
    pub hours: i64,
    pub minutes: i64,
    pub seconds: i64,
    pub frames: i64,
    /// Written with a semicolon before the frames, as drop-frame is.
    pub drop_frame: bool,
}

impl Timecode {
    /// The timecode for a label, wrapping every 24 hours.
    pub fn from_label(rate: Rate, label: i64) -> Timecode {
        let day = rate.nominal() * 86_400;
        let l = label.rem_euclid(day);
        let fps = rate.nominal();
        Timecode {
            hours: l / (fps * 3600),
            minutes: l / (fps * 60) % 60,
            seconds: l / fps % 60,
            frames: l % fps,
            drop_frame: rate.drop_frame(),
        }
    }

    pub fn label(self, rate: Rate) -> i64 {
        ((self.hours * 60 + self.minutes) * 60 + self.seconds) * rate.nominal() + self.frames
    }

    /// Reads "01:00:00:00", with any of `:` `;` `.` between the parts.
    /// Fewer parts fill from the right ("10:00" is ten seconds), so
    /// "1:00:00:00" and "100:00:00" don't mean the same. `None` if a part
    /// is out of range or the label is one drop-frame skips.
    pub fn parse(rate: Rate, text: &str) -> Option<Timecode> {
        let parts: Vec<&str> = text.trim().split([':', ';', '.']).collect();
        if parts.is_empty() || parts.len() > 4 {
            return None;
        }
        let mut v = [0i64; 4];
        let skip = 4 - parts.len();
        for (i, p) in parts.iter().enumerate() {
            let p = p.trim();
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || p.len() > 3 {
                return None;
            }
            v[skip + i] = p.parse().ok()?;
        }
        let [hours, minutes, seconds, frames] = v;
        if hours > 23 || minutes > 59 || seconds > 59 || frames >= rate.nominal() {
            return None;
        }
        let tc = Timecode {
            hours,
            minutes,
            seconds,
            frames,
            drop_frame: rate.drop_frame(),
        };
        rate.index_of(tc.label(rate))?;
        Some(tc)
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{sep}{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

/// How the project's time lines up with the picture's timecode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    pub rate: Rate,
    /// The label at `offset`: the picture's start timecode, such as
    /// 01:00:00:00.
    pub start: i64,
    /// Where on the timeline, in samples, the start timecode falls.
    pub offset: i64,
}

impl Clock {
    /// The frame at sample `at`, as a frame index from the start
    /// timecode's frame.
    pub fn frame_at(&self, at: i64, sample_rate: u32) -> i64 {
        self.rate.frame_at(at - self.offset, sample_rate)
    }

    /// The first sample of frame `index` from the start timecode's frame.
    pub fn frame_start(&self, index: i64, sample_rate: u32) -> i64 {
        self.offset + self.rate.frame_start(index, sample_rate)
    }

    /// The start of the frame containing `at`.
    pub fn snap(&self, at: i64, sample_rate: u32) -> i64 {
        self.frame_start(self.frame_at(at, sample_rate), sample_rate)
    }

    /// The timecode at sample `at`.
    pub fn timecode(&self, at: i64, sample_rate: u32) -> Timecode {
        let start = self.rate.index_of(self.start).unwrap_or(0);
        let index = start + self.frame_at(at, sample_rate);
        let index = index.rem_euclid(self.rate.frames_per_day());
        Timecode::from_label(self.rate, self.rate.label_of(index))
    }

    /// Where timecode `tc` falls, in samples: the first sample of its
    /// frame. Labels before the start timecode are taken as the next
    /// day's only if that is nearer, so a start of 23:59:00:00 and a
    /// timecode of 00:00:10:00 are 70 seconds apart.
    pub fn position(&self, tc: Timecode, sample_rate: u32) -> Option<i64> {
        let to = self.rate.index_of(tc.label(self.rate))?;
        let from = self.rate.index_of(self.start).unwrap_or(0);
        let day = self.rate.frames_per_day();
        let mut d = (to - from).rem_euclid(day);
        if d > day / 2 {
            d -= day;
        }
        Some(self.frame_start(d, sample_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(rate: Rate, index: i64) -> String {
        Timecode::from_label(rate, rate.label_of(index)).to_string()
    }

    #[test]
    fn frames_hold_the_samples_inside_them() {
        for rate in Rate::ALL {
            for sr in [44_100, 48_000, 96_000] {
                for i in [-3, 0, 1, 2, 999, 86_400 * 30] {
                    let s = rate.frame_start(i, sr);
                    assert_eq!(rate.frame_at(s, sr), i, "{rate} {sr} {i}");
                    assert_eq!(rate.frame_at(s - 1, sr), i - 1, "{rate} {sr} {i}");
                }
            }
        }
        // 2002 samples a frame at 23.976 and 48 kHz, exactly.
        assert_eq!(Rate::Fps23_976.frame_start(1, 48_000), 2002);
        assert_eq!(Rate::Fps23_976.frame_at(2001, 48_000), 0);
        // An hour of 25 fps is 90,000 frames.
        assert_eq!(Rate::Fps25.frame_at(3600 * 48_000, 48_000), 90_000);
    }

    #[test]
    fn drop_frame_skips_two_labels_a_minute_but_every_tenth() {
        let r = Rate::Fps29_97Drop;
        assert_eq!(tc(r, 0), "00:00:00;00");
        assert_eq!(tc(r, 1799), "00:00:59;29");
        // The next frame is labelled ;02: ;00 and ;01 are skipped.
        assert_eq!(tc(r, 1800), "00:01:00;02");
        assert_eq!(tc(r, 3597), "00:01:59;29");
        assert_eq!(tc(r, 3598), "00:02:00;02");
        // Ten minutes is 17,982 frames, and minute ten keeps its ;00.
        assert_eq!(tc(r, 17_981), "00:09:59;29");
        assert_eq!(tc(r, 17_982), "00:10:00;00");
        assert_eq!(tc(r, 17_983), "00:10:00;01");
        assert_eq!(tc(r, 17_982 + 1800), "00:11:00;02");
        // An hour of labels is 107,892 frames, which at 29.97 a second
        // is 3600 s less 3.6 ms: the count keeps up with the clock.
        assert_eq!(tc(r, 107_892), "01:00:00;00");
        let secs = 107_892.0 / r.as_f64();
        assert!((secs - 3600.0).abs() < 0.004, "{secs}");
        // Without drop-frame the labels run behind the clock.
        assert_eq!(tc(Rate::Fps29_97, 107_892), "00:59:56:12");
        // Every index maps back from its label; skipped labels don't.
        for i in (0..300_000).step_by(7) {
            assert_eq!(r.index_of(r.label_of(i)), Some(i));
        }
        assert_eq!(r.index_of(1800), None);
        assert_eq!(r.index_of(1801), None);
        assert_eq!(r.index_of(18_000), Some(17_982));
        // 30 fps drop-frame counts the same way.
        assert_eq!(tc(Rate::Fps30Drop, 1800), "00:01:00;02");
    }

    #[test]
    fn typed_timecode_is_read_and_checked() {
        let r = Rate::Fps25;
        let t = Timecode::parse(r, "01:00:00:00").unwrap();
        assert_eq!(t.label(r), 90_000);
        assert_eq!(
            Timecode::parse(r, " 1:02:03.04 ").unwrap().to_string(),
            "01:02:03:04"
        );
        assert_eq!(
            Timecode::parse(r, "10:05").unwrap().to_string(),
            "00:00:10:05"
        );
        assert!(Timecode::parse(r, "00:00:00:25").is_none());
        assert!(Timecode::parse(r, "00:60:00:00").is_none());
        assert!(Timecode::parse(r, "24:00:00:00").is_none());
        assert!(Timecode::parse(r, "1:2:x:4").is_none());
        assert!(Timecode::parse(r, "").is_none());
        let d = Rate::Fps29_97Drop;
        assert!(Timecode::parse(d, "00:01:00;00").is_none());
        assert!(Timecode::parse(d, "00:01:00;02").is_some());
        assert!(Timecode::parse(d, "00:10:00;00").is_some());
        assert_eq!(
            Timecode::parse(d, "00:10:00;00").unwrap().to_string(),
            "00:10:00;00"
        );
    }

    #[test]
    fn the_start_timecode_and_offset_line_up_the_picture() {
        let sr = 48_000;
        let c = Clock {
            rate: Rate::Fps25,
            start: 90_000,
            offset: 2 * sr as i64,
        };
        // Two seconds in is 01:00:00:00, and a second before is the
        // hour before.
        assert_eq!(c.timecode(2 * 48_000, sr).to_string(), "01:00:00:00");
        assert_eq!(c.timecode(2 * 48_000 + 1919, sr).to_string(), "01:00:00:00");
        assert_eq!(c.timecode(2 * 48_000 + 1920, sr).to_string(), "01:00:00:01");
        assert_eq!(c.timecode(48_000, sr).to_string(), "00:59:59:00");
        let t = Timecode::parse(Rate::Fps25, "01:00:10:00").unwrap();
        assert_eq!(c.position(t, sr), Some(12 * 48_000));
        assert_eq!(c.snap(12 * 48_000 + 1000, sr), 12 * 48_000);
        // Timecode wraps at midnight.
        let late = Clock {
            rate: Rate::Fps25,
            start: Timecode::parse(Rate::Fps25, "23:59:59:00")
                .unwrap()
                .label(Rate::Fps25),
            offset: 0,
        };
        assert_eq!(late.timecode(2 * 48_000, sr).to_string(), "00:00:01:00");
        let t = Timecode::parse(Rate::Fps25, "00:00:01:00").unwrap();
        assert_eq!(late.position(t, sr), Some(2 * 48_000));
        // Drop-frame from a start of 01:00:00;00.
        let d = Clock {
            rate: Rate::Fps29_97Drop,
            start: Timecode::parse(Rate::Fps29_97Drop, "01:00:00;00")
                .unwrap()
                .label(Rate::Fps29_97Drop),
            offset: 0,
        };
        let one_minute = Rate::Fps29_97Drop.frame_start(1800, sr);
        assert_eq!(d.timecode(one_minute, sr).to_string(), "01:01:00;02");
        let t = Timecode::parse(Rate::Fps29_97Drop, "01:01:00;02").unwrap();
        assert_eq!(d.position(t, sr), Some(one_minute));
    }

    #[test]
    fn rates_are_stored_by_id() {
        for r in Rate::ALL {
            assert_eq!(Rate::from_id(r.id()), Some(r));
        }
        assert_eq!(Rate::from_id("nope"), None);
    }
}
