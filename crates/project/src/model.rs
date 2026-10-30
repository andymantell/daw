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
    /// Things to practise by a date, oldest first.
    pub assignments: Vec<Assignment>,
    /// Pinned comments and their replies, oldest first.
    pub comments: Vec<Comment>,
    /// Tracks edited together, in the order made.
    pub edit_groups: Vec<EditGroup>,
    /// Control groups: one fader and mute over several tracks.
    pub control_groups: Vec<ControlGroup>,
    /// Headphone mixes, one per performer, in the order made.
    pub cue_mixes: Vec<CueMix>,
    /// How the song lines up with a picture's timecode.
    pub timecode: TimecodeSetup,
    /// Moments in the picture to write music to, in time order.
    pub hit_points: Vec<HitPoint>,
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
            assignments: Vec::new(),
            comments: Vec::new(),
            edit_groups: Vec::new(),
            control_groups: Vec::new(),
            cue_mixes: Vec::new(),
            timecode: TimecodeSetup::default(),
            hit_points: Vec::new(),
        }
    }
}

/// What a track is for. Buses and folders have no clips: they sum the
/// tracks routed to them, through their own effects and fader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrackKind {
    #[default]
    Audio,
    /// Plays notes (roadmap 3.1): its note clips drive its instrument,
    /// which makes its audio. Audio clips on it are played too.
    Instrument,
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
            TrackKind::Instrument => "instrument",
            TrackKind::Bus => "bus",
            TrackKind::Folder => "folder",
        }
    }

    pub fn from_name(s: &str) -> TrackKind {
        [
            TrackKind::Audio,
            TrackKind::Instrument,
            TrackKind::Bus,
            TrackKind::Folder,
        ]
        .into_iter()
        .find(|k| k.name() == s)
        .unwrap_or_default()
    }

    /// Whether other tracks can be routed into it.
    pub fn is_bus(self) -> bool {
        matches!(self, TrackKind::Bus | TrackKind::Folder)
    }

    /// Whether it plays notes.
    pub fn plays_notes(self) -> bool {
        self == TrackKind::Instrument
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
    /// Note clips (roadmap 3.1), in time order. Only an instrument
    /// track plays them; another kind keeps them, silent.
    pub note_clips: Vec<NoteClip>,
    /// The instrument that turns an instrument track's notes into audio,
    /// before its insert effects: one that comes with the app
    /// ([`Effect::builtin`]) or a plugin. `None` plays no notes (an
    /// instrument track made before one was chosen, or notes sent only
    /// to a MIDI output).
    pub instrument: Option<Effect>,
    /// Insert effects, in order.
    pub effects: Vec<Effect>,
    /// The track rendered through its effects, for computers that lack
    /// one of its plugins.
    pub frozen: Option<Frozen>,
    /// Frozen by its owner: it plays its frozen audio, with its effects
    /// switched off, wherever it is opened.
    pub freeze: bool,
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
    /// Automation lanes (roadmap 2.6): a line each for the settings that
    /// change as the song plays, in the order added.
    pub automation: Vec<Lane>,
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
            note_clips: Vec::new(),
            instrument: None,
            effects: Vec::new(),
            frozen: None,
            freeze: false,
            comps: Vec::new(),
            comp: None,
            kind: TrackKind::Audio,
            parent: None,
            output: None,
            sends: Vec::new(),
            group: None,
            automation: Vec::new(),
        }
    }

    /// Where its audio goes: the bus it is routed to, else its folder,
    /// else (`None`) the master.
    pub fn goes_to(&self) -> Option<&str> {
        self.output.as_deref().or(self.parent.as_deref())
    }

    /// Whether the track needs frozen audio: it has a plugin switched
    /// on (every computer has the built-in effects), or it is frozen and
    /// has any effect switched on.
    pub fn wants_frozen(&self) -> bool {
        (self.effects.iter()).any(|e| !e.bypass && (e.is_plugin() || self.freeze))
            || self.instrument.as_ref().is_some_and(Effect::is_plugin)
    }

    /// The track's notes as they play: each note clip's notes that fall
    /// inside the clip, placed on the timeline, in time order. Ticks at
    /// [`PPQ`].
    pub fn placed_notes(&self) -> Vec<Note> {
        let mut out: Vec<Note> = (self.note_clips.iter())
            .filter(|c| !c.muted)
            .flat_map(NoteClip::placed)
            .collect();
        out.sort_by(|a, b| {
            (a.start, a.pitch, a.id.as_str()).cmp(&(b.start, b.pitch, b.id.as_str()))
        });
        out
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

/// An automation lane: how one setting of a track changes over the
/// song, as points joined by straight lines, held level before the
/// first and after the last. Which setting is named by `param` (see
/// [`Lane::GAIN`] and the other prefixes). A lane with no points leaves
/// the setting as the track has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lane {
    /// Which setting: `gain`, `pan`, `send:<bus id>`, `fx:<effect
    /// id>:<setting key>` for a built-in effect (in the setting's own
    /// unit) or `plugin:<effect id>:<parameter id>` for a plugin (0 to
    /// 1, as the plugin counts it). Never change one.
    pub param: String,
    /// In time order.
    pub points: Vec<AutoPoint>,
}

