// SPDX-License-Identifier: GPL-3.0-or-later
//! Patterns (roadmap 3.3): drum patterns made of lanes of steps, played
//! by note clips that link to them.
//!
//! A pattern is a set of lanes, one per sound (a drum, say), each a row
//! of steps at its own rate, length and direction, so lanes of different
//! lengths drift against each other (polymeter: triplet hi-hats over a
//! sixteenth-note kick). A note clip that names a pattern plays it from
//! its content start, each lane cycling for as long as the clip runs;
//! several clips can name one pattern, so editing it changes them all.
//! Such a clip's notes are made from the pattern ([`fill_pattern_clips`])
//! rather than stored, so the rest of the app (the timeline, the engine,
//! MIDI files) sees plain notes.
//!
//! Chance is decided as the song plays, a fresh roll each time round, by
//! the engine: a step's chance becomes its notes' [`Note::chance`]. Notes
//! can share one roll ([`Note::group`]), so a whole group plays or
//! doesn't, or have one of them picked at random ([`Note::one_of`]).
//! [`NoteClip::rolls`] works out what each note shares.
//!
//! In the document: a `patterns` map from id to pattern, each a map of
//! its settings and a `lanes` map from id to lane; a lane's steps are a
//! `steps` map from step number to step, one entry per field that isn't
//! the default, so two people editing different steps keep both.

use std::collections::{HashMap, HashSet};

use loro::{LoroDoc, LoroValue};

use crate::doc::{Writer, boolean, float, int, keys, string};
use crate::{Note, NoteClip, PPQ, Track};

/// How long one step of a lane is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepRate {
    Quarter,
    Eighth,
    EighthTriplet,
    #[default]
    Sixteenth,
    SixteenthTriplet,
    ThirtySecond,
}

impl StepRate {
    pub const ALL: [StepRate; 6] = [
        StepRate::Quarter,
        StepRate::Eighth,
        StepRate::EighthTriplet,
        StepRate::Sixteenth,
        StepRate::SixteenthTriplet,
        StepRate::ThirtySecond,
    ];

    /// One step, in ticks at [`PPQ`].
    pub fn ticks(self) -> i64 {
        match self {
            StepRate::Quarter => PPQ,
            StepRate::Eighth => PPQ / 2,
            StepRate::EighthTriplet => PPQ / 3,
            StepRate::Sixteenth => PPQ / 4,
            StepRate::SixteenthTriplet => PPQ / 6,
            StepRate::ThirtySecond => PPQ / 8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StepRate::Quarter => "1/4",
            StepRate::Eighth => "1/8",
            StepRate::EighthTriplet => "1/8 triplet",
            StepRate::Sixteenth => "1/16",
            StepRate::SixteenthTriplet => "1/16 triplet",
            StepRate::ThirtySecond => "1/32",
        }
    }

    /// Kept in the document: never change one.
    pub fn name(self) -> &'static str {
        match self {
            StepRate::Quarter => "4",
            StepRate::Eighth => "8",
            StepRate::EighthTriplet => "8t",
            StepRate::Sixteenth => "16",
            StepRate::SixteenthTriplet => "16t",
            StepRate::ThirtySecond => "32",
        }
    }

    pub fn from_name(s: &str) -> Option<StepRate> {
        StepRate::ALL.into_iter().find(|r| r.name() == s)
    }
}

/// Which way a lane goes through its steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Forward,
    Backward,
    /// Forward then back, without playing the end steps twice.
    PingPong,
    /// A step picked at random each time.
    Random,
}

impl Direction {
    pub const ALL: [Direction; 4] = [
        Direction::Forward,
        Direction::Backward,
        Direction::PingPong,
        Direction::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Direction::Forward => "Forward",
            Direction::Backward => "Backward",
            Direction::PingPong => "Back and forth",
            Direction::Random => "Random",
        }
    }

    /// Kept in the document: never change one.
    pub fn name(self) -> &'static str {
        match self {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
            Direction::PingPong => "pingpong",
            Direction::Random => "random",
        }
    }

    pub fn from_name(s: &str) -> Option<Direction> {
        Direction::ALL.into_iter().find(|d| d.name() == s)
    }
}

