// SPDX-License-Identifier: GPL-3.0-or-later
//! The notation crate's own score types.
//!
//! These are deliberately independent of the project's note model: the
//! timeline lanes (roadmap 3.8) convert between the two. A score holds
//! parts, a part holds bars, and a bar holds events in one or more voices.
//! Positions inside a bar come from adding up the durations of the events
//! before it in the same voice.

use crate::drums::DrumMap;
use crate::tuning::Tuning;

/// Ticks per quarter note. Divisible by 2, 3, 5 and 4 so the usual tuplets
/// come out whole.
pub const TICKS_PER_QUARTER: u32 = 960;

#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub title: String,
    pub composer: String,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    pub kind: PartKind,
    pub bars: Vec<Bar>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PartKind {
    /// Guitar or bass tab, optionally with a standard staff above it.
    Tab(TabSetup),
    /// A standard five-line staff.
    Staff { clef: Clef },
    /// A five-line percussion staff driven by a drum map.
    Drums(DrumMap),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabSetup {
    pub tuning: Tuning,
    /// Capo fret. Fret numbers in the score are counted from the capo, as
    /// a player reads them; the sounding pitch is open string + capo + fret.
    pub capo: u8,
    /// Draw a standard staff above the tab.
    pub show_staff: bool,
    /// Draw stems and beams under the tab.
    pub show_rhythm: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clef {
    Treble,
    /// Treble clef an octave down, as guitar is written.
    Treble8vb,
    Bass,
    /// Bass clef an octave down, as bass guitar is written.
    Bass8vb,
}

impl Clef {
    /// The written diatonic step of the bottom staff line, counted as
    /// octave * 7 + step with C = 0 (so E4 is 30).
    pub(crate) fn bottom_line_step(self) -> i32 {
        match self {
            Clef::Treble | Clef::Treble8vb => 4 * 7 + 2,
            Clef::Bass | Clef::Bass8vb => 2 * 7 + 4,
        }
    }

    /// Semitones added to the sounding pitch to get the written pitch.
    pub(crate) fn transpose(self) -> i32 {
        match self {
            Clef::Treble | Clef::Bass => 0,
            Clef::Treble8vb | Clef::Bass8vb => 12,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSig {
    pub beats: u8,
    pub beat_type: u8,
}

impl TimeSig {
    pub fn bar_ticks(self) -> u32 {
        u32::from(self.beats) * TICKS_PER_QUARTER * 4 / u32::from(self.beat_type.max(1))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bar {
    /// A new time signature from this bar on.
    pub time: Option<TimeSig>,
    /// A new key from this bar on, as sharps (positive) or flats (negative).
    pub key: Option<i8>,
    /// A new tempo (quarter notes per minute) from this bar on.
    pub tempo: Option<f64>,
    pub events: Vec<Event>,
    pub chords: Vec<ChordSymbol>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Voice number from 1. Drum parts use voice 1 for hands (stems up)
    /// and voice 2 for feet (stems down).
    pub voice: u8,
    pub duration: Duration,
    /// No notes means a rest.
    pub notes: Vec<Note>,
    pub strum: Option<Strum>,
}

impl Event {
    pub fn rest(voice: u8, duration: Duration) -> Self {
        Self {
            voice,
            duration,
            notes: Vec::new(),
            strum: None,
        }
    }

    pub fn chord(voice: u8, duration: Duration, notes: Vec<Note>) -> Self {
        Self {
            voice,
            duration,
            notes,
            strum: None,
        }
    }

    pub fn is_rest(&self) -> bool {
        self.notes.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strum {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoteValue {
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    SixtyFourth,
}

impl NoteValue {
    pub const ALL: [NoteValue; 7] = [
        NoteValue::Whole,
        NoteValue::Half,
        NoteValue::Quarter,
        NoteValue::Eighth,
        NoteValue::Sixteenth,
        NoteValue::ThirtySecond,
        NoteValue::SixtyFourth,
    ];

    pub fn ticks(self) -> u32 {
        (TICKS_PER_QUARTER * 4) >> (self as u32)
    }

    /// Beams (or flags) this value carries: 0 for quarter notes and longer.
    pub fn beams(self) -> usize {
        (self as usize).saturating_sub(2)
    }

    /// The MusicXML name of the value.
    pub fn name(self) -> &'static str {
        match self {
            NoteValue::Whole => "whole",
            NoteValue::Half => "half",
            NoteValue::Quarter => "quarter",
            NoteValue::Eighth => "eighth",
            NoteValue::Sixteenth => "16th",
            NoteValue::ThirtySecond => "32nd",
            NoteValue::SixtyFourth => "64th",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == name)
    }
}

/// A tuplet ratio: `actual` notes in the time of `normal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tuplet {
    pub actual: u8,
    pub normal: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Duration {
    pub value: NoteValue,
    pub dots: u8,
    pub tuplet: Option<Tuplet>,
}

impl Duration {
    pub const fn new(value: NoteValue) -> Self {
        Self {
            value,
            dots: 0,
            tuplet: None,
        }
    }

    pub const fn dotted(value: NoteValue) -> Self {
        Self {
            value,
            dots: 1,
            tuplet: None,
        }
    }

    pub const fn triplet(value: NoteValue) -> Self {
        Self {
            value,
            dots: 0,
            tuplet: Some(Tuplet {
                actual: 3,
                normal: 2,
            }),
        }
    }

    pub fn ticks(self) -> u32 {
        let base = self.value.ticks();
        let mut total = base;
        let mut add = base;
        for _ in 0..self.dots {
            add /= 2;
            total += add;
        }
        match self.tuplet {
            Some(t) => total * u32::from(t.normal) / u32::from(t.actual.max(1)),
            None => total,
        }
    }

    /// The plain or dotted value (no tuplet) closest to a number of ticks,
    /// for files that give only a length.
    pub fn from_ticks(ticks: u32) -> Self {
        let mut best = Duration::new(NoteValue::Quarter);
        let mut best_err = u32::MAX;
        for value in NoteValue::ALL {
            for dots in 0..=2 {
                let d = Duration {
                    value,
                    dots,
                    tuplet: None,
                };
                let err = d.ticks().abs_diff(ticks);
                if err < best_err {
                    best = d;
                    best_err = err;
                }
            }
        }
        best
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// Sounding pitch as a MIDI note number. For drum parts this is the
    /// drum map's key (the standard percussion key map by default).
    pub pitch: u8,
    /// String number from 1 (the highest-pitched string), for tab.
    pub string: Option<u8>,
    /// Fret, counted from the capo.
    pub fret: Option<u8>,
    /// The user fixed this position, so the string and fret assigner
    /// must leave it alone.
    pub locked: bool,
    /// Tied into the next note of the same pitch.
    pub tie: bool,
    pub techniques: Vec<Technique>,
}

impl Note {
    pub fn new(pitch: u8) -> Self {
        Self {
            pitch,
            string: None,
            fret: None,
            locked: false,
            tie: false,
            techniques: Vec::new(),
        }
    }

    pub fn at(pitch: u8, string: u8, fret: u8) -> Self {
        Self {
            string: Some(string),
            fret: Some(fret),
            ..Self::new(pitch)
        }
    }

    pub fn with(mut self, technique: Technique) -> Self {
        self.techniques.push(technique);
        self
    }

    pub fn has(&self, technique: &Technique) -> bool {
        self.techniques.iter().any(|t| t == technique)
    }
}

/// Playing techniques for tab and drums (roadmap 3.8).
#[derive(Clone, Debug, PartialEq)]
pub enum Technique {
    // Fretted strings.
    /// Hammer-on to the next note on this string.
    HammerOn,
    /// Pull-off to the next note on this string.
    PullOff,
    /// Slide to the next note on this string.
    Slide,
    /// Bend up by `semitones` (1 = half, 2 = full); `release` returns to
    /// the fretted pitch.
    Bend {
        semitones: f32,
        release: bool,
    },
    Vibrato,
    PalmMute,
    LetRing,
    /// Muted string, drawn as x.
    Dead,
    /// Ghost note, drawn in brackets.
    Ghost,
    NaturalHarmonic,
    ArtificialHarmonic,
    Tap,
    Slap,
    Pop,
    TremoloPicking,
    // Drums (accents and ghosts also apply to strings).
    Accent,
    Flam,
    Drag,
    Roll,
    /// Hi-hat played open.
    Open,
    /// Hi-hat played closed.
    Closed,
    /// Cymbal choked straight after the hit.
    Choke,
    Rimshot,
    CrossStick,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChordSymbol {
    /// Position in the bar, in ticks.
    pub tick: u32,
    pub name: String,
    pub diagram: Option<ChordDiagram>,
}

/// A chord box: which fret each string is held at.
#[derive(Clone, Debug, PartialEq)]
pub struct ChordDiagram {
    /// One entry per string, from the highest string (string 1) down.
    /// `None` is a string not played (x), `Some(0)` an open string.
    pub frets: Vec<Option<u8>>,
    /// The fret shown at the top of the box (1 means the nut is shown).
    pub first_fret: u8,
}

impl Part {
    /// The time signature in force at each bar, starting from 4/4.
    pub fn time_signatures(&self) -> Vec<TimeSig> {
        let mut current = TimeSig {
            beats: 4,
            beat_type: 4,
        };
        self.bars
            .iter()
            .map(|bar| {
                if let Some(t) = bar.time {
                    current = t;
                }
                current
            })
            .collect()
    }

    /// Every event with its position in its bar, in time order (voices
    /// interleaved), as `(bar, tick in bar, event index)`.
    pub fn positions(&self) -> Vec<(usize, u32, usize)> {
        let mut out = Vec::new();
        for (b, bar) in self.bars.iter().enumerate() {
            out.extend(bar_positions(bar).into_iter().map(|(t, e)| (b, t, e)));
        }
        out
    }
}

/// The tick of each event in a bar, in time order then voice order.
pub fn bar_positions(bar: &Bar) -> Vec<(u32, usize)> {
    let mut cursor: Vec<(u8, u32)> = Vec::new();
    let mut out = Vec::new();
    for (i, e) in bar.events.iter().enumerate() {
        let slot = match cursor.iter_mut().find(|(v, _)| *v == e.voice) {
            Some(slot) => slot,
            None => {
                cursor.push((e.voice, 0));
                cursor.last_mut().expect("just pushed")
            }
        };
        out.push((slot.1, i));
        slot.1 += e.duration.ticks();
    }
    out.sort_by_key(|&(t, i)| (t, bar.events[i].voice, i));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_add_up() {
        assert_eq!(Duration::new(NoteValue::Quarter).ticks(), 960);
        assert_eq!(Duration::dotted(NoteValue::Quarter).ticks(), 1440);
        assert_eq!(Duration::triplet(NoteValue::Eighth).ticks(), 320);
        let double = Duration {
            value: NoteValue::Half,
            dots: 2,
            tuplet: None,
        };
        assert_eq!(double.ticks(), 1920 + 960 + 480);
        assert_eq!(
            Duration::from_ticks(1440),
            Duration::dotted(NoteValue::Quarter)
        );
        assert_eq!(
            TimeSig {
                beats: 6,
                beat_type: 8
            }
            .bar_ticks(),
            2880
        );
    }

    #[test]
    fn voices_are_placed_independently() {
        let q = Duration::new(NoteValue::Quarter);
        let h = Duration::new(NoteValue::Half);
        let bar = Bar {
            events: vec![
                Event::chord(1, q, vec![Note::new(38)]),
                Event::chord(1, q, vec![Note::new(38)]),
                Event::chord(2, h, vec![Note::new(36)]),
                Event::chord(2, h, vec![Note::new(36)]),
            ],
            ..Bar::default()
        };
        let pos = bar_positions(&bar);
        assert_eq!(pos, vec![(0, 0), (0, 2), (960, 1), (1920, 3)]);
    }
}