impl Lane {
    /// The track's volume, in dB.
    pub const GAIN: &'static str = "gain";
    /// The track's pan, −1 to +1.
    pub const PAN: &'static str = "pan";
    /// A send's level, in dB: `send:` and the bus's id.
    pub const SEND: &'static str = "send:";
    /// A built-in effect's setting: `fx:`, the effect's id, `:` and the
    /// setting's key.
    pub const FX: &'static str = "fx:";
    /// A plugin's parameter: `plugin:`, the effect's id, `:` and the
    /// parameter's id.
    pub const PLUGIN: &'static str = "plugin:";

    pub fn new(param: impl Into<String>) -> Lane {
        Lane {
            param: param.into(),
            points: Vec::new(),
        }
    }

    /// The lane's setting, taken apart: a send's bus, or an effect's id
    /// and its setting, if it names one.
    pub fn target(&self) -> LaneTarget<'_> {
        let p = self.param.as_str();
        if p == Lane::GAIN {
            LaneTarget::Gain
        } else if p == Lane::PAN {
            LaneTarget::Pan
        } else if let Some(bus) = p.strip_prefix(Lane::SEND) {
            LaneTarget::Send(bus)
        } else if let Some(rest) = p.strip_prefix(Lane::FX)
            && let Some((effect, key)) = rest.split_once(':')
        {
            LaneTarget::Fx { effect, key }
        } else if let Some(rest) = p.strip_prefix(Lane::PLUGIN)
            && let Some((effect, id)) = rest.split_once(':')
            && let Ok(id) = id.parse()
        {
            LaneTarget::Plugin { effect, id }
        } else {
            LaneTarget::Unknown
        }
    }

    /// The effect the lane belongs to, if it belongs to one.
    pub fn effect(&self) -> Option<&str> {
        match self.target() {
            LaneTarget::Fx { effect, .. } | LaneTarget::Plugin { effect, .. } => Some(effect),
            _ => None,
        }
    }

    /// The value at `at`: on the line between the points either side,
    /// or the nearest point's beyond the ends. `None` with no points.
    pub fn value_at(&self, at: i64) -> Option<f64> {
        let p = &self.points;
        let first = p.first()?;
        if at <= first.at {
            return Some(first.value);
        }
        let last = p.last()?;
        if at >= last.at {
            return Some(last.value);
        }
        let i = p.partition_point(|q| q.at <= at);
        let (a, b) = (&p[i - 1], &p[i]);
        if b.at == a.at {
            return Some(b.value);
        }
        let k = (at - a.at) as f64 / (b.at - a.at) as f64;
        Some(a.value + (b.value - a.value) * k)
    }

    /// Puts a point in, keeping time order; a point already at `at` is
    /// replaced. Returns where it went.
    pub fn put(&mut self, at: i64, value: f64) -> usize {
        let i = self.points.partition_point(|q| q.at < at);
        if self.points.get(i).is_some_and(|q| q.at == at) {
            self.points[i].value = value;
        } else {
            self.points.insert(i, AutoPoint { at, value });
        }
        i
    }

    /// Replaces the points in `[from, to]` with `written`, as a pass of
    /// writing does: points on the way are gone, and the new ones take
    /// their place. The lane's value just outside the pass is kept by
    /// points at its ends, so the pass changes nothing beyond it.
    pub fn write(&mut self, from: i64, to: i64, written: &[AutoPoint]) {
        let (from, to) = (from.min(to), from.max(to));
        let before = self.value_at(from);
        let after = self.value_at(to);
        self.points.retain(|p| p.at < from || p.at > to);
        if let Some(v) = before
            && written.first().is_none_or(|p| p.at > from)
        {
            self.put(from, v);
        }
        if let Some(v) = after
            && written.last().is_none_or(|p| p.at < to)
        {
            self.put(to, v);
        }
        for p in written {
            if p.at >= from && p.at <= to {
                self.put(p.at, p.value);
            }
        }
    }
}