/// One step of a lane.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub on: bool,
    /// 1 to 127.
    pub velocity: u8,
    /// Semitones from the lane's pitch.
    pub pitch: i8,
    /// 0 to 1: the chance it plays each time round.
    pub chance: f64,
    /// Hits in the step (a ratchet): 1 to [`MAX_REPEATS`].
    pub repeats: u8,
    /// How far it is moved off the grid, as a fraction of a step: -0.5
    /// (early) to 0.5 (late).
    pub offset: f64,
    /// How long each hit lasts, as a fraction of its share of the step:
    /// [`MIN_GATE`] to [`MAX_GATE`].
    pub gate: f64,
    /// Up to how far the velocity varies either way, 0 to 127.
    pub vary: u8,
}

pub const MAX_REPEATS: u8 = 8;
pub const MIN_GATE: f64 = 0.05;
/// Past 1 a hit runs over into the next step, to tie notes.
pub const MAX_GATE: f64 = 4.0;
/// The most steps a lane can have.
pub const MAX_STEPS: usize = 64;

impl Default for Step {
    fn default() -> Step {
        Step {
            on: false,
            velocity: 100,
            pitch: 0,
            chance: 1.0,
            repeats: 1,
            offset: 0.0,
            gate: 0.5,
            vary: 0,
        }
    }
}

impl Step {
    /// The same step with every value in range.
    fn tidy(mut self) -> Step {
        self.velocity = self.velocity.clamp(1, 127);
        self.pitch = self.pitch.clamp(-48, 48);
        self.chance = finite(self.chance, 1.0).clamp(0.0, 1.0);
        self.repeats = self.repeats.clamp(1, MAX_REPEATS);
        self.offset = finite(self.offset, 0.0).clamp(-0.5, 0.5);
        self.gate = finite(self.gate, 0.5).clamp(MIN_GATE, MAX_GATE);
        self.vary = self.vary.min(127);
        self
    }
}

fn finite(v: f64, or: f64) -> f64 {
    if v.is_finite() { v } else { or }
}

/// One row of a pattern: a sound and its steps.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternLane {
    pub id: String,
    pub name: String,
    /// The note it plays, 0 to 127; a drum machine's pad listens for it.
    pub pitch: u8,
    /// Its length is the number of steps, 1 to [`MAX_STEPS`].
    pub steps: Vec<Step>,
    pub rate: StepRate,
    pub direction: Direction,
    pub muted: bool,
}

impl PatternLane {
    pub fn new(id: String, name: String, pitch: u8, steps: usize) -> PatternLane {
        PatternLane {
            id,
            name,
            pitch: pitch.min(127),
            steps: vec![Step::default(); steps.clamp(1, MAX_STEPS)],
            rate: StepRate::default(),
            direction: Direction::default(),
            muted: false,
        }
    }

    /// Changes how many steps it has, keeping the ones it had.
    pub fn set_len(&mut self, n: usize) {
        self.steps.resize(n.clamp(1, MAX_STEPS), Step::default());
    }

    /// Turns `hits` steps on, spread as evenly as they go (a Euclidean
    /// rhythm), turned `rotate` steps later; the rest go off.
    pub fn fill(&mut self, hits: usize, rotate: usize) {
        let on = euclid(hits, self.steps.len(), rotate);
        for (s, on) in self.steps.iter_mut().zip(on) {
            s.on = on;
        }
    }

    /// Which of its steps it plays at its `i`th step from the start.
    fn step_at(&self, i: u64, seed: u64) -> usize {
        let n = self.steps.len() as u64;
        let k = i % n;
        (match self.direction {
            Direction::Forward => k,
            Direction::Backward => n - 1 - k,
            Direction::PingPong if n > 1 => {
                let period = 2 * n - 2;
                let p = i % period;
                if p < n { p } else { period - p }
            }
            Direction::PingPong => 0,
            Direction::Random => mix(seed ^ hash(self.id.as_bytes()), i) % n,
        }) as usize
    }
}

/// `hits` onsets spread as evenly as they go over `steps`, the first on
/// the first step before turning `rotate` steps later.
pub fn euclid(hits: usize, steps: usize, rotate: usize) -> Vec<bool> {
    if steps == 0 {
        return Vec::new();
    }
    let hits = hits.min(steps);
    let rotate = rotate % steps;
    (0..steps)
        .map(|i| {
            let j = (i + steps - rotate) % steps;
            hits > 0 && (j * hits) % steps < hits
        })
        .collect()
}

