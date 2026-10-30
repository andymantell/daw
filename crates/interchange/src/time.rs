// SPDX-License-Identifier: GPL-3.0-or-later
//! Converting between the format's decimal times and ours.
//!
//! The format counts time as decimal beats (quarter notes) or decimal
//! seconds. We count whole ticks ([`PPQ`] to a quarter note) for notes
//! and whole frames at the project's rate for audio, automation and
//! comments. The rules, both ways:
//!
//! - Ticks to beats is exact (`ticks / PPQ`); beats to ticks rounds to
//!   the nearest tick.
//! - Frames to seconds is exact enough (`frames / rate`); seconds to
//!   frames rounds to the nearest frame.
//! - Frames to beats goes through the tempo map, and lands between two
//!   ticks: the fraction is kept, worked out from the frames of the
//!   ticks either side. Beats to frames is the reverse, rounded to the
//!   nearest frame, so a frame written out comes back as the same frame.
//! - Numbers are written in the shortest form that reads back as the
//!   same `f64`.

use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};
use daw_project::{PPQ, Project, TempoChange};

/// The project's tempo map: its starting tempo and time signature and
/// every change after, as the app plays them. Changes on the same beat
/// as one before them, or a tempo change on the very start, are left
/// out, as the app leaves them out.
pub fn tempo_map(p: &Project) -> TempoMap {
    let mut m = TempoMap::new(
        p.rate.max(1),
        tempo(p.tempo_bpm),
        signature(p.beats_per_bar, p.beat_unit),
    );
    let mut meters: Vec<_> = p.meter_changes.iter().collect();
    meters.sort_by_key(|s| s.bar);
    for s in meters {
        if s.bar > 1 {
            let at = m.bar_start(s.bar);
            let _ = m.add_meter(at, signature(s.beats, s.unit));
        }
    }
    let mut tempos: Vec<&TempoChange> = p.tempo_changes.iter().collect();
    tempos.sort_by_key(|t| (t.bar, t.beat));
    for t in tempos {
        let Some(at) = tempo_tick(&m, t) else {
            continue;
        };
        let _ = if t.glide {
            m.add_glide(at, tempo(t.bpm))
        } else {
            m.add_tempo(at, tempo(t.bpm))
        };
    }
    m
}

/// Where a tempo change falls, or `None` at the very start. A beat past
/// the end of its bar is taken as the bar's last.
pub fn tempo_tick(m: &TempoMap, t: &TempoChange) -> Option<Ticks> {
    let bar = m.bar_start(t.bar.max(1));
    let sig = m.signature_at(bar);
    let beat = (t.beat.max(1) - 1).min(sig.beats - 1) as i64;
    let at = Ticks(bar.0 + beat * sig.beat_ticks());
    (at.0 > 0).then_some(at)
}

pub fn signature(beats: u32, unit: u32) -> TimeSignature {
    let unit = if unit.is_power_of_two() && unit <= 32 {
        unit
    } else {
        4
    };
    TimeSignature::new(beats.clamp(1, 64), unit)
}

pub fn tempo(bpm: f64) -> Tempo {
    Tempo::from_f64(if bpm.is_finite() { bpm } else { 120.0 }.clamp(10.0, 999.0))
}

/// Beats in `ticks`, exactly.
pub fn ticks_to_beats(ticks: i64) -> f64 {
    ticks as f64 / PPQ as f64
}

/// The nearest tick to `beats`.
pub fn beats_to_ticks(beats: f64) -> i64 {
    (beats * PPQ as f64).round() as i64
}

/// Seconds in `frames` at `rate`.
pub fn frames_to_seconds(frames: i64, rate: u32) -> f64 {
    frames as f64 / rate.max(1) as f64
}

/// The nearest frame to `seconds` at `rate`.
pub fn seconds_to_frames(seconds: f64, rate: u32) -> i64 {
    (seconds * rate.max(1) as f64).round() as i64
}

/// The position of frame `at` in beats, through the tempo map, with the
/// part of a tick kept.
pub fn frames_to_beats(m: &TempoMap, at: i64) -> f64 {
    let t = m.ticks_at(Frames(at)).0;
    // The tick at or before the frame, and how far on to the next.
    let (t, f0, f1) = settle(m, t, at);
    let frac = if f1 > f0 {
        (at - f0) as f64 / (f1 - f0) as f64
    } else {
        0.0
    };
    (t as f64 + frac) / PPQ as f64
}

/// Finds the tick `t` whose frame is at or before `at` with the next
/// tick's after it, starting near `t`.
fn settle(m: &TempoMap, mut t: i64, at: i64) -> (i64, i64, i64) {
    loop {
        let f0 = m.frames_at(Ticks(t)).0;
        let f1 = m.frames_at(Ticks(t + 1)).0;
        if f0 > at {
            t -= 1;
        } else if f1 <= at && f1 > f0 {
            t += 1;
        } else {
            return (t, f0, f1);
        }
    }
}

/// The nearest frame to `beats`, through the tempo map.
pub fn beats_to_frames(m: &TempoMap, beats: f64) -> i64 {
    let t = beats * PPQ as f64;
    let t0 = t.floor();
    let frac = t - t0;
    let t0 = t0 as i64;
    let f0 = m.frames_at(Ticks(t0)).0 as f64;
    let f1 = m.frames_at(Ticks(t0 + 1)).0 as f64;
    (f0 + frac * (f1 - f0)).round() as i64
}