/// What a [`Lane`] sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneTarget<'a> {
    Gain,
    Pan,
    /// A send, by the bus's id.
    Send(&'a str),
    /// A built-in effect's setting.
    Fx {
        effect: &'a str,
        key: &'a str,
    },
    /// A plugin's parameter.
    Plugin {
        effect: &'a str,
        id: u32,
    },
    /// A setting this version doesn't know.
    Unknown,
}

/// A point on an automation lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoPoint {
    /// A frame on the timeline, at the project's rate.
    pub at: i64,
    pub value: f64,
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

/// A headphone mix for one performer (roadmap 2.3): its own level and
/// pan for each track, apart from the main mix, plus the click and
/// talkback. Which outputs it plays on belongs to the computer, not the
/// project.
#[derive(Clone, Debug, PartialEq)]
pub struct CueMix {
    pub id: String,
    pub name: String,
    pub gain_db: f64,
    pub mute: bool,
    /// The same as the main mix, rather than its own levels.
    pub follow_main: bool,
    /// The click's level in it; `None` for no click.
    pub click_db: Option<f64>,
    /// Talkback reaches it.
    pub talkback: bool,
    /// Tracks' places in it, by track id. A track not listed is at its
    /// [`CueMix::level`] default.
    pub levels: Vec<CueLevel>,
}

/// One track's place in a headphone mix.
#[derive(Clone, Debug, PartialEq)]
pub struct CueLevel {
    pub track: String,
    pub gain_db: f64,
    pub pan: f64,
    pub mute: bool,
}

impl CueLevel {
    pub fn new(track: String, gain_db: f64) -> CueLevel {
        CueLevel {
            track,
            gain_db,
            pan: 0.0,
            mute: false,
        }
    }
}

/// A level that is off, in a headphone mix.
pub const CUE_OFF_DB: f64 = -144.0;

impl CueMix {
    pub fn new(id: String, name: String) -> CueMix {
        CueMix {
            id,
            name,
            gain_db: 0.0,
            mute: false,
            follow_main: false,
            click_db: Some(0.0),
            talkback: true,
            levels: Vec::new(),
        }
    }

    /// The track's place in the mix: as set, or else at 0 dB for an
    /// audio track and off for a bus or folder, whose tracks are heard
    /// on their own already.
    pub fn level(&self, track: &Track) -> CueLevel {
        self.levels
            .iter()
            .find(|l| l.track == track.id)
            .cloned()
            .unwrap_or_else(|| {
                let db = if track.kind.is_bus() { CUE_OFF_DB } else { 0.0 };
                CueLevel::new(track.id.clone(), db)
            })
    }

