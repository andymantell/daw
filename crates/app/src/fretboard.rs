// SPDX-License-Identifier: GPL-3.0-or-later
//! The fretboard pane (Phase 4 plan D4): a guitar or bass neck in any
//! tuning and capo, showing the notes playing now, a scale or mode, a
//! chord shape or its arpeggio, and the notes selected in the note
//! editor. Clicking a fret, or typing on it with the arrows and Enter,
//! enters a note at the playhead. It follows the track the notation
//! commands act on, uses that track's tab tuning when it has one, and
//! shows the song's key unless another is picked.

use daw_engine::engine::Command;
use daw_engine::time::{Frames, Ticks};
use daw_notation::tuning::{Tuning, pitch_class_name, pitch_name};
use daw_project::{NotationView, Note, ROOTS, Scale, ScaleKind};
use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::App;
use crate::commands::Cmd;
use crate::notation::{clear_at, clip_at, placed, tuning_of};

/// What the neck shows besides the notes playing and selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Show {
    /// Only the notes playing and the notes selected.
    #[default]
    Playing,
    Scale,
    /// One shape of the chord, at the hand position.
    Chord,
    /// Every note of the chord.
    Arpeggio,
}

impl Show {
    pub(crate) const ALL: [Show; 4] = [Show::Playing, Show::Scale, Show::Chord, Show::Arpeggio];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Show::Playing => "Notes playing",
            Show::Scale => "Scale or mode",
            Show::Chord => "Chord shape",
            Show::Arpeggio => "Arpeggio",
        }
    }

    fn next(self) -> Show {
        let i = Show::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Show::ALL[(i + 1) % Show::ALL.len()]
    }
}

/// A kind of chord, as the steps from its root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChordKind {
    #[default]
    Major,
    Minor,
    Power,
    Dominant7,
    Major7,
    Minor7,
    HalfDiminished,
    Diminished,
    Diminished7,
    Augmented,
    Sus2,
    Sus4,
    Major6,
    Minor6,
    Add9,
}

impl ChordKind {
    pub(crate) const ALL: [ChordKind; 15] = [
        ChordKind::Major,
        ChordKind::Minor,
        ChordKind::Power,
        ChordKind::Dominant7,
        ChordKind::Major7,
        ChordKind::Minor7,
        ChordKind::HalfDiminished,
        ChordKind::Diminished,
        ChordKind::Diminished7,
        ChordKind::Augmented,
        ChordKind::Sus2,
        ChordKind::Sus4,
        ChordKind::Major6,
        ChordKind::Minor6,
        ChordKind::Add9,
    ];

    /// Semitones above the root of each note of the chord.
    pub(crate) fn steps(self) -> &'static [u8] {
        match self {
            ChordKind::Major => &[0, 4, 7],
            ChordKind::Minor => &[0, 3, 7],
            ChordKind::Power => &[0, 7],
            ChordKind::Dominant7 => &[0, 4, 7, 10],
            ChordKind::Major7 => &[0, 4, 7, 11],
            ChordKind::Minor7 => &[0, 3, 7, 10],
            ChordKind::HalfDiminished => &[0, 3, 6, 10],
            ChordKind::Diminished => &[0, 3, 6],
            ChordKind::Diminished7 => &[0, 3, 6, 9],
            ChordKind::Augmented => &[0, 4, 8],
            ChordKind::Sus2 => &[0, 2, 7],
            ChordKind::Sus4 => &[0, 5, 7],
            ChordKind::Major6 => &[0, 4, 7, 9],
            ChordKind::Minor6 => &[0, 3, 7, 9],
            ChordKind::Add9 => &[0, 4, 7, 14],
        }
    }

    /// Written after the root: "m7" for A minor seventh is "Am7".
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            ChordKind::Major => "",
            ChordKind::Minor => "m",
            ChordKind::Power => "5",
            ChordKind::Dominant7 => "7",
            ChordKind::Major7 => "maj7",
            ChordKind::Minor7 => "m7",
            ChordKind::HalfDiminished => "m7b5",
            ChordKind::Diminished => "dim",
            ChordKind::Diminished7 => "dim7",
            ChordKind::Augmented => "aug",
            ChordKind::Sus2 => "sus2",
            ChordKind::Sus4 => "sus4",
            ChordKind::Major6 => "6",
            ChordKind::Minor6 => "m6",
            ChordKind::Add9 => "add9",
        }
    }

    fn next(self) -> ChordKind {
        let i = ChordKind::ALL.iter().position(|&k| k == self).unwrap_or(0);
        ChordKind::ALL[(i + 1) % ChordKind::ALL.len()]
    }

    /// Whether `pitch` is a note of the chord on `root`.
    fn contains(self, root: u8, pitch: u8) -> bool {
        let step = (pitch as i32 - root as i32).rem_euclid(12) as u8;
        self.steps().iter().any(|s| s % 12 == step)
    }
}

/// What is written in each note's dot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Labels {
    #[default]
    Names,
    /// Steps from the root: 1, b3, 5.
    Degrees,
    Off,
}

impl Labels {
    pub(crate) const ALL: [Labels; 3] = [Labels::Names, Labels::Degrees, Labels::Off];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Labels::Names => "Note names",
            Labels::Degrees => "Steps from the root",
            Labels::Off => "No labels",
        }
    }

    fn next(self) -> Labels {
        let i = Labels::ALL.iter().position(|&l| l == self).unwrap_or(0);
        Labels::ALL[(i + 1) % Labels::ALL.len()]
    }
}

/// The steps from a root, as players name them.
const DEGREES: [&str; 12] = [
    "1", "b2", "2", "b3", "3", "4", "b5", "5", "b6", "6", "b7", "7",
];

/// How many frets a hand position covers.
pub(crate) const SPAN: u8 = 4;

