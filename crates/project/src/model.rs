// SPDX-License-Identifier: GPL-3.0-or-later
//! The project as plain values: what the document holds, without the
//! CRDT.

use sha2::{Digest, Sha256};

/// A whole project.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    /// Sample rate that clip positions are counted in.
    pub rate: u32,
    /// The tempo and time signature at the start of the song.
    pub tempo_bpm: f64,
    pub beats_per_bar: u32,
    /// The note value of a beat: 4 for quarter notes, 8 for eighths.
    pub beat_unit: u32,
    /// Changes of tempo after the start, in order.
    pub tempo_changes: Vec<TempoChange>,
    /// Changes of time signature after the start, in bar order.
    pub meter_changes: Vec<MeterChange>,
    /// Bars, counted from 1: the loop runs from the start of the first
    /// to the start of the second.
    pub loop_bars: (i64, i64),
    /// The tuner's reference: the pitch of the A above middle C, in Hz.
    pub reference_hz: f64,
    pub tracks: Vec<Track>,
    /// In bar order.
    pub markers: Vec<Marker>,
    /// Lesson notes: text with a few simple marks (`#` headings,
    /// `**bold**`, `-` lists) that the app shows formatted.
    pub notes: String,
    /// Tab blocks pinned to sections, oldest first.
    pub tabs: Vec<TabBlock>,
    /// Pinned comments and their replies, oldest first.
    pub comments: Vec<Comment>,
    /// Tracks edited together, in the order made.
    pub edit_groups: Vec<EditGroup>,
    /// Control groups: one fader and mute over several tracks.
    pub control_groups: Vec<ControlGroup>,
}

impl Default for Project {
    fn default() -> Project {
        Project {
            rate: 48_000,
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            beat_unit: 4,
            tempo_changes: Vec::new(),
            meter_changes: Vec::new(),
            loop_bars: (1, 5),
            reference_hz: 440.0,
            tracks: Vec::new(),
            markers: Vec::new(),
            notes: String::new(),
            tabs: Vec::new(),
            comments: Vec::new(),
            edit_groups: Vec::new(),
            control_groups: Vec::new(),
        }
    }
}

/// What a track is for. Buses and folders have no clips: they sum the
/// tracks routed to them, through their own effects and fader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrackKind {
    #[default]
    Audio,
    /// A bus other tracks output or send to: a group, or an effect
    /// return.
    Bus,
    /// A folder: a bus whose tracks sit under it in the lists, and go
    /// to it unless routed elsewhere.
    Folder,
}

