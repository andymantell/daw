// SPDX-License-Identifier: GPL-3.0-or-later
//! The project as plain values: what the document holds, without the
//! CRDT.

/// A whole project.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    /// Sample rate that clip positions are counted in.
    pub rate: u32,
    pub tempo_bpm: f64,
    pub beats_per_bar: u32,
    /// Bars, counted from 1: the loop runs from the start of the first
    /// to the start of the second.
    pub loop_bars: (i64, i64),
    /// The tuner's reference: the pitch of the A above middle C, in Hz.
    pub reference_hz: f64,
    pub tracks: Vec<Track>,
    /// In bar order.
    pub markers: Vec<Marker>,
    /// Lesson notes, plain text.
    pub notes: String,
    /// Pinned comments and their replies, oldest first.
    pub comments: Vec<Comment>,
}

impl Default for Project {
    fn default() -> Project {
        Project {
            rate: 48_000,
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            loop_bars: (1, 5),
            reference_hz: 440.0,
            tracks: Vec::new(),
            markers: Vec::new(),
            notes: String::new(),
            comments: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub gain_db: f64,
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    /// In the order added: later clips draw (and play) on top.
    pub clips: Vec<Clip>,
    /// Insert effects, in order.
    pub effects: Vec<Effect>,
}

impl Track {
    pub fn new(id: String, name: String) -> Track {
        Track {
            id,
            name,
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            clips: Vec::new(),
            effects: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: String,
    /// The audio file's name in the project's assets folder.
    pub asset: String,
    /// What the file was called when it came in, for showing.
    pub name: String,
    /// Where it plays, in frames at the project's rate.
    pub start: i64,
    /// The first frame of the asset that plays, and how many, at the
    /// project's rate.
    pub offset: i64,
    pub len: i64,
    /// Which recorded take (from 0), or `None` for an imported file.
    pub take: Option<u32>,
    pub muted: bool,
    /// Order among the track's clips; higher is on top.
    pub seq: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub id: String,
    pub name: String,
    /// The plugin's class id, 16 bytes.
    pub class: Vec<u8>,
    /// Where the plugin was found, as a hint; the class id is what counts.
    pub bundle: String,
    pub bypass: bool,
    /// The plugin's own settings. `None` when setting leaves the
    /// document's copy as it is (the app reads plugin state only when
    /// saving).
    pub state: Option<EffectState>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectState {
    pub component: Vec<u8>,
    pub controller: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: String,
    /// Counted from 1.
    pub bar: i64,
    pub name: String,
}

/// One person's practice settings. Each computer keeps its own in the
/// document, so a student slowing the song down or muting the teacher's
/// riff changes nothing for the teacher.
#[derive(Clone, Debug, PartialEq)]
pub struct Practice {
    /// Percent.
    pub speed_pct: f64,
    pub semitones: i32,
    pub cents: i32,
    /// Whether the loop is on. The loop's bars are the song's.
    pub looping: bool,
    /// Ids of the tracks silenced to play the part yourself.
    pub muted: Vec<String>,
    pub trainer: TrainerSettings,
}

impl Default for Practice {
    fn default() -> Practice {
        Practice {
            speed_pct: 100.0,
            semitones: 0,
            cents: 0,
            looping: false,
            muted: Vec::new(),
            trainer: TrainerSettings::default(),
        }
    }
}

/// The tempo trainer: speed goes up by `step` percent every `every`
/// loops, up to `target` percent.
#[derive(Clone, Debug, PartialEq)]
pub struct TrainerSettings {
    pub on: bool,
    pub step: f64,
    pub every: u64,
    pub target: f64,
}

impl Default for TrainerSettings {
    fn default() -> TrainerSettings {
        TrainerSettings {
            on: false,
            step: 5.0,
            every: 2,
            target: 100.0,
        }
    }
}

/// A comment pinned to a time on a track or the whole timeline, or a
/// reply to one.
#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub id: String,
    /// Who wrote it: the member's public key, as hex. Empty when written
    /// before the project had members.
    pub author: String,
    /// The comment this replies to, or `None` for the first comment of a
    /// thread. A reply's own track, times and `resolved` are not used.
    pub parent: Option<String>,
    /// The track it is pinned to, or `None` for the whole timeline.
    pub track: Option<String>,
    /// Where it is pinned, in frames at the project's rate: a point, or
    /// a range when `end` is set.
    pub start: i64,
    pub end: Option<i64>,
    pub text: String,
    pub resolved: bool,
    /// When it was written, in milliseconds since 1970, for ordering.
    pub created: i64,
}