/// The fretboard pane's settings.
pub(crate) struct Fretboard {
    pub(crate) show: Show,
    /// The key shown, over the song's (or the edited clip's).
    pub(crate) own_key: Option<Scale>,
    /// The chord's root, over the key's.
    pub(crate) chord_root: Option<u8>,
    pub(crate) chord: ChordKind,
    pub(crate) labels: Labels,
    /// The tuning (a place in `Tuning::presets`) and capo used when the
    /// track has no tab lane of its own.
    pub(crate) tuning: usize,
    pub(crate) capo: u8,
    /// How many frets the neck has.
    pub(crate) frets: u8,
    /// The lowest fret of the hand position, 0 for the whole neck.
    pub(crate) position: u8,
    /// Typing on the neck: the box's string (1 the highest) and fret
    /// (on the neck, the capo not counted).
    pub(crate) typing: bool,
    pub(crate) cursor: (u8, u8),
    /// Clicking a fret plays its note on the track's instrument.
    pub(crate) hear: bool,
    /// Notes held down on a MIDI keyboard or guitar now, by pitch.
    pub(crate) live: Vec<u8>,
}

impl Default for Fretboard {
    fn default() -> Fretboard {
        Fretboard {
            show: Show::Playing,
            own_key: None,
            chord_root: None,
            chord: ChordKind::Major,
            labels: Labels::Names,
            tuning: 0,
            capo: 0,
            frets: 22,
            position: 0,
            typing: false,
            cursor: (1, 0),
            hear: true,
            live: Vec::new(),
        }
    }
}

/// The neck being shown: its tuning, capo, and where they come from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Neck {
    pub(crate) tuning: Tuning,
    pub(crate) capo: u8,
    pub(crate) frets: u8,
    /// The track whose tab lane gives the tuning.
    pub(crate) from_track: bool,
}

impl Neck {
    /// The pitch at a string (1 the highest) and fret on the neck, if
    /// it can be played: at or above the capo.
    pub(crate) fn pitch(&self, string: u8, fret: u8) -> Option<u8> {
        if fret < self.capo || fret > self.frets {
            return None;
        }
        let open = *self
            .tuning
            .strings
            .get(usize::from(string).checked_sub(1)?)?;
        let p = u16::from(open) + u16::from(fret);
        (p <= 127).then_some(p as u8)
    }

    fn strings(&self) -> u8 {
        self.tuning.string_count() as u8
    }

    /// Where a note is played: its own string and fret if it has them,
    /// else the lowest fret it can be found at (the higher string if
    /// two are as low).
    pub(crate) fn place(&self, n: &Note) -> Option<(u8, u8)> {
        if let (Some(s), Some(f)) = (n.string, n.fret) {
            let at = f.saturating_add(self.capo);
            if self.pitch(s, at) == Some(n.pitch) {
                return Some((s, at));
            }
        }
        (1..=self.strings())
            .filter_map(|s| {
                let open = self.pitch(s, self.capo)?;
                let f = n.pitch.checked_sub(open)?.checked_add(self.capo)?;
                (f <= self.frets).then_some((s, f))
            })
            .min_by_key(|&(s, f)| (f, s))
    }

    /// The frets a hand position covers: from `position` for `SPAN`
    /// frets, with the open strings when the hand is at the first
    /// fret. Position 0 is the whole neck.
    pub(crate) fn window(&self, position: u8) -> (u8, u8) {
        if position == 0 {
            return (self.capo, self.frets);
        }
        let from = position.max(self.capo + 1);
        let low = if from <= self.capo + 1 {
            self.capo
        } else {
            from
        };
        (low, (from + SPAN - 1).min(self.frets))
    }

    /// One shape of a chord in a hand position: a fret for each string
    /// (from string 1), or none where the string isn't played. The root
    /// is the lowest note; strings below it are left out.
    pub(crate) fn voicing(&self, root: u8, kind: ChordKind, position: u8) -> Vec<Option<u8>> {
        let (low, high) = self.window(if position == 0 { 1 } else { position });
        let n = self.strings();
        let mut out = vec![None; usize::from(n)];
        let tones = |s: u8| -> Vec<u8> {
            (low..=high)
                .filter(|&f| self.pitch(s, f).is_some_and(|p| kind.contains(root, p)))
                .collect()
        };
        // The bass: the lowest string with the root in reach.
        let Some((bass, fret)) = (1..=n).rev().find_map(|s| {
            tones(s)
                .into_iter()
                .find(|&f| self.pitch(s, f).is_some_and(|p| p % 12 == root % 12))
                .map(|f| (s, f))
        }) else {
            return out;
        };
        out[usize::from(bass - 1)] = Some(fret);
        let mut have: Vec<u8> = vec![root % 12];
        for s in (1..bass).rev() {
            let frets = tones(s);
            // A note not yet in the chord, else the lowest fret.
            let pick = (frets.iter().copied())
                .find(|&f| self.pitch(s, f).is_some_and(|p| !have.contains(&(p % 12))))
                .or_else(|| frets.first().copied());
            if let Some(f) = pick {
                have.push(self.pitch(s, f).unwrap_or(0) % 12);
                out[usize::from(s - 1)] = Some(f);
            }
        }
        out
    }
}

/// What one place on the neck shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mark {
    /// In the scale, chord or arpeggio shown; and whether it is the root.
    pub(crate) tone: bool,
    pub(crate) root: bool,
    pub(crate) playing: bool,
    /// Held down on a MIDI keyboard or guitar now.
    pub(crate) live: bool,
    pub(crate) selected: bool,
}

/// Everything the pane draws, worked out from the app.
pub(crate) struct View {
    pub(crate) neck: Neck,
    /// By string (from 1) then fret on the neck.
    pub(crate) marks: Vec<Vec<Mark>>,
    /// The root that steps are counted from.
    pub(crate) root: u8,
    /// Such as "A minor pentatonic" or "Am7".
    pub(crate) title: String,
    pub(crate) track: Option<usize>,
    /// The notes playing, as "E4 (string 1, fret 0)".
    pub(crate) playing: Vec<String>,
}

impl View {
    pub(crate) fn mark(&self, string: u8, fret: u8) -> Mark {
        (self.marks.get(usize::from(string).wrapping_sub(1)))
            .and_then(|r| r.get(usize::from(fret)))
            .copied()
            .unwrap_or_default()
    }
}

impl App {
    /// The neck the fretboard shows: the track's tab tuning, or its own.
    pub(crate) fn fretboard_neck(&self) -> Neck {
        let fb = &self.fretboard;
        let lane = (self.notation_track())
            .and_then(|i| self.tracks[i].notation.as_ref())
            .filter(|n| n.view == NotationView::Tab && !n.tuning.is_empty());
        let (tuning, capo, from_track) = match lane {
            Some(n) => (tuning_of(n), n.capo, true),
            None => {
                let presets = Tuning::presets();
                (presets[fb.tuning % presets.len()].clone(), fb.capo, false)
            }
        };
        Neck {
            tuning,
            capo: capo.min(fb.frets.saturating_sub(SPAN)),
            frets: fb.frets,
            from_track,
        }
    }

