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
    pub tracks: Vec<Track>,
    /// In bar order.
    pub markers: Vec<Marker>,
    /// Lesson notes, plain text.
    pub notes: String,
}

impl Default for Project {
    fn default() -> Project {
        Project {
            rate: 48_000,
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            loop_bars: (1, 5),
            tracks: Vec::new(),
            markers: Vec::new(),
            notes: String::new(),
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