/// A drum pattern: lanes of steps, played by the clips that link to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub id: String,
    pub name: String,
    /// How long a clip made of it is to begin with, in ticks.
    pub len: i64,
    /// 0 to 1: how far every second step of a lane is moved late, from
    /// straight (0) to half a step (1).
    pub swing: f64,
    /// Top to bottom.
    pub lanes: Vec<PatternLane>,
}

/// Lanes a new pattern starts with: General MIDI's drum notes, which a
/// drum machine's pads use too.
pub const DRUM_LANES: [(&str, u8); 4] = [
    ("Kick", 36),
    ("Snare", 38),
    ("Closed hi-hat", 42),
    ("Open hi-hat", 46),
];

impl Pattern {
    /// An empty pattern `len` ticks long, with no lanes.
    pub fn new(id: String, name: String, len: i64) -> Pattern {
        Pattern {
            id,
            name,
            len: len.max(1),
            swing: 0.0,
            lanes: Vec::new(),
        }
    }

    /// A pattern a bar of `bar` ticks long with the drum lanes, as many
    /// sixteenth steps as fill the bar, and a plain rock beat to start
    /// from.
    pub fn drums(id: String, name: String, bar: i64) -> Pattern {
        let mut p = Pattern::new(id, name, bar);
        let steps = ((bar / StepRate::Sixteenth.ticks()) as usize).clamp(1, MAX_STEPS);
        for (name, pitch) in DRUM_LANES {
            p.lanes
                .push(PatternLane::new(crate::new_id(), name.into(), pitch, steps));
        }
        let beat = 4usize;
        for (i, s) in p.lanes[0].steps.iter_mut().enumerate() {
            s.on = i % (beat * 2) == 0;
        }
        for (i, s) in p.lanes[1].steps.iter_mut().enumerate() {
            s.on = i % (beat * 2) == beat;
        }
        for (i, s) in p.lanes[2].steps.iter_mut().enumerate() {
            s.on = i % 2 == 0;
            s.velocity = if i % beat == 0 { 100 } else { 80 };
        }
        p
    }

    /// The notes it plays from its start up to `until` ticks, each lane
    /// cycling through its steps, in time order. Notes are counted from
    /// the content start of a clip that plays it; each one's id says
    /// which lane, step and hit made it, so it stays the same however
    /// often this is worked out.
    pub fn notes(&self, until: i64) -> Vec<Note> {
        let seed = hash(self.id.as_bytes());
        let swing = finite(self.swing, 0.0).clamp(0.0, 1.0);
        let mut out = Vec::new();
        for lane in self
            .lanes
            .iter()
            .filter(|l| !l.muted && !l.steps.is_empty())
        {
            let len = lane.rate.ticks();
            let lane_seed = seed ^ hash(lane.id.as_bytes());
            let mut i: i64 = 0;
            // A step moved early by its offset can still land before
            // `until`, so go one step past.
            while i * len < until + len {
                let step = lane.steps[lane.step_at(i as u64, seed)].clone().tidy();
                let at = i * len
                    + if i % 2 == 1 {
                        (swing * len as f64 / 2.0).round() as i64
                    } else {
                        0
                    }
                    + (step.offset * len as f64).round() as i64;
                if step.on {
                    let hits = step.repeats as i64;
                    let share = len / hits;
                    let gate = if hits > 1 {
                        step.gate.min(1.0)
                    } else {
                        step.gate
                    };
                    for j in 0..hits {
                        let start = at + j * share;
                        if start < 0 || start >= until {
                            continue;
                        }
                        let velocity = if step.vary > 0 {
                            let spread = 2 * step.vary as u64 + 1;
                            let by = (mix(lane_seed, (i as u64) << 8 | j as u64) % spread) as i32
                                - step.vary as i32;
                            (step.velocity as i32 + by).clamp(1, 127) as u8
                        } else {
                            step.velocity
                        };
                        let pitch = (lane.pitch as i32 + step.pitch as i32).clamp(0, 127) as u8;
                        let mut n = Note::new(
                            start,
                            ((share as f64 * gate).round() as i64).max(1),
                            pitch,
                            velocity,
                        );
                        n.id = format!("{}.{i}.{j}", lane.id);
                        n.chance = step.chance;
                        out.push(n);
                    }
                }
                i += 1;
            }
        }
        out.sort_by(|a, b| {
            (a.start, a.pitch, a.id.as_str()).cmp(&(b.start, b.pitch, b.id.as_str()))
        });
        out
    }

