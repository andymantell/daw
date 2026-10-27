// SPDX-License-Identifier: GPL-3.0-or-later
//! Musical and absolute time (decision 0005).
//!
//! Musical positions are integer [`Ticks`] at [`PPQ`] per quarter note.
//! Absolute positions are integer [`Frames`] at the project sample rate.
//! The [`TempoMap`] converts between them exactly: tempos are rational
//! numbers, and all arithmetic is done in 128-bit integers, so converting
//! never drifts however long the project is. A position that falls between
//! two frames (or two ticks) rounds down, to the earlier one.
//!
//! Phase 1 needs one tempo and one time signature, but the map already
//! takes step changes of either, at tick positions. Tempo ramps are not
//! supported yet.

use std::fmt;

/// Ticks per quarter note.
pub const PPQ: i64 = 960;

/// A musical position or length, in ticks at [`PPQ`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticks(pub i64);

/// An absolute position or length, in sample frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Frames(pub i64);

/// Beats (quarter notes) per minute, as the exact fraction `num / den`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tempo {
    num: u64,
    den: u64,
}

impl Tempo {
    /// A whole-number tempo, e.g. `Tempo::bpm(120)`.
    pub fn bpm(bpm: u64) -> Tempo {
        Tempo::ratio(bpm, 1)
    }

    /// A fractional tempo, e.g. `Tempo::ratio(24001, 200)` for 120.005.
    ///
    /// # Panics
    /// If either part is zero.
    pub fn ratio(num: u64, den: u64) -> Tempo {
        assert!(num > 0 && den > 0, "tempo must be positive");
        let g = gcd(num, den);
        Tempo {
            num: num / g,
            den: den / g,
        }
    }

    /// The nearest tempo with a denominator of 1000 (three decimal places),
    /// for tempos typed or detected as decimals.
    pub fn from_f64(bpm: f64) -> Tempo {
        Tempo::ratio((bpm * 1000.0).round().max(1.0) as u64, 1000)
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

/// Beats per bar and the note value of a beat, e.g. 6/8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSignature {
    pub beats: u32,
    /// 1, 2, 4, 8, 16 or 32.
    pub unit: u32,
}

impl TimeSignature {
    pub const COMMON: TimeSignature = TimeSignature { beats: 4, unit: 4 };

    /// # Panics
    /// If `beats` is zero or `unit` is not a power of two from 1 to 32.
    pub fn new(beats: u32, unit: u32) -> TimeSignature {
        assert!(beats > 0, "a bar needs at least one beat");
        assert!(
            unit.is_power_of_two() && unit <= 32,
            "beat unit must be 1, 2, 4, 8, 16 or 32"
        );
        TimeSignature { beats, unit }
    }

    /// Length of one beat in ticks.
    pub fn beat_ticks(self) -> i64 {
        PPQ * 4 / self.unit as i64
    }

    /// Length of one bar in ticks.
    pub fn bar_ticks(self) -> i64 {
        self.beat_ticks() * self.beats as i64
    }
}

/// A position as bar, beat and tick, all counted from 1 except the tick,
/// as shown to users ("12.3.240").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarBeat {
    pub bar: i64,
    pub beat: i64,
    pub tick: i64,
}

impl fmt::Display for BarBeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{:03}", self.bar, self.beat, self.tick)
    }
}

#[derive(Clone, Copy, Debug)]
struct TempoPoint {
    at: Ticks,
    /// Where `at` falls in frames, worked out when the map is built.
    frame: Frames,
    tempo: Tempo,
}

#[derive(Clone, Copy, Debug)]
struct MeterPoint {
    at: Ticks,
    /// The bar number (from 0) that starts at `at`.
    bar: i64,
    sig: TimeSignature,
}

/// Tempo and time-signature changes, and conversions between ticks,
/// frames and bars.
#[derive(Clone, Debug)]
pub struct TempoMap {
    rate: u32,
    tempos: Vec<TempoPoint>,
    meters: Vec<MeterPoint>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MapError {
    /// A change must come after the start, and after the change before it.
    OutOfOrder,
    /// A time-signature change must fall on a bar line.
    NotOnBarLine,
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MapError::OutOfOrder => "changes must be in order, after the start",
            MapError::NotOnBarLine => "a time signature can only change on a bar line",
        })
    }
}

impl std::error::Error for MapError {}