/// The tick at frame `at`, rounded to the nearest.
pub fn frames_to_ticks(m: &TempoMap, at: i64) -> i64 {
    (frames_to_beats(m, at) * PPQ as f64).round() as i64
}

/// The frame at tick `at`.
pub fn ticks_to_frames(m: &TempoMap, at: i64) -> i64 {
    m.frames_at(Ticks(at)).0
}

/// `v` written so it reads back as the same number: `inf` and `-inf`
/// as the format spells them.
pub fn num(v: f64) -> String {
    if v == f64::INFINITY {
        "inf".into()
    } else if v == f64::NEG_INFINITY {
        "-inf".into()
    } else if v.is_nan() {
        "0".into()
    } else if v == 0.0 {
        // No "-0".
        "0".into()
    } else {
        format!("{v}")
    }
}

/// A number as the format writes it, or `None`.
pub fn parse_num(s: &str) -> Option<f64> {
    match s.trim() {
        "inf" | "+inf" | "Infinity" => Some(f64::INFINITY),
        "-inf" | "-Infinity" => Some(f64::NEG_INFINITY),
        s => s.parse().ok(),
    }
}

/// dB from a linear gain; silence is −144 dB.
pub fn linear_to_db(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        -144.0
    } else {
        (20.0 * v.log10()).max(-144.0)
    }
}

/// A linear gain from dB; −144 dB and below is silence.
pub fn db_to_linear(db: f64) -> f64 {
    if db <= -144.0 {
        0.0
    } else {
        10f64.powf(db / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_project::MeterChange;

    fn song() -> Project {
        Project {
            rate: 44_100,
            tempo_bpm: 97.5,
            tempo_changes: vec![
                TempoChange {
                    id: "a".into(),
                    bar: 3,
                    beat: 2,
                    bpm: 140.0,
                    glide: false,
                },
                TempoChange {
                    id: "b".into(),
                    bar: 9,
                    beat: 1,
                    bpm: 72.25,
                    glide: true,
                },
            ],
            meter_changes: vec![MeterChange {
                id: "m".into(),
                bar: 5,
                beats: 7,
                unit: 8,
            }],
            ..Project::default()
        }
    }

    #[test]
    fn ticks_and_beats_convert_exactly_or_to_the_nearest_tick() {
        for t in [0, 1, 479, 960, 961, 123_457, -5] {
            assert_eq!(beats_to_ticks(ticks_to_beats(t)), t);
        }
        assert_eq!(beats_to_ticks(0.25), 240);
        // A third of a beat is 320 ticks; a hair either side rounds to it.
        assert_eq!(beats_to_ticks(1.0 / 3.0 + 1e-6), 320);
        assert_eq!(beats_to_ticks(0.3332), 320);
        assert_eq!(beats_to_ticks(0.0005), 0);
        assert_eq!(beats_to_ticks(0.00053), 1);
    }

    #[test]
    fn seconds_round_to_the_nearest_frame() {
        assert_eq!(seconds_to_frames(1.0, 48_000), 48_000);
        assert_eq!(seconds_to_frames(2.823541666666667, 48_000), 135_530);
        assert_eq!(seconds_to_frames(0.5 / 48_000.0 + 1e-12, 48_000), 1);
        for f in [0, 1, 47_999, 1_234_567_891] {
            assert_eq!(seconds_to_frames(frames_to_seconds(f, 48_000), 48_000), f);
        }
    }

    #[test]
    fn every_frame_comes_back_through_beats_with_tempo_changes() {
        let m = tempo_map(&song());
        // Through the jump, the time signature change and the glide.
        let end = m.frames_at(Ticks(PPQ * 80)).0;
        let mut f = 0;
        while f < end {
            let b = frames_to_beats(&m, f);
            assert_eq!(beats_to_frames(&m, b), f, "frame {f}");
            // And through the written number.
            let b2 = parse_num(&num(b)).unwrap();
            assert_eq!(beats_to_frames(&m, b2), f, "frame {f} as text");
            f += 977;
        }
    }

    #[test]
    fn beats_land_where_the_tempo_map_puts_them() {
        let m = tempo_map(&song());
        for t in [0, PPQ, PPQ * 9, PPQ * 9 + 480, PPQ * 40] {
            let b = ticks_to_beats(t);
            assert_eq!(beats_to_frames(&m, b), m.frames_at(Ticks(t)).0);
            assert_eq!(frames_to_ticks(&m, ticks_to_frames(&m, t)), t);
        }
        // 97.5 bpm at 44.1 kHz: a beat is 27138.46… frames.
        assert_eq!(beats_to_frames(&m, 1.0), 27_138);
        assert!((frames_to_beats(&m, 27_138) - 0.99998).abs() < 1e-4);
    }

    #[test]
    fn levels_convert_between_linear_and_db() {
        assert_eq!(linear_to_db(1.0), 0.0);
        assert!((linear_to_db(2.0) - 6.0206).abs() < 1e-4);
        assert_eq!(linear_to_db(0.0), -144.0);
        assert_eq!(db_to_linear(-144.0), 0.0);
        for db in [-60.0, -6.0, 0.0, 3.5] {
            assert!((linear_to_db(db_to_linear(db)) - db).abs() < 1e-9);
        }
    }

    #[test]
    fn numbers_are_written_to_read_back_the_same() {
        for v in [0.1, 1.0 / 3.0, 123456.789, 1e-9, -2.5] {
            assert_eq!(parse_num(&num(v)), Some(v));
        }
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(f64::NEG_INFINITY), "-inf");
        assert_eq!(parse_num("-inf"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_num(" 2 "), Some(2.0));
    }
}