    /// Sets the track's place in the mix.
    pub fn set_level(&mut self, level: CueLevel) {
        match self.levels.iter_mut().find(|l| l.track == level.track) {
            Some(l) => *l = level,
            None => self.levels.push(level),
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
    /// A plugin's extra outputs mixed into the track, by the plugin's
    /// bus index (its main output, 0, always is). Empty for most.
    pub outputs: Vec<u32>,
    /// The effect container it sits in, by the container's id on the
    /// same track; `None` for the track's own chain. Effects in one of a
    /// container's chains play in the order of the track's list.
    pub container: Option<String>,
    /// Which of the container's chains, from 0.
    pub branch: u32,
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

/// The picture's frame rate and start timecode, and where on the
/// timeline that start falls.
#[derive(Clone, Debug, PartialEq)]
pub struct TimecodeSetup {
    /// The frame rate's id, such as "25" or "29.97df".
    pub rate: String,
    /// The start timecode, as written: "01:00:00:00".
    pub start: String,
    /// Where the start timecode falls, in frames at the project's rate.
    pub offset: i64,
    /// Show the timecode ruler and position.
    pub shown: bool,
}

impl Default for TimecodeSetup {
    fn default() -> TimecodeSetup {
        TimecodeSetup {
            rate: "25".into(),
            start: "00:00:00:00".into(),
            offset: 0,
            shown: false,
        }
    }
}

/// A moment in the picture to write music to ("door slams"). Placed in
/// time, not in bars, so it stays with the picture when the tempo
/// changes.
#[derive(Clone, Debug, PartialEq)]
pub struct HitPoint {
    pub id: String,
    /// In frames at the project's rate: the start of a picture frame.
    pub at: i64,
    pub name: String,
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

/// Something to practise by a date: "bars 9–16 at 90 bpm by next week".
/// Progress on it is worked out from the practice log, so both teacher
/// and student see the same.
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub id: String,
    /// Bars, counted from 1: from the start of the first to the start
    /// of the second, as the loop has them.
    pub bars: (i64, i64),
    /// The tempo to reach, in beats per minute.
    pub bpm: f64,
    /// When it is due, in milliseconds since 1970, or 0 for no date.
    pub due: i64,
    /// Anything else to say about it.
    pub note: String,
    /// The lesson it was set in, if any.
    pub lesson: Option<String>,
    /// Who set it: the member's public key, as hex, or empty.
    pub author: String,
    /// When it was set, in milliseconds since 1970, for ordering.
    pub created: i64,
}

/// Ticks per quarter note, as the engine counts musical time (decision
/// 0005). Note positions and lengths are in these.
pub const PPQ: i64 = 960;

/// A note (roadmap 3.1), as a note clip holds it: where it starts and
/// how long it lasts in ticks at [`PPQ`] from the clip's content start,
/// so it moves with the tempo, plus how it is played. The optional
/// string, fret, techniques and drum articulation are what notation
/// shows (decision 0009); the instrument plays the pitch either way.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: String,
    /// From the start of the clip's content, in ticks.
    pub start: i64,
    /// In ticks; at least 1.
    pub len: i64,
    /// 0 to 127: 60 is middle C.
    pub pitch: u8,
    /// 1 to 127.
    pub velocity: u8,
    /// 0 to 15, for instruments and MIDI outputs that listen per channel
    /// (a MIDI guitar's string per channel, say).
    pub channel: u8,
    /// Which string plays it, counted from 1 at the highest, for tab.
    pub string: Option<u8>,
    /// Which fret, 0 for open, for tab.
    pub fret: Option<u8>,
    /// How far it bends up, in semitones; 0 for no bend.
    pub bend: f64,
    /// Playing techniques, for tab, in no particular order.
    pub techniques: Vec<Technique>,
    /// How a drum hit is played, for a drum staff.
    pub articulation: Option<Articulation>,
    /// 0 to 1: the chance the note plays at all, for patterns; 1 always.
    pub chance: f64,
}

impl Note {
    pub fn new(start: i64, len: i64, pitch: u8, velocity: u8) -> Note {
        Note {
            id: crate::new_id(),
            start,
            len: len.max(1),
            pitch,
            velocity: velocity.clamp(1, 127),
            channel: 0,
            string: None,
            fret: None,
            bend: 0.0,
            techniques: Vec::new(),
            articulation: None,
            chance: 1.0,
        }
    }

    pub fn end(&self) -> i64 {
        self.start + self.len
    }

    /// The pitch's name, such as `C4` for middle C or `F#2`.
    pub fn pitch_name(pitch: u8) -> String {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        let octave = pitch as i32 / 12 - 1;
        format!("{}{octave}", NAMES[pitch as usize % 12])
    }

    /// Whether `other` is the same note played the same way, wherever
    /// and whenever: everything but the id and the times.
    pub fn same_sound(&self, other: &Note) -> bool {
        self.pitch == other.pitch
            && self.velocity == other.velocity
            && self.channel == other.channel
            && self.string == other.string
            && self.fret == other.fret
            && self.bend == other.bend
            && self.techniques == other.techniques
            && self.articulation == other.articulation
            && self.chance == other.chance
    }
}

/// A way of playing a note on a string (roadmap 3.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Technique {
    HammerOn,
    PullOff,
    /// Slides into the next note on the string.
    Slide,
    Vibrato,
    PalmMute,
    LetRing,
    /// A muted, pitchless hit.
    Dead,
    /// Played very softly.
    Ghost,
    NaturalHarmonic,
    ArtificialHarmonic,
    Tap,
    Slap,
    Pop,
    TremoloPicking,
    StrumDown,
    StrumUp,
}

impl Technique {
    pub const ALL: [Technique; 16] = [
        Technique::HammerOn,
        Technique::PullOff,
        Technique::Slide,
        Technique::Vibrato,
        Technique::PalmMute,
        Technique::LetRing,
        Technique::Dead,
        Technique::Ghost,
        Technique::NaturalHarmonic,
        Technique::ArtificialHarmonic,
        Technique::Tap,
        Technique::Slap,
        Technique::Pop,
        Technique::TremoloPicking,
        Technique::StrumDown,
        Technique::StrumUp,
    ];