    /// The key shown: the fretboard's own, else the clip's or the
    /// song's, else C major.
    pub(crate) fn fretboard_key(&self) -> (Scale, bool) {
        match self.fretboard.own_key.or_else(|| self.editing_scale()) {
            Some(s) => (s, true),
            None => (Scale::new(0, ScaleKind::Major), false),
        }
    }

    fn fretboard_chord_root(&self) -> u8 {
        (self.fretboard.chord_root).unwrap_or(self.fretboard_key().0.root) % 12
    }

    /// What the fretboard shows now.
    pub(crate) fn fretboard_view(&self) -> View {
        let fb = &self.fretboard;
        let neck = self.fretboard_neck();
        let n = usize::from(neck.strings());
        let mut marks = vec![vec![Mark::default(); usize::from(neck.frets) + 1]; n];
        let (key, _) = self.fretboard_key();
        let chord_root = self.fretboard_chord_root();
        let (low, high) = neck.window(fb.position);
        let (root, title) = match fb.show {
            Show::Playing | Show::Scale => (key.root, key.label()),
            Show::Chord | Show::Arpeggio => (
                chord_root,
                format!("{}{}", ROOTS[usize::from(chord_root)], fb.chord.symbol()),
            ),
        };
        match fb.show {
            Show::Playing => {}
            Show::Scale | Show::Arpeggio => {
                for s in 1..=neck.strings() {
                    for f in low..=high {
                        let Some(p) = neck.pitch(s, f) else { continue };
                        let tone = match fb.show {
                            Show::Scale => key.contains(p),
                            _ => fb.chord.contains(chord_root, p),
                        };
                        let m = &mut marks[usize::from(s - 1)][usize::from(f)];
                        m.tone = tone;
                        m.root = tone && p % 12 == root;
                    }
                }
            }
            Show::Chord => {
                for (i, f) in neck
                    .voicing(chord_root, fb.chord, fb.position)
                    .iter()
                    .enumerate()
                {
                    if let Some(f) = *f {
                        let m = &mut marks[i][usize::from(f)];
                        m.tone = true;
                        m.root = neck.pitch(i as u8 + 1, f).is_some_and(|p| p % 12 == root);
                    }
                }
            }
        }
        let track = self.notation_track();
        let mut playing = Vec::new();
        if let Some(t) = track.and_then(|i| self.tracks.get(i)) {
            let map = self.tempo_map();
            let now = map
                .ticks_at(Frames(self.ctl.status().position().0.max(0)))
                .0;
            for note in placed(&t.note_clips) {
                if note.start > now || note.start + note.len <= now {
                    continue;
                }
                if let Some((s, f)) = neck.place(&note) {
                    marks[usize::from(s - 1)][usize::from(f)].playing = true;
                    playing.push(format!(
                        "{} (string {s}, fret {})",
                        pitch_name(note.pitch),
                        f - neck.capo
                    ));
                }
            }
            if let Some(clip) = self.editing_clip()
                && t.note_clips.iter().any(|c| c.id == clip.id)
            {
                for note in clip
                    .notes
                    .iter()
                    .filter(|n| self.notes_editor.selected.contains(&n.id))
                {
                    if let Some((s, f)) = neck.place(note) {
                        marks[usize::from(s - 1)][usize::from(f)].selected = true;
                    }
                }
            }
        }
        for &p in &self.fretboard.live {
            if let Some((s, f)) = neck.place(&Note::new(0, 1, p, 1)) {
                marks[usize::from(s - 1)][usize::from(f)].live = true;
                playing.push(format!(
                    "{} (string {s}, fret {}, played live)",
                    pitch_name(p),
                    f - neck.capo
                ));
            }
        }
        View {
            neck,
            marks,
            root,
            title,
            track,
            playing,
        }
    }

    /// What the fretboard shows, for assistants (roadmap 2.11).
    pub(crate) fn fretboard_value(&self) -> serde_json::Value {
        let v = self.fretboard_view();
        let neck = &v.neck;
        let places = |pick: fn(&Mark) -> bool| -> Vec<serde_json::Value> {
            let mut out = Vec::new();
            for s in 1..=neck.strings() {
                for f in neck.capo..=neck.frets {
                    let m = v.mark(s, f);
                    if pick(&m) {
                        let p = neck.pitch(s, f).unwrap_or(0);
                        out.push(serde_json::json!({
                            "string": s,
                            "fret": f - neck.capo,
                            "pitch": p,
                            "name": pitch_name(p),
                            "root": m.root,
                        }));
                    }
                }
            }
            out
        };
        let fb = &self.fretboard;
        serde_json::json!({
            "open": self.workspace.layout.is_open(crate::workspace::Pane::Fretboard),
            "track": v.track.and_then(|i| self.tracks.get(i)).map(|t| t.key.clone()),
            "tuning": neck.tuning.name,
            "strings": neck.tuning.strings,
            "capo": neck.capo,
            "tuning_from_track": neck.from_track,
            "frets": neck.frets,
            "shows": fb.show.label(),
            "title": v.title,
            "position": fb.position,
            "labels": fb.labels.label(),
            "shown": places(|m| m.tone),
            "playing": places(|m| m.playing),
            "selected": places(|m| m.selected),
            "typing": fb.typing.then(|| serde_json::json!({
                "string": fb.cursor.0,
                "fret": fb.cursor.1.saturating_sub(neck.capo),
            })),
        })
    }