    /// Which step lane `lane` is on `at` ticks into a clip's content,
    /// going by the grid (not swing or offsets), for showing it play.
    pub fn step_at(&self, lane: usize, at: i64) -> Option<usize> {
        let l = self.lanes.get(lane).filter(|l| !l.steps.is_empty())?;
        (at >= 0).then(|| l.step_at((at / l.rate.ticks()) as u64, hash(self.id.as_bytes())))
    }

    /// A copy under a new id, with new lane ids, named `name`.
    pub fn copy(&self, name: String) -> Pattern {
        Pattern {
            id: crate::new_id(),
            name,
            lanes: (self.lanes.iter())
                .map(|l| PatternLane {
                    id: crate::new_id(),
                    ..l.clone()
                })
                .collect(),
            ..self.clone()
        }
    }
}

/// Makes the notes of every note clip on `tracks` that links to one of
/// `patterns` from its pattern, as far as the clip runs. A clip whose
/// pattern is gone (deleted, or not arrived yet from someone else) plays
/// nothing.
pub fn fill_pattern_clips(patterns: &[Pattern], tracks: &mut [Track]) {
    let by_id: HashMap<&str, &Pattern> = patterns.iter().map(|p| (p.id.as_str(), p)).collect();
    for c in tracks.iter_mut().flat_map(|t| t.note_clips.iter_mut()) {
        fill_clip(&by_id, c);
    }
}

/// Like [`fill_pattern_clips`], for loose clips.
pub fn fill_pattern_clip(patterns: &[Pattern], clip: &mut NoteClip) {
    let by_id: HashMap<&str, &Pattern> = patterns.iter().map(|p| (p.id.as_str(), p)).collect();
    fill_clip(&by_id, clip);
}

fn fill_clip(by_id: &HashMap<&str, &Pattern>, c: &mut NoteClip) {
    let Some(id) = &c.pattern else { return };
    let notes = match by_id.get(id.as_str()) {
        Some(p) => p.notes(c.offset + c.len),
        None => Vec::new(),
    };
    if c.notes != notes {
        c.notes = notes;
    }
}

/// What a note's chance is rolled with as it plays: notes with the same
/// `key` share one roll each time round. Of `of` notes sharing a key
/// with `of` above 1, only the one numbered `pick` plays, chosen at
/// random each time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roll {
    pub key: u64,
    pub pick: u16,
    pub of: u16,
}

impl NoteClip {
    /// The notes that play ([`NoteClip::placed`]), each with what its
    /// chance is rolled with: its own roll, its group's, or a place in
    /// its group's pick of one.
    pub fn rolls(&self) -> Vec<(Note, Roll)> {
        let clip = hash(self.id.as_bytes());
        let placed: Vec<Note> = self.placed().collect();
        let mut counts: HashMap<u32, u16> = HashMap::new();
        for n in &placed {
            if let (Some(g), true) = (n.group, n.one_of) {
                *counts.entry(g).or_default() += 1;
            }
        }
        let mut seen: HashMap<u32, u16> = HashMap::new();
        placed
            .into_iter()
            .map(|n| {
                let roll = match (n.group, n.one_of) {
                    (Some(g), true) => {
                        let k = seen.entry(g).or_default();
                        let pick = *k;
                        *k += 1;
                        Roll {
                            key: mix(clip, 0x0100_0000_0000 | g as u64),
                            pick,
                            of: counts[&g],
                        }
                    }
                    (Some(g), false) => Roll {
                        key: mix(clip, g as u64),
                        pick: 0,
                        of: 1,
                    },
                    (None, _) => Roll {
                        key: mix(clip, hash(n.id.as_bytes())),
                        pick: 0,
                        of: 1,
                    },
                };
                (n, roll)
            })
            .collect()
    }
}

impl Track {
    /// The track's notes as they play, like [`Track::placed_notes`],
    /// each with what its chance is rolled with.
    pub fn played_notes(&self) -> Vec<(Note, Roll)> {
        let mut out: Vec<(Note, Roll)> = (self.note_clips.iter())
            .filter(|c| !c.muted)
            .flat_map(NoteClip::rolls)
            .collect();
        out.sort_by(|(a, _), (b, _)| {
            (a.start, a.pitch, a.id.as_str()).cmp(&(b.start, b.pitch, b.id.as_str()))
        });
        out
    }
}