    /// As the document stores it. Never change one.
    pub fn name(self) -> &'static str {
        match self {
            Technique::HammerOn => "hammer-on",
            Technique::PullOff => "pull-off",
            Technique::Slide => "slide",
            Technique::Vibrato => "vibrato",
            Technique::PalmMute => "palm-mute",
            Technique::LetRing => "let-ring",
            Technique::Dead => "dead",
            Technique::Ghost => "ghost",
            Technique::NaturalHarmonic => "natural-harmonic",
            Technique::ArtificialHarmonic => "artificial-harmonic",
            Technique::Tap => "tap",
            Technique::Slap => "slap",
            Technique::Pop => "pop",
            Technique::TremoloPicking => "tremolo-picking",
            Technique::StrumDown => "strum-down",
            Technique::StrumUp => "strum-up",
        }
    }

    /// From the document; `None` for one this version doesn't know.
    pub fn from_name(s: &str) -> Option<Technique> {
        Technique::ALL.into_iter().find(|t| t.name() == s)
    }

    /// For showing.
    pub fn label(self) -> &'static str {
        match self {
            Technique::HammerOn => "Hammer-on",
            Technique::PullOff => "Pull-off",
            Technique::Slide => "Slide",
            Technique::Vibrato => "Vibrato",
            Technique::PalmMute => "Palm mute",
            Technique::LetRing => "Let ring",
            Technique::Dead => "Dead note",
            Technique::Ghost => "Ghost note",
            Technique::NaturalHarmonic => "Natural harmonic",
            Technique::ArtificialHarmonic => "Artificial harmonic",
            Technique::Tap => "Tap",
            Technique::Slap => "Slap",
            Technique::Pop => "Pop",
            Technique::TremoloPicking => "Tremolo picking",
            Technique::StrumDown => "Strum down",
            Technique::StrumUp => "Strum up",
        }
    }
}

/// How a drum hit is played (roadmap 3.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Articulation {
    Accent,
    Ghost,
    Flam,
    Drag,
    Roll,
    /// Hi-hat open, closed or played with the pedal.
    Open,
    Closed,
    Pedal,
    /// A cymbal grabbed after the hit.
    Choke,
    Rimshot,
    CrossStick,
}

impl Articulation {
    pub const ALL: [Articulation; 11] = [
        Articulation::Accent,
        Articulation::Ghost,
        Articulation::Flam,
        Articulation::Drag,
        Articulation::Roll,
        Articulation::Open,
        Articulation::Closed,
        Articulation::Pedal,
        Articulation::Choke,
        Articulation::Rimshot,
        Articulation::CrossStick,
    ];