    /// Enters the note at a string and fret on the neck, at the
    /// playhead on the fretboard's track, a grid step long. While
    /// stopped, the playhead then moves on by the step, so the next
    /// note follows.
    pub(crate) fn fretboard_put(&mut self, string: u8, fret: u8) -> bool {
        let Some(track) = self.notation_track() else {
            self.message =
                Some("Add an instrument track first: the fretboard writes its notes.".into());
            return false;
        };
        let neck = self.fretboard_neck();
        let Some(pitch) = neck.pitch(string, fret) else {
            self.message = Some(format!(
                "Fret {fret} can't be played on string {string} with the capo at {}.",
                neck.capo
            ));
            return false;
        };
        let map = self.tempo_map();
        let playing = self.ctl.status().playing();
        let at = map
            .ticks_at(Frames(self.ctl.status().position().0.max(0)))
            .0;
        let step = self.notes_editor.grid.ticks(&map, at).max(1);
        let bar = map.bar_start(map.bar_beat(Ticks(at)).bar).0;
        let tick = (bar + ((at - bar) as f64 / step as f64).round() as i64 * step).max(0);
        let t = &mut self.tracks[track];
        clear_at(&mut t.note_clips, tick, string);
        let clip = clip_at(&mut t.note_clips, &map, tick);
        let mut note = Note::new(tick - clip.place(0), step, pitch, 96);
        note.string = Some(string);
        note.fret = Some(fret - neck.capo);
        clip.notes.push(note);
        clip.len = clip.len.max(tick + step - clip.start);
        clip.sort_notes();
        self.rebuild();
        if self.fretboard.hear {
            self.send(Command::Audition {
                track,
                pitch,
                velocity: 96,
                frames: i64::from(self.rate) * 6 / 10,
            });
        }
        if !playing {
            let next = map.frames_at(Ticks(tick + step));
            self.send(Command::Seek(next));
        }
        true
    }

    /// Runs one of the fretboard's commands.
    pub(crate) fn fretboard_command(&mut self, cmd: Cmd) {
        let neck = self.fretboard_neck();
        let fb = &mut self.fretboard;
        match cmd {
            Cmd::FretboardView => fb.show = fb.show.next(),
            Cmd::FretboardRootUp | Cmd::FretboardRootDown => {
                let by = if cmd == Cmd::FretboardRootUp { 1 } else { 11 };
                match self.fretboard.show {
                    Show::Chord | Show::Arpeggio => {
                        let r = self.fretboard_chord_root();
                        self.fretboard.chord_root = Some((r + by) % 12);
                    }
                    _ => {
                        let (k, _) = self.fretboard_key();
                        self.fretboard.own_key = Some(Scale::new(k.root + by, k.kind));
                        if self.fretboard.show == Show::Playing {
                            self.fretboard.show = Show::Scale;
                        }
                    }
                }
            }
            Cmd::FretboardScale => {
                let (k, _) = self.fretboard_key();
                let i = ScaleKind::ALL
                    .iter()
                    .position(|&s| s == k.kind)
                    .unwrap_or(0);
                let kind = ScaleKind::ALL[(i + 1) % ScaleKind::ALL.len()];
                self.fretboard.own_key = Some(Scale::new(k.root, kind));
                self.fretboard.show = Show::Scale;
            }
            // Shows the chord first, then goes through the kinds.
            Cmd::FretboardChord => match fb.show {
                Show::Chord | Show::Arpeggio => fb.chord = fb.chord.next(),
                _ => fb.show = Show::Chord,
            },
            Cmd::FretboardSongKey => {
                fb.own_key = None;
                fb.chord_root = None;
            }
            Cmd::FretboardLabels => fb.labels = fb.labels.next(),
            Cmd::FretboardHear => fb.hear = !fb.hear,
            Cmd::FretboardPositionUp => {
                fb.position = (fb.position.max(neck.capo) + 1).min(neck.frets + 1 - SPAN);
            }
            Cmd::FretboardPositionDown => {
                fb.position = if fb.position <= neck.capo + 1 {
                    0
                } else {
                    fb.position - 1
                };
            }
            Cmd::FretboardTuning => {
                let presets = Tuning::presets();
                let at = presets
                    .iter()
                    .position(|t| t.strings == neck.tuning.strings);
                let next = at.map_or(0, |i| (i + 1) % presets.len());
                self.set_fretboard_tuning(&presets[next]);
            }
            Cmd::FretboardCapoUp | Cmd::FretboardCapoDown => {
                let capo = if cmd == Cmd::FretboardCapoUp {
                    (neck.capo + 1).min(neck.frets - SPAN)
                } else {
                    neck.capo.saturating_sub(1)
                };
                self.set_fretboard_capo(capo);
            }
            Cmd::FretboardType => {
                fb.typing = !fb.typing;
                if fb.typing {
                    fb.cursor = (1, neck.capo.max(fb.position));
                }
            }
            Cmd::FretLeft | Cmd::FretRight | Cmd::FretUp | Cmd::FretDown => {
                fb.typing = true;
                let (s, f) = fb.cursor;
                fb.cursor = match cmd {
                    Cmd::FretLeft => (s, f.saturating_sub(1).max(neck.capo)),
                    Cmd::FretRight => (s, (f + 1).min(neck.frets)),
                    Cmd::FretUp => (s.saturating_sub(1).max(1), f),
                    _ => ((s + 1).min(neck.strings()), f),
                };
            }
            Cmd::FretEnter => {
                let (s, f) = fb.cursor;
                fb.typing = true;
                self.fretboard_put(s, f);
            }
            _ => {}
        }
    }

    /// Keeps up with the notes held down on MIDI devices, from what the
    /// engine heard.
    pub(crate) fn fretboard_hear(&mut self, captured: &[daw_engine::midi::Captured]) {
        use daw_engine::midi::Kind;
        let live = &mut self.fretboard.live;
        for c in captured {
            match c.event.kind {
                Kind::NoteOn {
                    pitch, velocity, ..
                } if velocity > 0 => {
                    if !live.contains(&pitch) && live.len() < 16 {
                        live.push(pitch);
                    }
                }
                Kind::NoteOn { pitch, .. } | Kind::NoteOff { pitch, .. } => {
                    live.retain(|&p| p != pitch);
                }
                Kind::Control {
                    number: 120 | 123, ..
                } => live.clear(),
                _ => {}
            }
        }
    }

    /// Changes the tuning: the track's tab lane's, if it has one, else
    /// the fretboard's own.
    pub(crate) fn set_fretboard_tuning(&mut self, t: &Tuning) {
        if self.fretboard_neck().from_track
            && let Some(i) = self.notation_track()
            && let Some(mut n) = self.tracks[i].notation.clone()
        {
            n.tuning = t.strings.clone();
            n.tuning_name = t.name.clone();
            self.set_notation(i, Some(n));
        } else {
            let presets = Tuning::presets();
            self.fretboard.tuning = presets.iter().position(|p| p == t).unwrap_or(0);
        }
        let strings = t.string_count().max(1) as u8;
        self.fretboard.cursor.0 = self.fretboard.cursor.0.min(strings);
    }