/// A group number for notes not in one yet: one past the highest in
/// `notes`.
pub fn next_group(notes: &[Note]) -> u32 {
    notes
        .iter()
        .filter_map(|n| n.group)
        .max()
        .map_or(1, |g| g + 1)
}

/// FNV-1a: the same on every computer and every run.
pub fn hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// SplitMix64 of `a` and `b`.
pub fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Writes the patterns. A project that never had one gains no entries.
pub(crate) fn write_patterns(w: &mut Writer, doc: &LoroDoc, patterns: &[Pattern]) {
    if patterns.is_empty() && doc.get_by_str_path("patterns").is_none() {
        return;
    }
    let all = doc.get_map("patterns");
    let keep: HashSet<&str> = patterns.iter().map(|p| p.id.as_str()).collect();
    for id in keys(&all) {
        if !keep.contains(id.as_str()) {
            w.delete(&all, &id);
        }
    }
    for p in patterns {
        let m = w.child(&all, &p.id);
        w.put(&m, "name", p.name.as_str());
        w.put(&m, "len", p.len);
        w.put(&m, "swing", p.swing);
        let lanes = w.fixed(&m, "lanes");
        let keep: HashSet<&str> = p.lanes.iter().map(|l| l.id.as_str()).collect();
        for id in keys(&lanes) {
            if !keep.contains(id.as_str()) {
                w.delete(&lanes, &id);
            }
        }
        for (i, l) in p.lanes.iter().enumerate() {
            let lm = w.child(&lanes, &l.id);
            w.put(&lm, "index", i as i64);
            w.put(&lm, "name", l.name.as_str());
            w.put(&lm, "pitch", l.pitch as i64);
            w.put(&lm, "len", l.steps.len() as i64);
            w.put(&lm, "rate", l.rate.name());
            w.put(&lm, "direction", l.direction.name());
            w.put(&lm, "muted", l.muted);
            let steps = w.fixed(&lm, "steps");
            for k in keys(&steps) {
                if k.parse::<usize>().is_ok_and(|k| k < l.steps.len()) {
                    continue;
                }
                w.delete(&steps, &k);
            }
            let plain = Step::default();
            for (k, s) in l.steps.iter().enumerate() {
                let key = k.to_string();
                // Steps left as they start gain no entries.
                if *s == plain && steps.get(&key).is_none() {
                    continue;
                }
                let sm = w.child(&steps, &key);
                let put = |w: &mut Writer, f: &str, v: LoroValue, unset: bool| {
                    if !(unset && sm.get(f).is_none()) {
                        w.put(&sm, f, v);
                    }
                };
                put(w, "on", s.on.into(), !s.on);
                put(
                    w,
                    "velocity",
                    (s.velocity as i64).into(),
                    s.velocity == plain.velocity,
                );
                put(w, "pitch", (s.pitch as i64).into(), s.pitch == 0);
                put(w, "chance", s.chance.into(), s.chance == 1.0);
                put(w, "repeats", (s.repeats as i64).into(), s.repeats == 1);
                put(w, "offset", s.offset.into(), s.offset == 0.0);
                put(w, "gate", s.gate.into(), s.gate == plain.gate);
                put(w, "vary", (s.vary as i64).into(), s.vary == 0);
            }
        }
    }
}