    /// As the document stores it. Never change one.
    pub fn name(self) -> &'static str {
        match self {
            Articulation::Accent => "accent",
            Articulation::Ghost => "ghost",
            Articulation::Flam => "flam",
            Articulation::Drag => "drag",
            Articulation::Roll => "roll",
            Articulation::Open => "open",
            Articulation::Closed => "closed",
            Articulation::Pedal => "pedal",
            Articulation::Choke => "choke",
            Articulation::Rimshot => "rimshot",
            Articulation::CrossStick => "cross-stick",
        }
    }

    /// From the document; `None` for one this version doesn't know.
    pub fn from_name(s: &str) -> Option<Articulation> {
        Articulation::ALL.into_iter().find(|a| a.name() == s)
    }

    /// For showing.
    pub fn label(self) -> &'static str {
        match self {
            Articulation::Accent => "Accent",
            Articulation::Ghost => "Ghost",
            Articulation::Flam => "Flam",
            Articulation::Drag => "Drag",
            Articulation::Roll => "Roll",
            Articulation::Open => "Open hi-hat",
            Articulation::Closed => "Closed hi-hat",
            Articulation::Pedal => "Pedal hi-hat",
            Articulation::Choke => "Choke",
            Articulation::Rimshot => "Rimshot",
            Articulation::CrossStick => "Cross-stick",
        }
    }
}

/// A clip of notes on a track (roadmap 3.1). It sits in musical time, so
/// it moves with the tempo: `start` and `len` are ticks at [`PPQ`] on
/// the timeline. Its notes are counted from the clip's content start,
/// `offset` ticks before where it begins (so trimming its start keeps
/// the notes where they were), and only notes that begin inside the
/// clip play.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteClip {
    pub id: String,
    pub name: String,
    /// Where it begins on the timeline, in ticks.
    pub start: i64,
    /// How long it is, in ticks.
    pub len: i64,
    /// How far into the content the clip begins, in ticks.
    pub offset: i64,
    /// In time order.
    pub notes: Vec<Note>,
    pub muted: bool,
    /// Which recorded pass made it (from 0), or `None` for one drawn or
    /// imported.
    pub take: Option<u32>,
}

impl NoteClip {
    pub fn new(id: String, name: String, start: i64, len: i64) -> NoteClip {
        NoteClip {
            id,
            name,
            start,
            len: len.max(1),
            offset: 0,
            notes: Vec::new(),
            muted: false,
            take: None,
        }
    }

    pub fn end(&self) -> i64 {
        self.start + self.len
    }

    /// Where a note at `rel` ticks into the content falls on the
    /// timeline.
    pub fn place(&self, rel: i64) -> i64 {
        self.start - self.offset + rel
    }