    pub(crate) fn set_fretboard_capo(&mut self, capo: u8) {
        if self.fretboard_neck().from_track
            && let Some(i) = self.notation_track()
            && let Some(mut n) = self.tracks[i].notation.clone()
        {
            n.capo = capo;
            self.set_notation(i, Some(n));
        } else {
            self.fretboard.capo = capo;
        }
        let fb = &mut self.fretboard;
        fb.cursor.1 = fb.cursor.1.max(capo);
    }

    /// While typing on the neck: the arrows move the box, Enter enters
    /// its note, and Escape stops. Taken before the command keys.
    pub(crate) fn fretboard_keys(&mut self, ctx: &egui::Context) {
        if !self.fretboard.typing
            || self.notation.entry.is_some()
            || ctx.egui_wants_keyboard_input()
            || !self
                .workspace
                .layout
                .is_open(crate::workspace::Pane::Fretboard)
        {
            return;
        }
        use egui::{Key, Modifiers};
        let mut cmds = Vec::new();
        ctx.input_mut(|i| {
            for (k, c) in [
                (Key::ArrowLeft, Cmd::FretLeft),
                (Key::ArrowRight, Cmd::FretRight),
                (Key::ArrowUp, Cmd::FretUp),
                (Key::ArrowDown, Cmd::FretDown),
                (Key::Enter, Cmd::FretEnter),
            ] {
                if i.consume_key(Modifiers::NONE, k) {
                    cmds.push(c);
                }
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                cmds.push(Cmd::FretboardType);
            }
        });
        for c in cmds {
            self.fretboard_command(c);
        }
    }

    /// The fretboard pane.
    pub(crate) fn fretboard_pane(&mut self, ui: &mut egui::Ui) {
        self.fretboard_bar(ui);
        let view = self.fretboard_view();
        if view.track.is_none() {
            ui.label(
                "No instrument track yet. Add one to see its notes here and to enter notes \
                 by clicking the frets.",
            );
        }
        let clicked = draw_neck(ui, &view, &self.fretboard);
        if let Some((s, f)) = clicked {
            self.fretboard.cursor = (s, f);
            self.fretboard_put(s, f);
        }
        if self.ctl.status().playing() {
            ui.ctx().request_repaint();
        }
    }

    fn fretboard_bar(&mut self, ui: &mut egui::Ui) {
        let neck = self.fretboard_neck();
        let (key, song_key) = self.fretboard_key();
        let mut run = None;
        ui.horizontal_wrapped(|ui| {
            // What's shown.
            let mut show = self.fretboard.show;
            egui::ComboBox::from_id_salt("fretboard-show")
                .selected_text(show.label())
                .show_ui(ui, |ui| {
                    for s in Show::ALL {
                        ui.selectable_value(&mut show, s, s.label());
                    }
                })
                .response
                .on_hover_text(self.commands.hover(Cmd::FretboardView))
                .accessible_name("What the fretboard shows");
            self.fretboard.show = show;
            let chordy = matches!(show, Show::Chord | Show::Arpeggio);
            // The root.
            let mut root = if chordy { self.fretboard_chord_root() } else { key.root };
            egui::ComboBox::from_id_salt("fretboard-root")
                .selected_text(ROOTS[usize::from(root)])
                .width(48.0)
                .show_ui(ui, |ui| {
                    for (i, r) in ROOTS.iter().enumerate() {
                        ui.selectable_value(&mut root, i as u8, *r);
                    }
                })
                .response
                .accessible_name("Root");
            if chordy {
                if root != self.fretboard_chord_root() {
                    self.fretboard.chord_root = Some(root);
                }
                let mut kind = self.fretboard.chord;
                egui::ComboBox::from_id_salt("fretboard-chord")
                    .selected_text(format!("{}{}", ROOTS[usize::from(root)], kind.symbol()))
                    .show_ui(ui, |ui| {
                        for k in ChordKind::ALL {
                            let name = format!("{}{}", ROOTS[usize::from(root)], k.symbol());
                            ui.selectable_value(&mut kind, k, name);
                        }
                    })
                    .response
                    .accessible_name("Chord");
                self.fretboard.chord = kind;
            } else {
                let mut kind = key.kind;
                egui::ComboBox::from_id_salt("fretboard-scale")
                    .selected_text(kind.label())
                    .show_ui(ui, |ui| {
                        for k in ScaleKind::ALL {
                            ui.selectable_value(&mut kind, k, k.label());
                        }
                    })
                    .response
                    .accessible_name("Scale or mode");
                if (root, kind) != (key.root, key.kind) {
                    self.fretboard.own_key = Some(Scale::new(root, kind));
                    if show == Show::Playing {
                        self.fretboard.show = Show::Scale;
                    }
                }
            }
            let own = self.fretboard.own_key.is_some() || self.fretboard.chord_root.is_some();
            if ui
                .add_enabled(own, egui::Button::new("Song's key"))
                .on_hover_text(if song_key {
                    self.commands.hover(Cmd::FretboardSongKey)
                } else {
                    "The song has no key set (Notes → Scale), so the fretboard starts on C major"
                        .to_string()
                })
                .clicked()
            {
                run = Some(Cmd::FretboardSongKey);
            }
            ui.separator();
            // The neck.
            let presets = Tuning::presets();
            let mut pick = None;
            egui::ComboBox::from_id_salt("fretboard-tuning")
                .selected_text(format!("{} ({})", neck.tuning.name, neck.tuning.letters()))
                .show_ui(ui, |ui| {
                    for t in &presets {
                        let on = t.strings == neck.tuning.strings;
                        if ui
                            .selectable_label(on, format!("{} ({})", t.name, t.letters()))
                            .clicked()
                        {
                            pick = Some(t.clone());
                        }
                    }
                })
                .response
                .on_hover_text(if neck.from_track {
                    "The track's tab lane's tuning: changing it here changes the lane's"
                } else {
                    "The tuning the fretboard shows (the track has no tab lane)"
                })
                .accessible_name("Tuning");
            if let Some(t) = pick {
                self.set_fretboard_tuning(&t);
            }
            ui.label("Capo");
            let mut capo = neck.capo;
            if ui
                .add(egui::DragValue::new(&mut capo).range(0..=neck.frets - SPAN))
                .on_hover_text("The fret the capo is on, 0 for none")
                .changed()
            {
                self.set_fretboard_capo(capo);
            }
            ui.label("Position");
            let mut pos = self.fretboard.position;
            ui.add(
                egui::DragValue::new(&mut pos)
                    .range(0..=neck.frets + 1 - SPAN)
                    .custom_formatter(|v, _| {
                        if v < 0.5 {
                            "whole neck".into()
                        } else {
                            format!("fret {v}")
                        }
                    }),
            )
            .on_hover_text("Where the hand is: scales and arpeggios show only these four frets, and the chord's shape is found here");
            self.fretboard.position = pos;
            let mut labels = self.fretboard.labels;
            egui::ComboBox::from_id_salt("fretboard-labels")
                .selected_text(labels.label())
                .show_ui(ui, |ui| {
                    for l in Labels::ALL {
                        ui.selectable_value(&mut labels, l, l.label());
                    }
                })
                .response
                .accessible_name("Labels");
            self.fretboard.labels = labels;
            if ui
                .selectable_label(self.fretboard.typing, "Type on it")
                .on_hover_text(self.commands.hover(Cmd::FretboardType))
                .clicked()
            {
                run = Some(Cmd::FretboardType);
            }
            if ui
                .selectable_label(self.fretboard.hear, "Hear notes")
                .on_hover_text(self.commands.hover(Cmd::FretboardHear))
                .clicked()
            {
                run = Some(Cmd::FretboardHear);
            }
        });
        if let Some(c) = run {
            self.fretboard_command(c);
        }
    }
}