impl TrackKind {
    /// As the document stores it. Never change one.
    pub fn name(self) -> &'static str {
        match self {
            TrackKind::Audio => "audio",
            TrackKind::Bus => "bus",
            TrackKind::Folder => "folder",
        }
    }

    pub fn from_name(s: &str) -> TrackKind {
        [TrackKind::Audio, TrackKind::Bus, TrackKind::Folder]
            .into_iter()
            .find(|k| k.name() == s)
            .unwrap_or_default()
    }

    /// Whether other tracks can be routed into it.
    pub fn is_bus(self) -> bool {
        self != TrackKind::Audio
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
    /// The track rendered through its effects, for computers that lack
    /// one of its plugins.
    pub frozen: Option<Frozen>,
    /// Comps made from the track's takes, in the order made.
    pub comps: Vec<Comp>,
    /// The comp that plays, by id, or `None` for the takes as recorded.
    pub comp: Option<String>,
    pub kind: TrackKind,
    /// The folder it sits in, if any.
    pub parent: Option<String>,
    /// The bus its audio goes to after the fader: `None` for its folder,
    /// or the master when it has none.
    pub output: Option<String>,
    /// Copies of its audio sent to buses, in order.
    pub sends: Vec<Send>,
    /// The control group whose fader and mute act on it.
    pub group: Option<String>,
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
            frozen: None,
            comps: Vec::new(),
            comp: None,
            kind: TrackKind::Audio,
            parent: None,
            output: None,
            sends: Vec::new(),
            group: None,
        }
    }

    /// Where its audio goes: the bus it is routed to, else its folder,
    /// else (`None`) the master.
    pub fn goes_to(&self) -> Option<&str> {
        self.output.as_deref().or(self.parent.as_deref())
    }

    /// Whether the track needs frozen audio: it has a plugin switched
    /// on. Every computer has the built-in effects.
    pub fn wants_frozen(&self) -> bool {
        self.effects.iter().any(|e| !e.bypass && e.is_plugin())
    }

    /// A fingerprint of everything the track's frozen audio depends on:
    /// the clips that play, and each effect with its settings. Positions
    /// are counted at `rate`. Mixer settings are left out, as frozen
    /// audio plays through the track's fader like its clips would.
    pub fn freeze_print(&self, rate: u32) -> String {
        let mut h = Sha256::new();
        let mut field = |b: &[u8]| {
            h.update((b.len() as u64).to_le_bytes());
            h.update(b);
        };
        field(&rate.to_le_bytes());
        for c in self.clips.iter().filter(|c| !c.muted) {
            field(c.asset.as_bytes());
            for v in [c.start, c.offset, c.len] {
                field(&v.to_le_bytes());
            }
            // Left out for a clip at its plain level, so prints made
            // before fades still match.
            let sound = c.sound_print(rate);
            if !sound.is_empty() {
                field(sound.as_bytes());
            }
        }
        field(b"fx");
        for e in &self.effects {
            field(e.id.as_bytes());
            field(&e.class);
            field(&[e.bypass as u8]);
            match &e.state {
                Some(s) => {
                    field(&s.component);
                    field(s.controller.as_deref().unwrap_or(b"-"));
                }
                None => field(b"no state"),
            }
            field(e.builtin.as_bytes());
            for (k, v) in &e.params {
                field(k.as_bytes());
                field(&v.to_le_bytes());
            }
            field(e.response.as_bytes());
            if let Some(k) = &e.sidechain {
                field(k.as_bytes());
            }
        }
        h.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// What frozen audio made of this track now, at `rate`, would hold:
    /// everything but the audio and the fingerprint.
    pub fn freeze_shape(&self, rate: u32) -> Frozen {
        let clips: Vec<FrozenClip> = (self.clips.iter())
            .filter(|c| !c.muted && c.len > 0)
            .map(|c| FrozenClip {
                asset: c.asset.clone(),
                start: c.start,
                offset: c.offset,
                len: c.len,
                sound: c.sound_print(rate),
            })
            .collect();
        let first = clips.iter().map(|c| c.start).min().unwrap_or(0);
        let last = clips.iter().map(|c| c.start + c.len).max().unwrap_or(0);
        let rate64 = rate as i64;
        // A second before, so effects' latency loses nothing at the
        // start, and room after for them to ring out.
        let start = (first - rate64).max(0);
        let end = last + FROZEN_TAIL_SECONDS * rate64;
        Frozen {
            asset: String::new(),
            rate,
            start,
            len: (end - start).max(0),
            print: String::new(),
            clips,
            chain: (self.effects.iter())
                .map(|e| (e.id.clone(), e.bypass))
                .collect(),
        }
    }
}

/// A copy of a track's audio sent to a bus (an effect return, say),
/// with its own level.
#[derive(Clone, Debug, PartialEq)]
pub struct Send {
    /// The bus it goes to.
    pub to: String,
    pub gain_db: f64,
    /// Taken before the track's volume fader, so the fader leaves the
    /// send alone (after its effects, pan and mute either way).
    pub pre_fader: bool,
    pub mute: bool,
}

impl Send {
    pub fn to(to: String) -> Send {
        Send {
            to,
            gain_db: 0.0,
            pre_fader: false,
            mute: false,
        }
    }
}

/// One fader and mute over several tracks: its volume adds to each
/// member's, in dB, and its mute silences them all. Their audio is
/// never summed, so each still goes where it is routed.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlGroup {
    pub id: String,
    pub name: String,
    pub gain_db: f64,
    pub mute: bool,
}

impl ControlGroup {
    pub fn new(id: String, name: String) -> ControlGroup {
        ControlGroup {
            id,
            name,
            gain_db: 0.0,
            mute: false,
        }
    }
}

/// How long after a track's last clip its frozen audio goes on, for
/// effects such as echoes to ring out.
pub const FROZEN_TAIL_SECONDS: i64 = 2;