    /// The notes that play: those that begin inside the clip, with their
    /// starts on the timeline, each cut short at the clip's end.
    pub fn placed(&self) -> impl Iterator<Item = Note> + '_ {
        self.notes
            .iter()
            .filter(|n| n.start >= self.offset && n.start < self.offset + self.len)
            .map(|n| {
                let start = self.place(n.start);
                Note {
                    start,
                    len: n.len.min(self.end() - start).max(1),
                    ..n.clone()
                }
            })
    }

    /// Puts the notes in time order, as the document keeps them.
    pub fn sort_notes(&mut self) {
        self.notes.sort_by(|a, b| {
            (a.start, a.pitch, a.id.as_str()).cmp(&(b.start, b.pitch, b.id.as_str()))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_places_the_notes_that_begin_inside_it() {
        let mut c = NoteClip::new("c".into(), "Riff".into(), PPQ * 4, PPQ * 2);
        c.offset = PPQ;
        c.notes = vec![
            Note::new(0, PPQ, 60, 100),           // before the clip: trimmed off
            Note::new(PPQ, PPQ, 62, 100),         // at the clip's start
            Note::new(PPQ * 2, PPQ * 4, 64, 100), // runs past the end: cut short
            Note::new(PPQ * 3, PPQ, 65, 100),     // after the end
        ];
        let placed: Vec<(i64, i64, u8)> = c.placed().map(|n| (n.start, n.len, n.pitch)).collect();
        assert_eq!(placed, [(PPQ * 4, PPQ, 62), (PPQ * 5, PPQ, 64)]);
    }

    #[test]
    fn a_tracks_notes_come_in_time_order_without_muted_clips() {
        let mut t = Track::new("t".into(), "Keys".into());
        let mut a = NoteClip::new("a".into(), "A".into(), PPQ * 8, PPQ * 4);
        a.notes.push(Note::new(0, PPQ, 60, 100));
        let mut b = NoteClip::new("b".into(), "B".into(), 0, PPQ * 4);
        b.notes.push(Note::new(PPQ, PPQ, 62, 100));
        let mut m = NoteClip::new("m".into(), "M".into(), 0, PPQ * 4);
        m.notes.push(Note::new(0, PPQ, 70, 100));
        m.muted = true;
        t.note_clips = vec![a, b, m];
        let pitches: Vec<u8> = t.placed_notes().iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, [62, 60]);
    }

    #[test]
    fn pitch_names() {
        assert_eq!(Note::pitch_name(60), "C4");
        assert_eq!(Note::pitch_name(42), "F#2");
        assert_eq!(Note::pitch_name(0), "C-1");
        assert_eq!(Note::pitch_name(127), "G9");
    }

    #[test]
    fn technique_and_articulation_names_round_trip() {
        for t in Technique::ALL {
            assert_eq!(Technique::from_name(t.name()), Some(t));
        }
        for a in Articulation::ALL {
            assert_eq!(Articulation::from_name(a.name()), Some(a));
        }
        assert_eq!(Technique::from_name("made up"), None);
    }

    fn lane(points: &[(i64, f64)]) -> Lane {
        let mut l = Lane::new(Lane::GAIN);
        for &(at, v) in points {
            l.put(at, v);
        }
        l
    }

    #[test]
    fn a_lane_reads_between_its_points_and_holds_its_ends() {
        let l = lane(&[(100, 0.0), (0, -12.0), (200, 6.0)]);
        assert_eq!(l.value_at(-5), Some(-12.0));
        assert_eq!(l.value_at(50), Some(-6.0));
        assert_eq!(l.value_at(150), Some(3.0));
        assert_eq!(l.value_at(900), Some(6.0));
        assert_eq!(Lane::new(Lane::PAN).value_at(0), None);
    }

    #[test]
    fn putting_a_point_at_a_taken_frame_replaces_it() {
        let mut l = lane(&[(10, 1.0)]);
        assert_eq!(l.put(10, 2.0), 0);
        assert_eq!(l.points, vec![AutoPoint { at: 10, value: 2.0 }]);
    }

    #[test]
    fn a_pass_of_writing_replaces_only_what_it_passed() {
        let mut l = lane(&[(0, 0.0), (1000, -10.0), (2000, 0.0)]);
        let written = [
            AutoPoint {
                at: 500,
                value: -3.0,
            },
            AutoPoint {
                at: 1500,
                value: -4.0,
            },
        ];
        l.write(400, 1600, &written);
        // Held at its old value just outside the pass, the pass's own
        // points inside, and the old point on the way gone.
        assert_eq!(l.value_at(0), Some(0.0));
        assert_eq!(l.value_at(400), Some(-4.0));
        assert_eq!(l.value_at(500), Some(-3.0));
        assert_eq!(l.value_at(1500), Some(-4.0));
        assert_eq!(l.value_at(1600), Some(-4.0));
        assert_eq!(l.value_at(2000), Some(0.0));
        assert!(!l.points.iter().any(|p| p.at == 1000));

        // Writing over an empty lane just takes the points.
        let mut e = Lane::new(Lane::PAN);
        e.write(0, 100, &written[..1]);
        assert_eq!(
            e.points,
            vec![AutoPoint {
                at: 500,
                value: -3.0
            }][..0]
                .to_vec()
        );
        e.write(400, 600, &written[..1]);
        assert_eq!(
            e.points,
            vec![AutoPoint {
                at: 500,
                value: -3.0
            }]
        );
    }

    #[test]
    fn lane_names_take_apart() {
        assert_eq!(Lane::new("gain").target(), LaneTarget::Gain);
        assert_eq!(Lane::new("send:abc").target(), LaneTarget::Send("abc"));
        assert_eq!(
            Lane::new("fx:e1:threshold").target(),
            LaneTarget::Fx {
                effect: "e1",
                key: "threshold"
            }
        );
        assert_eq!(
            Lane::new("plugin:e2:17").target(),
            LaneTarget::Plugin {
                effect: "e2",
                id: 17
            }
        );
        assert_eq!(Lane::new("plugin:e2:x").target(), LaneTarget::Unknown);
        assert_eq!(Lane::new("fx:e1:threshold").effect(), Some("e1"));
        assert_eq!(Lane::new("pan").effect(), None);
    }
}