/// The neck's width for each fret: as a real neck's, narrower up it,
/// but not so narrow that the high frets can't be clicked.
fn fret_edges(left: f32, width: f32, frets: u8) -> Vec<f32> {
    let real = |f: f32| 1.0 - 2f32.powf(-f / 12.0);
    let total = real(f32::from(frets));
    (0..=frets)
        .map(|f| {
            let f = f32::from(f);
            let k = 0.6 * real(f) / total + 0.4 * f / f32::from(frets.max(1));
            left + width * k
        })
        .collect()
}

/// Draws the neck; says which string and fret was clicked.
fn draw_neck(ui: &mut egui::Ui, view: &View, fb: &Fretboard) -> Option<(u8, u8)> {
    let look = crate::theme::look(ui.ctx());
    let c = look.c();
    let neck = &view.neck;
    let strings = neck.strings().max(1);
    let avail = ui.available_size();
    let row = ((avail.y - 40.0) / f32::from(strings)).clamp(22.0, 32.0);
    let height = row * f32::from(strings) + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(avail.x.max(300.0), height), Sense::click());
    let summary = if view.playing.is_empty() {
        format!("Fretboard: {}, {}", neck.tuning.name, view.title)
    } else {
        format!("Fretboard: playing {}", view.playing.join(", "))
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, &summary));
    let painter = ui.painter_at(rect);
    let open_w = 44.0;
    let board = Rect::from_min_max(
        pos2(rect.left() + open_w, rect.top() + 4.0),
        pos2(
            rect.right() - 8.0,
            rect.top() + row * f32::from(strings) + 4.0,
        ),
    );
    let edges = fret_edges(board.left(), board.width(), neck.frets);
    let y_of = |s: u8| board.top() + row * (f32::from(s) - 0.5);
    // The cell a fret's note sits in: open strings left of the nut.
    let x_of = |f: u8| {
        if f == 0 {
            rect.left() + open_w * 0.62
        } else {
            (edges[usize::from(f - 1)] + edges[usize::from(f)]) / 2.0
        }
    };
    painter.rect_filled(board, 2.0, crate::theme::mix(c.panel, c.raised, 0.5));
    // The hand position, shaded.
    if fb.position > 0 {
        let (lo, hi) = neck.window(fb.position);
        let x0 = if lo == 0 {
            rect.left()
        } else {
            edges[usize::from(lo - 1)]
        };
        let r = Rect::from_x_y_ranges(x0..=edges[usize::from(hi)], board.y_range());
        painter.rect_filled(r, 0.0, c.selection.gamma_multiply(0.5));
    }
    // Inlays.
    for f in [3u8, 5, 7, 9, 12, 15, 17, 19, 21, 24] {
        if f > neck.frets {
            continue;
        }
        let x = x_of(f);
        let dot = c.line.gamma_multiply(0.8);
        if f % 12 == 0 {
            painter.circle_filled(pos2(x, board.top() + board.height() * 0.3), 4.0, dot);
            painter.circle_filled(pos2(x, board.top() + board.height() * 0.7), 4.0, dot);
        } else {
            painter.circle_filled(pos2(x, board.center().y), 4.0, dot);
        }
        painter.text(
            pos2(x, board.bottom() + 4.0),
            Align2::CENTER_TOP,
            f.to_string(),
            FontId::proportional(11.0),
            c.text_dim,
        );
    }
    // Frets, the nut, and the capo.
    for (f, x) in edges.iter().enumerate() {
        let w = if f == 0 { 4.0 } else { 1.5 };
        painter.line_segment(
            [pos2(*x, board.top()), pos2(*x, board.bottom())],
            Stroke::new(
                w,
                if f == 0 {
                    c.text
                } else {
                    c.text_dim.gamma_multiply(0.7)
                },
            ),
        );
    }
    if neck.capo > 0 {
        let x = edges[usize::from(neck.capo - 1)] + 3.0;
        let r = Rect::from_center_size(pos2(x, board.center().y), vec2(7.0, board.height() + 6.0));
        painter.rect_filled(r, 3.0, c.text_dim);
        // Frets below the capo can't be played.
        let under = Rect::from_x_y_ranges(rect.left()..=x - 4.0, board.y_range());
        painter.rect_filled(under, 0.0, c.bg.gamma_multiply(0.6));
    }
    // Strings, thicker lower down, with their open note's name.
    for s in 1..=strings {
        let y = y_of(s);
        let w = 1.0 + 1.5 * f32::from(s - 1) / f32::from(strings.max(2) - 1);
        painter.line_segment(
            [pos2(rect.left() + 16.0, y), pos2(board.right(), y)],
            Stroke::new(w, c.text_dim),
        );
        if let Some(p) = neck.pitch(s, neck.capo) {
            painter.text(
                pos2(rect.left() + 2.0, y),
                Align2::LEFT_CENTER,
                pitch_class_name(p),
                FontId::proportional(10.0),
                c.text_dim,
            );
        }
    }
    // The notes.
    let radius = (row * 0.4).min(11.0);
    for s in 1..=strings {
        for f in 0..=neck.frets {
            let m = view.mark(s, f);
            if !(m.tone || m.playing || m.live || m.selected) {
                continue;
            }
            let at = pos2(x_of(f), y_of(s));
            let (fill, ink) = if m.live {
                (c.record, c.on_accent)
            } else if m.playing {
                (c.good, c.bg)
            } else if m.root {
                (c.accent, c.on_accent)
            } else if m.tone {
                (c.text_dim, c.bg)
            } else {
                (c.panel, c.text)
            };
            // Notes playing stand out a little bigger.
            let radius = if m.playing || m.live {
                radius + 1.5
            } else {
                radius
            };
            painter.circle_filled(at, radius, fill);
            if m.selected {
                painter.circle_stroke(at, radius + 2.5, Stroke::new(2.0, c.glow));
            }
            let label = neck.pitch(s, f).and_then(|p| match fb.labels {
                Labels::Names => Some(pitch_class_name(p).to_string()),
                Labels::Degrees => {
                    Some(DEGREES[usize::from((p + 12 - view.root) % 12)].to_string())
                }
                Labels::Off => None,
            });
            if let Some(l) = label {
                painter.text(
                    at,
                    Align2::CENTER_CENTER,
                    l,
                    FontId::proportional(radius * 0.95),
                    ink,
                );
            }
        }
    }
    // The typing box.
    if fb.typing {
        let (s, f) = fb.cursor;
        let r = Rect::from_center_size(pos2(x_of(f), y_of(s)), vec2(radius * 2.6, row));
        painter.rect_stroke(r, 3.0, Stroke::new(2.0, c.accent), StrokeKind::Inside);
    }
    // Where the pointer is, and a click.
    let hit = |p: Pos2| -> Option<(u8, u8)> {
        if !board.y_range().contains(p.y) || p.x < rect.left() || p.x > board.right() {
            return None;
        }
        let s = (((p.y - board.top()) / row) as u8 + 1).min(strings);
        let f = if p.x < board.left() {
            0
        } else {
            edges.iter().position(|&x| x >= p.x).unwrap_or(edges.len()) as u8
        };
        Some((s, f.min(neck.frets)))
    };
    let mut clicked = None;
    if let Some(p) = resp.hover_pos()
        && let Some((s, f)) = hit(p)
        && let Some(pitch) = neck.pitch(s, f)
    {
        let r = Rect::from_center_size(pos2(x_of(f), y_of(s)), vec2(radius * 2.2, row));
        painter.rect_stroke(r, 3.0, Stroke::new(1.0, c.text_dim), StrokeKind::Inside);
        let fret = f - neck.capo;
        let resp = resp.clone().on_hover_text(format!(
            "{} · string {s}, fret {fret}. Click to enter it at the playhead",
            pitch_name(pitch)
        ));
        if resp.clicked() {
            clicked = Some((s, f));
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_audio_io::SilentInput;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::Audio;
    use crate::workspace::Pane;

    fn standard() -> Neck {
        Neck {
            tuning: Tuning::guitar_standard(),
            capo: 0,
            frets: 22,
            from_track: false,
        }
    }

    #[test]
    fn notes_are_found_on_the_neck() {
        let neck = standard();
        assert_eq!(neck.pitch(1, 0), Some(64));
        assert_eq!(neck.pitch(6, 5), Some(45));
        assert_eq!(neck.pitch(7, 0), None, "six strings");
        // A4 is lowest at the 5th fret of the top string.
        assert_eq!(neck.place(&Note::new(0, 1, 69, 90)), Some((1, 5)));
        // A note's own string is kept.
        let mut n = Note::new(0, 1, 69, 90);
        (n.string, n.fret) = (Some(2), Some(10));
        assert_eq!(neck.place(&n), Some((2, 10)));
        // With a capo, frets are counted from it.
        let capo = Neck {
            capo: 2,
            ..standard()
        };
        assert_eq!(capo.pitch(1, 1), None, "under the capo");
        let mut n = Note::new(0, 1, 66, 90);
        (n.string, n.fret) = (Some(1), Some(0));
        assert_eq!(capo.place(&n), Some((1, 2)));
        assert_eq!(capo.place(&Note::new(0, 1, 30, 90)), None, "too low");
    }

    #[test]
    fn chord_shapes_start_on_the_root_in_the_hand_position() {
        let neck = standard();
        // C major in the first position: x32010.
        let c = neck.voicing(0, ChordKind::Major, 1);
        assert_eq!(c, [Some(0), Some(1), Some(0), Some(2), Some(3), None]);
        // A minor at the 5th fret: the E-shape barre, 577555.
        let am = neck.voicing(9, ChordKind::Minor, 5);
        assert_eq!(am, [Some(5), Some(5), Some(5), Some(7), Some(7), Some(5)]);
        for (i, f) in am.iter().enumerate() {
            let p = neck.pitch(i as u8 + 1, f.unwrap()).unwrap();
            assert!(ChordKind::Minor.contains(9, p));
        }
        // The window covers four frets, and the open strings at the first.
        assert_eq!(neck.window(1), (0, 4));
        assert_eq!(neck.window(5), (5, 8));
        assert_eq!(neck.window(0), (0, 22));
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        let app = h.state_mut();
        let eng = app.replace_engine();
        app.audio = Audio::Silent(
            daw_audio_io::start_silent_with(app.rate, 256, eng, SilentInput::None),
            String::new(),
        );
        h.run();
        h
    }

    #[test]
    fn commands_change_the_view_and_enter_notes() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddInstrumentTrack);
        h.state_mut().run(&ctx, Cmd::Fretboard);
        h.run();
        assert!(h.state().workspace.layout.is_open(Pane::Fretboard));
        // The song's key is shown as a scale.
        h.state_mut().scale = Some(Scale::new(9, ScaleKind::MinorPentatonic));
        h.state_mut().run(&ctx, Cmd::FretboardView);
        let v = h.state().fretboard_view();
        assert_eq!(v.title, "A minor pentatonic");
        assert!(v.mark(6, 5).root, "A on the low E's 5th fret");
        assert!(v.mark(6, 8).tone && !v.mark(6, 7).tone);
        // A chord, its root moved up.
        h.state_mut().run(&ctx, Cmd::FretboardChord);
        h.state_mut().run(&ctx, Cmd::FretboardRootUp);
        let v = h.state().fretboard_view();
        assert_eq!(v.title, "A#");
        h.state_mut().run(&ctx, Cmd::FretboardSongKey);
        assert_eq!(h.state().fretboard_view().title, "A");
        // A capo and a tuning.
        h.state_mut().run(&ctx, Cmd::FretboardCapoUp);
        h.state_mut().run(&ctx, Cmd::FretboardTuning);
        let neck = h.state().fretboard_neck();
        assert_eq!((neck.capo, neck.tuning.name.as_str()), (1, "Drop D"));
        // Typing: the box moves and Enter writes the note it is on.
        h.state_mut().run(&ctx, Cmd::FretboardType);
        assert_eq!(h.state().fretboard.cursor, (1, 1));
        h.state_mut().run(&ctx, Cmd::FretRight);
        h.state_mut().run(&ctx, Cmd::FretRight);
        h.state_mut().run(&ctx, Cmd::FretDown);
        h.state_mut().run(&ctx, Cmd::FretEnter);
        h.run();
        let notes = placed(&h.state().tracks[0].note_clips);
        assert_eq!(notes.len(), 1);
        // B string, 3rd fret on the neck, 2nd from the capo: D4.
        assert_eq!(
            (
                notes[0].start,
                notes[0].pitch,
                notes[0].string,
                notes[0].fret
            ),
            (0, 62, Some(2), Some(2))
        );
        // The playhead moved on by a grid step, so it isn't playing now;
        // back at the start it is.
        h.state_mut().send(Command::Seek(Frames(0)));
        // Wait for the engine to move the playhead there, which can take
        // a moment on a slow runner.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !h.state().fretboard_view().mark(2, 3).playing && std::time::Instant::now() < end {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let v = h.state().fretboard_view();
        assert!(v.mark(2, 3).playing, "the note under the playhead");
        assert_eq!(v.playing, ["D4 (string 2, fret 2)"]);
    }

    #[test]
    fn an_assistant_reads_the_fretboard_and_enters_notes_by_command() {
        use crate::agent::driver::Driver;
        use serde_json::json;
        let mut d = Driver::new();
        d.command("add-instrument-track");
        d.command("fretboard");
        for id in [
            "fretboard-view",
            "fretboard-root-down",
            "fretboard-root-down",
            "fretboard-root-down",
        ] {
            d.command(id);
        }
        let fb = d.call("get_fretboard", json!({})).unwrap();
        assert_eq!(fb["open"], true);
        assert_eq!(fb["title"], "A major");
        assert_eq!(fb["tuning"], "Standard");
        // The low E's 5th fret is the root.
        let shown = fb["shown"].as_array().unwrap();
        assert!(
            shown
                .iter()
                .any(|p| p["string"] == 6 && p["fret"] == 5 && p["root"] == true)
        );
        // Type the G string's 2nd fret, an A.
        d.command("fretboard-type");
        for id in [
            "fret-down",
            "fret-down",
            "fret-right",
            "fret-right",
            "fret-enter",
        ] {
            d.command(id);
        }
        let p = d.call("get_project", json!({"notes": true})).unwrap();
        let note = &p["tracks"][0]["note_clips"][0]["notes"][0];
        assert_eq!(note["pitch"], 57, "{p}");
        d.call("seek", json!({"seconds": 0.0})).unwrap();
        d.h.run();
        let fb = d.call("get_fretboard", json!({})).unwrap();
        assert_eq!(fb["playing"][0]["name"], "A3", "{fb}");
        assert_eq!(
            (
                fb["playing"][0]["string"].clone(),
                fb["playing"][0]["fret"].clone()
            ),
            (json!(3), json!(2))
        );
    }

    #[test]
    fn a_fret_entered_is_heard_and_live_notes_show() {
        use daw_engine::midi::{Captured, Event};
        let mut h = harness();
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddInstrumentTrack);
        h.state_mut().run(&ctx, Cmd::Fretboard);
        h.run();
        // Stopped, so only the audition can make a sound.
        h.state_mut().run(&ctx, Cmd::FretEnter);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while h.state().master_meter.0 < 0.001 && std::time::Instant::now() < end {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(h.state().master_meter.0 >= 0.001, "the note is heard");
        // And stops of its own accord.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while h.state().master_meter.0 > 0.0001 && std::time::Instant::now() < end {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(h.state().master_meter.0 <= 0.0001, "the note ends");
        // A note held on a MIDI keyboard shows; let go, it doesn't.
        let held = |on: bool| Captured {
            port: 0,
            event: if on {
                Event::note_on(0, 0, 69, 100, 1)
            } else {
                Event::note_off(0, 0, 69, 1)
            },
            at: 0,
            playing: false,
            recording: false,
        };
        h.state_mut().fretboard_hear(&[held(true)]);
        let v = h.state().fretboard_view();
        assert!(v.mark(1, 5).live, "A4 at the top string's 5th fret");
        assert_eq!(
            v.playing.last().unwrap(),
            "A4 (string 1, fret 5, played live)"
        );
        h.state_mut().fretboard_hear(&[held(false)]);
        assert!(!h.state().fretboard_view().mark(1, 5).live);
    }

    #[test]
    fn clicking_a_fret_enters_its_note() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddInstrumentTrack);
        h.state_mut().run(&ctx, Cmd::Fretboard);
        h.state_mut().workspace.full = Some(Pane::Fretboard);
        h.run();
        let neck = h.get_by_label_contains("Fretboard:").rect();
        // The top string's open note, left of the nut.
        let at = pos2(neck.left() + 27.0, neck.top() + 4.0 + 10.0);
        h.input_mut().events.push(egui::Event::PointerMoved(at));
        h.step();
        for pressed in [true, false] {
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
        h.run();
        let notes = placed(&h.state().tracks[0].note_clips);
        assert_eq!(notes.len(), 1, "a click enters one note");
        assert_eq!(
            (notes[0].pitch, notes[0].string, notes[0].fret),
            (64, Some(1), Some(0))
        );
    }
}