/// The patterns in the document, oldest name first.
pub(crate) fn read_patterns(root: &loro::LoroMapValue) -> Vec<Pattern> {
    let Some(LoroValue::Map(all)) = root.get("patterns") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, p) in all.iter() {
        let LoroValue::Map(p) = p else { continue };
        let mut pat = Pattern::new(
            id.to_string(),
            string(p, "name").unwrap_or_default(),
            int(p, "len").unwrap_or(PPQ * 4),
        );
        pat.swing = float(p, "swing").map_or(0.0, |s| finite(s, 0.0).clamp(0.0, 1.0));
        let mut lanes: Vec<(i64, PatternLane)> = Vec::new();
        if let Some(LoroValue::Map(ls)) = p.get("lanes") {
            for (lid, l) in ls.iter() {
                let LoroValue::Map(l) = l else { continue };
                let len = int(l, "len").unwrap_or(16).clamp(1, MAX_STEPS as i64) as usize;
                let mut lane = PatternLane::new(
                    lid.to_string(),
                    string(l, "name").unwrap_or_default(),
                    int(l, "pitch").unwrap_or(36).clamp(0, 127) as u8,
                    len,
                );
                lane.rate = string(l, "rate")
                    .and_then(|r| StepRate::from_name(&r))
                    .unwrap_or_default();
                lane.direction = string(l, "direction")
                    .and_then(|d| Direction::from_name(&d))
                    .unwrap_or_default();
                lane.muted = boolean(l, "muted").unwrap_or(false);
                if let Some(LoroValue::Map(ss)) = l.get("steps") {
                    for (k, s) in ss.iter() {
                        let LoroValue::Map(s) = s else { continue };
                        let Some(k) = k.parse::<usize>().ok().filter(|&k| k < len) else {
                            continue;
                        };
                        let d = Step::default();
                        lane.steps[k] = Step {
                            on: boolean(s, "on").unwrap_or(false),
                            velocity: int(s, "velocity")
                                .map_or(d.velocity, |v| v.clamp(1, 127) as u8),
                            pitch: int(s, "pitch").map_or(0, |v| v.clamp(-48, 48) as i8),
                            chance: float(s, "chance").unwrap_or(1.0),
                            repeats: int(s, "repeats")
                                .map_or(1, |v| v.clamp(1, MAX_REPEATS as i64) as u8),
                            offset: float(s, "offset").unwrap_or(0.0),
                            gate: float(s, "gate").unwrap_or(d.gate),
                            vary: int(s, "vary").map_or(0, |v| v.clamp(0, 127) as u8),
                        }
                        .tidy();
                    }
                }
                lanes.push((int(l, "index").unwrap_or(i64::MAX), lane));
            }
        }
        lanes.sort_by(|(a, x), (b, y)| a.cmp(b).then(x.id.cmp(&y.id)));
        pat.lanes = lanes.into_iter().map(|(_, l)| l).collect();
        out.push(pat);
    }
    out.sort_by(|a, b| (a.name.as_str(), a.id.as_str()).cmp(&(b.name.as_str(), b.id.as_str())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Project, ProjectDoc};

    fn lane(steps: usize) -> PatternLane {
        PatternLane::new("l".into(), "Kick".into(), 36, steps)
    }

    fn starts(p: &Pattern, until: i64) -> Vec<i64> {
        p.notes(until).iter().map(|n| n.start).collect()
    }

    #[test]
    fn euclidean_fills_spread_hits_evenly() {
        let s = |v: Vec<bool>| {
            v.iter()
                .map(|&b| if b { 'x' } else { '.' })
                .collect::<String>()
        };
        assert_eq!(s(euclid(4, 16, 0)), "x...x...x...x...");
        assert_eq!(s(euclid(3, 8, 0)), "x..x..x.");
        assert_eq!(s(euclid(3, 8, 1)), ".x..x..x");
        assert_eq!(s(euclid(5, 8, 0)).matches('x').count(), 5);
        assert_eq!(s(euclid(0, 4, 0)), "....");
        assert_eq!(s(euclid(9, 4, 0)), "xxxx");
    }

    #[test]
    fn lanes_cycle_on_their_own_length_and_rate() {
        let mut p = Pattern::new("p".into(), "P".into(), PPQ * 4);
        // A kick every quarter, from four sixteenth steps with the first
        // on; a three-step eighth-triplet lane over it.
        let mut kick = lane(4);
        kick.steps[0].on = true;
        let mut hats = PatternLane::new("h".into(), "Hats".into(), 42, 3);
        hats.rate = StepRate::EighthTriplet;
        hats.steps[0].on = true;
        p.lanes = vec![kick, hats];
        let notes = p.notes(PPQ * 2);
        let kicks: Vec<i64> = (notes.iter())
            .filter(|n| n.pitch == 36)
            .map(|n| n.start)
            .collect();
        let hats: Vec<i64> = (notes.iter())
            .filter(|n| n.pitch == 42)
            .map(|n| n.start)
            .collect();
        assert_eq!(kicks, vec![0, PPQ]);
        assert_eq!(hats, vec![0, PPQ]);
        // Five steps of triplets against four sixteenths drift apart.
        p.lanes[1].set_len(5);
        let hats: Vec<i64> = (p.notes(PPQ * 4).iter())
            .filter(|n| n.pitch == 42)
            .map(|n| n.start)
            .collect();
        assert_eq!(hats, vec![0, PPQ * 5 / 3, PPQ * 10 / 3]);
    }

    #[test]
    fn swing_offset_repeats_and_gate_shape_the_hits() {
        let mut p = Pattern::new("p".into(), "P".into(), PPQ);
        let mut l = lane(4);
        for s in &mut l.steps {
            s.on = true;
        }
        p.lanes = vec![l];
        p.swing = 1.0;
        let six = PPQ / 4;
        assert_eq!(
            starts(&p, PPQ),
            vec![0, six + six / 2, six * 2, six * 3 + six / 2]
        );
        p.swing = 0.0;
        p.lanes[0].steps[2].offset = -0.25;
        assert_eq!(starts(&p, PPQ)[2], six * 2 - six / 4);
        p.lanes[0].steps[2].offset = 0.0;
        p.lanes[0].steps[1].repeats = 3;
        p.lanes[0].steps[1].gate = 1.0;
        let notes = p.notes(PPQ);
        assert_eq!(notes.len(), 6);
        assert_eq!(notes[1].start, six);
        assert_eq!(notes[2].start, six + six / 3);
        assert_eq!(notes[1].len, six / 3);
        p.lanes[0].steps[0].gate = 2.0;
        assert_eq!(
            p.notes(PPQ)[0].len,
            six * 2,
            "a gate past 1 ties into the next step"
        );
    }

    #[test]
    fn directions_pitch_velocity_and_chance() {
        let mut p = Pattern::new("p".into(), "P".into(), PPQ);
        let mut l = lane(3);
        for (i, s) in l.steps.iter_mut().enumerate() {
            s.on = true;
            s.pitch = i as i8;
        }
        l.steps[1].chance = 0.25;
        l.steps[2].velocity = 40;
        p.lanes = vec![l];
        let pitches = |p: &Pattern| {
            p.notes(PPQ / 4 * 6)
                .iter()
                .map(|n| n.pitch - 36)
                .collect::<Vec<_>>()
        };
        assert_eq!(pitches(&p), vec![0, 1, 2, 0, 1, 2]);
        p.lanes[0].direction = Direction::Backward;
        assert_eq!(pitches(&p), vec![2, 1, 0, 2, 1, 0]);
        p.lanes[0].direction = Direction::PingPong;
        assert_eq!(pitches(&p), vec![0, 1, 2, 1, 0, 1]);
        p.lanes[0].direction = Direction::Random;
        let r = pitches(&p);
        assert_eq!(
            r,
            pitches(&p),
            "random is the same each time it is worked out"
        );
        assert!(r.iter().all(|&x| x < 3));
        p.lanes[0].direction = Direction::Forward;
        let notes = p.notes(PPQ / 4 * 3);
        assert_eq!(notes[1].chance, 0.25);
        assert_eq!(notes[2].velocity, 40);
        p.lanes[0].steps[0].vary = 20;
        let v: Vec<u8> = (p.notes(PPQ * 8).iter())
            .filter(|n| n.pitch == 36)
            .map(|n| n.velocity)
            .collect();
        assert!(v.iter().all(|&v| (80..=120).contains(&v)));
        assert!(v.iter().any(|&x| x != v[0]), "the velocity varies");
        p.lanes[0].muted = true;
        assert!(p.notes(PPQ).is_empty());
    }

    #[test]
    fn linked_clips_follow_their_pattern() {
        let mut pat = Pattern::drums("p".into(), "Beat".into(), PPQ * 4);
        let mut t = Track::new("t".into(), "Drums".into());
        let mut a = NoteClip::new("a".into(), "A".into(), 0, PPQ * 4);
        a.pattern = Some("p".into());
        let mut b = NoteClip::new("b".into(), "B".into(), PPQ * 4, PPQ * 8);
        b.pattern = Some("p".into());
        t.note_clips = vec![a, b];
        let mut tracks = vec![t];
        fill_pattern_clips(std::slice::from_ref(&pat), &mut tracks);
        let kicks = |c: &NoteClip| c.notes.iter().filter(|n| n.pitch == 36).count();
        assert_eq!(kicks(&tracks[0].note_clips[0]), 2);
        assert_eq!(
            kicks(&tracks[0].note_clips[1]),
            4,
            "a longer clip goes round again"
        );
        pat.lanes[0].fill(4, 0);
        fill_pattern_clips(std::slice::from_ref(&pat), &mut tracks);
        assert_eq!(
            kicks(&tracks[0].note_clips[0]),
            4,
            "an edit changes every clip"
        );
        assert_eq!(kicks(&tracks[0].note_clips[1]), 8);
        fill_pattern_clips(&[], &mut tracks);
        assert!(tracks[0].note_clips[0].notes.is_empty());
        let copy = pat.copy("Beat 2".into());
        assert_ne!(copy.id, pat.id);
        assert_ne!(copy.lanes[0].id, pat.lanes[0].id);
        assert_eq!(copy.notes(PPQ * 4).len(), pat.notes(PPQ * 4).len());
    }

    #[test]
    fn groups_share_one_roll_and_one_of_picks_a_place() {
        let mut c = NoteClip::new("c".into(), "C".into(), 0, PPQ * 4);
        for (i, p) in [60, 64, 67, 72].into_iter().enumerate() {
            let mut n = Note::new(i as i64, PPQ, p, 100);
            n.group = Some(1);
            n.one_of = i < 3;
            c.notes.push(n);
        }
        c.notes.push(Note::new(0, PPQ, 50, 100));
        let r = c.rolls();
        let of: Vec<(u16, u16)> = (r.iter().filter(|(n, _)| n.one_of))
            .map(|(_, r)| (r.pick, r.of))
            .collect();
        assert_eq!(of, vec![(0, 3), (1, 3), (2, 3)]);
        let keys: HashSet<u64> = (r.iter().filter(|(n, _)| n.one_of))
            .map(|(_, r)| r.key)
            .collect();
        assert_eq!(keys.len(), 1);
        let shared = r.iter().find(|(n, _)| n.pitch == 72).unwrap().1;
        let alone = r.iter().find(|(n, _)| n.pitch == 50).unwrap().1;
        assert_ne!(shared.key, alone.key);
        assert_eq!(next_group(&c.notes), 2);
    }

    #[test]
    fn patterns_round_trip_and_merge_step_by_step() {
        let mut p = Project::default();
        let mut pat = Pattern::drums("p".into(), "Beat".into(), PPQ * 4);
        pat.swing = 0.4;
        pat.lanes[1].rate = StepRate::SixteenthTriplet;
        pat.lanes[1].direction = Direction::PingPong;
        pat.lanes[1].set_len(12);
        pat.lanes[3].muted = true;
        pat.lanes[0].steps[3] = Step {
            on: true,
            velocity: 70,
            pitch: -2,
            chance: 0.5,
            repeats: 3,
            offset: 0.1,
            gate: 0.8,
            vary: 9,
        };
        p.patterns.push(pat);
        let mut t = Track::new("t".into(), "Drums".into());
        let mut c = NoteClip::new("c".into(), "Beat".into(), 0, PPQ * 4);
        c.pattern = Some("p".into());
        t.note_clips.push(c);
        p.tracks.push(t);
        let mut doc = ProjectDoc::new(1);
        doc.set(&p);
        let back = doc.get();
        assert_eq!(back.patterns, p.patterns);
        let clip = &back.tracks[0].note_clips[0];
        assert_eq!(clip.pattern.as_deref(), Some("p"));
        assert!(
            !clip.notes.is_empty(),
            "a pattern clip's notes are made when read"
        );

        // Two people each turn on a different step at once.
        let mut other = ProjectDoc::from_bytes(&doc.to_bytes(), 2).unwrap();
        let mut mine = back.clone();
        mine.patterns[0].lanes[0].steps[5].on = true;
        doc.set(&mine);
        let mut theirs = other.get();
        theirs.patterns[0].lanes[2].steps[1].on = true;
        other.set(&theirs);
        doc.merge(&other.to_bytes()).unwrap();
        let merged = doc.get();
        assert!(merged.patterns[0].lanes[0].steps[5].on);
        assert!(merged.patterns[0].lanes[2].steps[1].on);
    }
}