impl TempoMap {
    /// One tempo and time signature from the start.
    ///
    /// # Panics
    /// If `rate` is zero.
    pub fn new(rate: u32, tempo: Tempo, sig: TimeSignature) -> TempoMap {
        assert!(rate > 0, "sample rate must be positive");
        TempoMap {
            rate,
            tempos: vec![TempoPoint {
                at: Ticks(0),
                frame: Frames(0),
                tempo,
            }],
            meters: vec![MeterPoint {
                at: Ticks(0),
                bar: 0,
                sig,
            }],
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Changes the tempo from `at` onwards. Changes must be added in order.
    pub fn add_tempo(&mut self, at: Ticks, tempo: Tempo) -> Result<(), MapError> {
        let last = *self.tempos.last().unwrap();
        if at <= last.at {
            return Err(MapError::OutOfOrder);
        }
        let frame = Frames(last.frame.0 + span_frames(at.0 - last.at.0, last.tempo, self.rate));
        self.tempos.push(TempoPoint { at, frame, tempo });
        Ok(())
    }

    /// Changes the time signature from the bar line at `at` onwards.
    pub fn add_meter(&mut self, at: Ticks, sig: TimeSignature) -> Result<(), MapError> {
        let last = *self.meters.last().unwrap();
        if at <= last.at {
            return Err(MapError::OutOfOrder);
        }
        let since = at.0 - last.at.0;
        if since % last.sig.bar_ticks() != 0 {
            return Err(MapError::NotOnBarLine);
        }
        self.meters.push(MeterPoint {
            at,
            bar: last.bar + since / last.sig.bar_ticks(),
            sig,
        });
        Ok(())
    }

    /// The tempo in force at `at`.
    pub fn tempo_at(&self, at: Ticks) -> Tempo {
        self.tempo_point(at).tempo
    }

    /// The time signature in force at `at`.
    pub fn signature_at(&self, at: Ticks) -> TimeSignature {
        self.meter_point(at).sig
    }

    fn tempo_point(&self, at: Ticks) -> &TempoPoint {
        let i = self.tempos.partition_point(|p| p.at <= at);
        &self.tempos[i.saturating_sub(1)]
    }

    fn meter_point(&self, at: Ticks) -> &MeterPoint {
        let i = self.meters.partition_point(|p| p.at <= at);
        &self.meters[i.saturating_sub(1)]
    }

    /// The frame at a musical position, rounded down. Positions before the
    /// start use the first tempo.
    pub fn frames_at(&self, at: Ticks) -> Frames {
        let p = self.tempo_point(at);
        Frames(p.frame.0 + span_frames(at.0 - p.at.0, p.tempo, self.rate))
    }

    /// The musical position at a frame, rounded down.
    pub fn ticks_at(&self, at: Frames) -> Ticks {
        let i = self.tempos.partition_point(|p| p.frame <= at);
        let p = &self.tempos[i.saturating_sub(1)];
        Ticks(p.at.0 + span_ticks(at.0 - p.frame.0, p.tempo, self.rate))
    }

    /// Bar, beat and tick at a musical position (from 0 upwards).
    pub fn bar_beat(&self, at: Ticks) -> BarBeat {
        let m = self.meter_point(at);
        let since = at.0 - m.at.0;
        let bar = m.bar + since.div_euclid(m.sig.bar_ticks());
        let in_bar = since.rem_euclid(m.sig.bar_ticks());
        BarBeat {
            bar: bar + 1,
            beat: in_bar / m.sig.beat_ticks() + 1,
            tick: in_bar % m.sig.beat_ticks(),
        }
    }

    /// The musical position of a bar line (bars counted from 1).
    pub fn bar_start(&self, bar: i64) -> Ticks {
        let bar0 = bar - 1;
        let i = self.meters.partition_point(|p| p.bar <= bar0);
        let m = &self.meters[i.saturating_sub(1)];
        Ticks(m.at.0 + (bar0 - m.bar) * m.sig.bar_ticks())
    }

    /// Seconds at a frame, for display only.
    pub fn seconds(&self, at: Frames) -> f64 {
        at.0 as f64 / self.rate as f64
    }
}

/// Frames in `ticks` at a fixed tempo:
/// ticks × 60 × rate × den ÷ (PPQ × num), rounded down.
fn span_frames(ticks: i64, t: Tempo, rate: u32) -> i64 {
    let n = ticks as i128 * 60 * rate as i128 * t.den as i128;
    let d = PPQ as i128 * t.num as i128;
    n.div_euclid(d) as i64
}

/// Ticks in `frames` at a fixed tempo, rounded down.
fn span_ticks(frames: i64, t: Tempo, rate: u32) -> i64 {
    let n = frames as i128 * PPQ as i128 * t.num as i128;
    let d = 60 * rate as i128 * t.den as i128;
    n.div_euclid(d) as i64
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map120() -> TempoMap {
        TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON)
    }

    #[test]
    fn one_beat_at_120_is_half_a_second() {
        let m = map120();
        assert_eq!(m.frames_at(Ticks(PPQ)), Frames(24_000));
        assert_eq!(m.ticks_at(Frames(24_000)), Ticks(PPQ));
        assert_eq!(m.frames_at(Ticks(-PPQ)), Frames(-24_000));
    }

    #[test]
    fn no_drift_over_ten_hours() {
        // 120.005 BPM at 44.1 kHz: neither unit divides the other.
        let m = TempoMap::new(44_100, Tempo::ratio(24_001, 200), TimeSignature::COMMON);
        let beats: i64 = 120 * 60 * 10;
        let f = m.frames_at(Ticks(beats * PPQ));
        // Exact: 72,000 beats × 60 ÷ 120.005 × 44,100 = 1,587,533,852.756...
        assert_eq!(f, Frames(1_587_533_852));
        // Summing one beat at a time agrees with converting in one go.
        let summed: i64 = (0..beats)
            .map(|b| m.frames_at(Ticks((b + 1) * PPQ)).0 - m.frames_at(Ticks(b * PPQ)).0)
            .sum();
        assert_eq!(summed, f.0);
    }

    #[test]
    fn round_trips_never_move_forward() {
        let m = TempoMap::new(44_100, Tempo::from_f64(97.3), TimeSignature::COMMON);
        for t in (0..2_000_000).step_by(997) {
            let back = m.ticks_at(m.frames_at(Ticks(t)));
            assert!(back <= Ticks(t) && Ticks(t).0 - back.0 <= 1, "{t}");
        }
        for f in (0..5_000_000).step_by(1009) {
            let back = m.frames_at(m.ticks_at(Frames(f)));
            // A tick is about 28 frames here, so going via ticks can land
            // up to one tick earlier.
            assert!(back <= Frames(f) && f - back.0 < 30, "{f}");
        }
    }

    #[test]
    fn tempo_change_takes_effect_at_its_tick() {
        let mut m = map120();
        let bar3 = m.bar_start(3);
        m.add_tempo(bar3, Tempo::bpm(60)).unwrap();
        // Two bars at 120 = 4 s; then each beat takes 1 s.
        assert_eq!(m.frames_at(bar3), Frames(4 * 48_000));
        assert_eq!(m.frames_at(Ticks(bar3.0 + PPQ)), Frames(5 * 48_000));
        assert_eq!(m.ticks_at(Frames(5 * 48_000)), Ticks(bar3.0 + PPQ));
        assert_eq!(m.tempo_at(Ticks(0)), Tempo::bpm(120));
        assert_eq!(m.tempo_at(bar3), Tempo::bpm(60));
        assert_eq!(
            m.add_tempo(Ticks(PPQ), Tempo::bpm(90)),
            Err(MapError::OutOfOrder)
        );
    }

    #[test]
    fn bars_and_beats_follow_meter_changes() {
        let mut m = map120();
        assert_eq!(m.bar_beat(Ticks(0)).to_string(), "1.1.000");
        assert_eq!(m.bar_beat(Ticks(PPQ * 5 + 240)).to_string(), "2.2.240");
        // 6/8 from bar 3: beats are eighth notes, six to a bar.
        let bar3 = m.bar_start(3);
        m.add_meter(bar3, TimeSignature::new(6, 8)).unwrap();
        assert_eq!(m.bar_beat(bar3).to_string(), "3.1.000");
        assert_eq!(m.bar_beat(Ticks(bar3.0 + 480 * 7)).to_string(), "4.2.000");
        assert_eq!(m.bar_start(4), Ticks(bar3.0 + 480 * 6));
        assert_eq!(m.signature_at(m.bar_start(5)), TimeSignature::new(6, 8));
        assert_eq!(
            m.add_meter(Ticks(m.bar_start(6).0 + 1), TimeSignature::COMMON),
            Err(MapError::NotOnBarLine)
        );
    }

    #[test]
    fn tempos_are_kept_exact() {
        assert_eq!(Tempo::ratio(240, 2), Tempo::bpm(120));
        assert_eq!(Tempo::from_f64(97.3), Tempo::ratio(973, 10));
        assert!((Tempo::from_f64(97.3).as_f64() - 97.3).abs() < 1e-9);
    }
}