/// A track's audio after its effects, rendered on a computer that has
/// all its plugins and shared with the project, so a computer missing
/// one plays this instead (roadmap 1.3).
#[derive(Clone, Debug, PartialEq)]
pub struct Frozen {
    /// The audio file's name in the project's assets folder.
    pub asset: String,
    /// The rate `start`, `len` and the clips' positions are counted at
    /// (the file's own rate).
    pub rate: u32,
    /// Where the audio starts on the timeline, and how long it is.
    pub start: i64,
    pub len: i64,
    /// [`Track::freeze_print`] of the track it was made from.
    pub print: String,
    /// The clips it was made from.
    pub clips: Vec<FrozenClip>,
    /// The effects it was made through, in order, by id, and whether
    /// each was bypassed.
    pub chain: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrozenClip {
    pub asset: String,
    pub start: i64,
    pub offset: i64,
    pub len: i64,
    /// [`Clip::sound_print`]: its gain, fades and envelope.
    pub sound: String,
}

impl Frozen {
    /// Whether this still sounds like `track` (at `rate`) would through
    /// the effects it was made with: the same clips in the same places
    /// and the same effects, each on or off as it was. Positions may
    /// differ by a frame or two, as converting a project to another
    /// rate and back rounds them. The effects' own settings aren't
    /// compared: on a computer missing a plugin, that plugin's settings
    /// can't change, and the others' are heard only once someone with
    /// every plugin saves.
    pub fn fits(&self, track: &Track, rate: u32) -> bool {
        let now = track.freeze_shape(rate);
        let scale = |v: i64| (v as f64 * self.rate as f64 / rate.max(1) as f64).round() as i64;
        let near = |a: i64, b: i64| (scale(a) - b).abs() <= 2;
        now.chain == self.chain
            && now.clips.len() == self.clips.len()
            && now.clips.iter().zip(&self.clips).all(|(n, f)| {
                n.asset == f.asset
                    && near(n.start, f.start)
                    && near(n.offset, f.offset)
                    && near(n.len, f.len)
                    && n.sound == f.sound
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
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
    /// A piece of the track's comp, cut from take `take`: made again
    /// whenever the comp changes.
    pub comp: bool,
    /// Order among the track's clips; higher is on top.
    pub seq: i64,
    /// The clip's own gain, in dB.
    pub gain_db: f64,
    /// Fades at the start and end, in frames at the project's rate.
    pub fade_in: Fade,
    pub fade_out: Fade,
    /// The gain envelope, in order: levels at frames of the asset,
    /// counted at the project's rate, so the line stays with the audio.
    pub envelope: Vec<GainPoint>,
}

impl Clip {
    /// A fingerprint of what sets the clip's level (gain, fades and
    /// envelope), with frames counted at `rate`, or empty when it plays
    /// as recorded, as clips from before fades did. Times go in as
    /// milliseconds, so the same clip at another rate prints the same.
    pub fn sound_print(&self, rate: u32) -> String {
        if self.gain_db == 0.0
            && self.fade_in.len == 0
            && self.fade_out.len == 0
            && self.envelope.is_empty()
        {
            return String::new();
        }
        let ms = |f: i64| (f as f64 * 1000.0 / rate.max(1) as f64).round() as i64;
        let mut h = Sha256::new();
        h.update(self.gain_db.to_le_bytes());
        for f in [self.fade_in, self.fade_out] {
            h.update(ms(f.len).to_le_bytes());
            h.update(f.curve.name().as_bytes());
            h.update([0]);
        }
        for p in &self.envelope {
            h.update(ms(p.at).to_le_bytes());
            h.update(p.db.to_le_bytes());
        }
        h.finalize()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// A comp: which take plays where, chosen from a track's takes
/// (roadmap 2.2). The pieces that play are clips marked
/// [`Clip::comp`]; these parts say where they came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comp {
    pub id: String,
    pub name: String,
    /// In time order, not overlapping.
    pub parts: Vec<CompPart>,
}

/// Take `take` plays from `start` to `end`, in frames at the project's
/// rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompPart {
    pub take: u32,
    pub start: i64,
    pub end: i64,
}

/// A fade at one end of a clip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fade {
    /// In frames; 0 for none.
    pub len: i64,
    pub curve: Curve,
}

/// A fade's shape, as it rises from silence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    Linear,
    #[default]
    EqualPower,
    SCurve,
    Fast,
    Slow,
}

impl Curve {
    /// As the document stores it. Never change one.
    pub fn name(self) -> &'static str {
        match self {
            Curve::Linear => "linear",
            Curve::EqualPower => "equal-power",
            Curve::SCurve => "s-curve",
            Curve::Fast => "fast",
            Curve::Slow => "slow",
        }
    }

    /// From the document; a shape this version doesn't know plays as
    /// the default.
    pub fn from_name(s: &str) -> Curve {
        [
            Curve::Linear,
            Curve::EqualPower,
            Curve::SCurve,
            Curve::Fast,
            Curve::Slow,
        ]
        .into_iter()
        .find(|c| c.name() == s)
        .unwrap_or_default()
    }
}

/// A point on a clip's gain envelope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainPoint {
    /// A frame of the clip's asset, at the project's rate.
    pub at: i64,
    pub db: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
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
    /// For an effect that comes with the app, which one (such as "eq");
    /// empty for a plugin, whose `class` and `bundle` say which.
    pub builtin: String,
    /// A built-in effect's settings, by name.
    pub params: Vec<(String, f64)>,
    /// The file a built-in effect uses. For a convolution reverb or a
    /// speaker cabinet, its response: empty for its own rooms or
    /// cabinets, `room:` and an id for a recorded room that comes with
    /// the app, or else a sound file. For an amp capture player, its
    /// capture file (empty for none). A file is an asset name, or a full
    /// path until the project is saved.
    pub response: String,
    /// The track whose audio the effect listens to (a compressor's
    /// sidechain), by id, if it has one.
    pub sidechain: Option<String>,
}

impl Effect {
    /// Whether it is a plugin, which a computer may not have.
    pub fn is_plugin(&self) -> bool {
        self.builtin.is_empty()
    }

    /// The sound file it uses, if it uses one: an asset, like a clip's.
    pub fn asset(&self) -> Option<&str> {
        (!self.response.is_empty() && !self.response.starts_with("room:"))
            .then_some(self.response.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectState {
    pub component: Vec<u8>,
    pub controller: Option<Vec<u8>>,
}

/// Tracks edited together (roadmap 2.1), such as a drum kit's mics:
/// cutting, moving, trimming or fading a clip on one does the same to
/// the clips at that time on the others.
#[derive(Clone, Debug, PartialEq)]
pub struct EditGroup {
    pub id: String,
    pub name: String,
    /// Track ids, sorted. A track is in one group at most.
    pub tracks: Vec<String>,
    /// Off, the tracks are edited one at a time but stay grouped.
    pub active: bool,
    /// Order among the groups: when made, in milliseconds since 1970.
    pub created: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: String,
    /// Counted from 1.
    pub bar: i64,
    pub name: String,
}

/// A change of tempo on a beat. Placed by bar and beat, like markers,
/// so it stays on its beat when the time signature before it changes.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoChange {
    pub id: String,
    /// Counted from 1.
    pub bar: i64,
    /// The beat in the bar, counted from 1.
    pub beat: u32,
    pub bpm: f64,
    /// Glide smoothly from the tempo before, arriving at `bpm` here,
    /// instead of jumping to it.
    pub glide: bool,
}

/// A change of time signature, from the start of a bar.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterChange {
    pub id: String,
    /// Counted from 1.
    pub bar: i64,
    pub beats: u32,
    pub unit: u32,
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

/// A block of tab (or anything else to read in a fixed-width font), pinned
/// to the section a marker starts, or to the whole song.
#[derive(Clone, Debug, PartialEq)]
pub struct TabBlock {
    pub id: String,
    /// The marker whose section it belongs to, or `None` for the whole
    /// song. A marker since deleted leaves the block unpinned.
    pub marker: Option<String>,
    pub title: String,
    pub text: String,
    /// Who wrote it: the member's public key, as hex, or empty.
    pub author: String,
    /// When it was written, in milliseconds since 1970, for ordering.
    pub created: i64,
}
