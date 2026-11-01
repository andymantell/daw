// SPDX-License-Identifier: GPL-3.0-or-later
//! Pitch editing (plan B2): note-by-note editing of a single voice or a
//! single-note line. The notes found in a clip's audio are drawn as
//! blobs over its pitch line; each can be moved in pitch and time, made
//! longer or shorter, have its drift (the slow wander off the note) and
//! vibrato kept, reduced or (vibrato) deepened, and its formants moved.
//!
//! Nothing is rendered to a file. The edits become a pitch and formant
//! curve ([`PitchBend`]) and, for moves in time, a warp inside the clip;
//! the engine plays the clip through the stretcher along both, so an
//! edit is heard at once and can always be taken back.
//!
//! The notes and their edits are saved with the clip. The pitch line
//! they are drawn over is worked out again from the audio when a
//! project opens, by the same detector ([`daw_engine::pitch_line`]).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};

use daw_engine::bend::{PitchBend, PitchLine};
use daw_engine::pitch_line;
use daw_engine::warp::{Warp, WarpPoint};
use daw_project::{MAX_TRANSPOSE_CENTS, MAX_VIBRATO_PCT, PitchNote, Scale};

use crate::App;
use crate::commands::Cmd;
use crate::timeline::Clip;

/// The steepest a move in time may stretch or squeeze the audio either
/// side of a note, or the note itself, either way.
pub(crate) const MAX_NOTE_STRETCH: f64 = 3.0;
/// The shortest a note, or the gap between two that don't touch, can
/// be made, in seconds.
pub(crate) const MIN_SECONDS: f64 = 0.02;
/// How long an edit fades in and out at a note's ends, in seconds, so
/// the pitch doesn't jump.
const EDGE_SECONDS: f64 = 0.03;
/// The smoothing that takes the vibrato out of a pitch line, leaving
/// the drift: about one vibrato cycle.
const DRIFT_SECONDS: f64 = 0.15;

/// The pitch editor's state.
#[derive(Default)]
pub(crate) struct PitchEdit {
    /// Pitch lines worked out, by audio file.
    pub lines: HashMap<PathBuf, Arc<PitchLine>>,
    /// Lines being worked out.
    jobs: Vec<Job>,
    /// The notes selected in the clip shown, by index.
    pub selected: BTreeSet<usize>,
    /// The clip the selection belongs to.
    pub clip: Option<u64>,
    /// A drag in progress: the notes as they were when it began, where
    /// it began, and what it moves.
    drag: Option<Drag>,
    /// The snap commands leave notes this close (in cents) to where
    /// they would go.
    pub min_cents: i64,
}

struct Drag {
    from: Pos2,
    before: Vec<PitchNote>,
    what: DragWhat,
}

#[derive(Clone, Copy, PartialEq)]
enum DragWhat {
    /// Pitch and time.
    Move,
    /// The note's end.
    Length,
}

/// A pitch line on its way, for the clips playing `path`; when `find`
/// is set, notes are found for that clip too.
struct Job {
    path: PathBuf,
    find: Option<u64>,
    rx: mpsc::Receiver<Result<Found, String>>,
}

/// What working out a clip's audio gives.
pub(crate) struct Found {
    pub line: PitchLine,
    pub notes: Vec<pitch_line::LineNote>,
}

/// Works out the pitch line of `c`'s audio and the notes in it. Takes a
/// moment for a long take.
pub(crate) fn find(c: &Clip) -> Result<Found, String> {
    let samples = c.source.read_mono(c.source.frames)?;
    let line = pitch_line::line(&samples, c.source.rate);
    let notes = pitch_line::notes(&line, &samples);
    Ok(Found { line, notes })
}

/// A pitch in cents above MIDI 0 as people read it: "A4 −12 ct".
pub(crate) fn note_name(cents: i64) -> String {
    const NAMES: [&str; 12] = [
        "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
    ];
    let midi = (cents as f64 / 100.0).round() as i64;
    let off = cents - midi * 100;
    let name = format!(
        "{}{}",
        NAMES[midi.rem_euclid(12) as usize],
        midi.div_euclid(12) - 1
    );
    match off {
        0 => name,
        o if o < 0 => format!("{name} −{} ct", -o),
        o => format!("{name} +{o} ct"),
    }
}

/// `notes` found in a clip's audio, as pitch notes inside `lo..hi`,
/// keeping the pitch, drift, vibrato and formant edits of `old` notes
/// that cover most of each new one.
pub(crate) fn notes_from(
    found: &[pitch_line::LineNote],
    lo: i64,
    hi: i64,
    old: &[PitchNote],
) -> Vec<PitchNote> {
    found
        .iter()
        .filter_map(|n| {
            let (start, end) = (n.start.max(lo), n.end.min(hi));
            (end > start).then(|| {
                let mut p = PitchNote::found(start, end, (n.midi as f64 * 100.0).round() as i64);
                let overlap = |o: &PitchNote| (o.end.min(end) - o.start.max(start)).max(0);
                if let Some(o) = old.iter().max_by_key(|o| overlap(o))
                    && overlap(o) * 2 > end - start
                {
                    p.shift = o.shift;
                    p.drift = o.drift;
                    p.vibrato = o.vibrato;
                    p.formant = o.formant;
                }
                p
            })
        })
        .collect()
}

impl Clip {
    /// The clip's audio frames: what its notes are kept within.
    fn note_range(&self) -> (i64, i64) {
        (self.offset as i64, (self.offset + self.len) as i64)
    }

    /// Its pitch line, when worked out at its rate.
    pub(crate) fn line(&self) -> Option<&PitchLine> {
        self.pitch_line
            .as_deref()
            .filter(|l| l.rate == self.source.rate && l.hop > 0)
    }

    /// Whether any note is moved in pitch, formants, drift or vibrato.
    fn has_pitch_edits(&self) -> bool {
        self.pitch_notes
            .iter()
            .any(|n| n.shift != 0 || n.formant != 0 || n.drift != 100 || n.vibrato != 100)
    }

    /// The pitch and formant curve its edited notes make, or `None`
    /// when no note is moved in pitch.
    pub(crate) fn pitch_bend(&self) -> Option<PitchBend> {
        if !self.has_pitch_edits() {
            return None;
        }
        let rate = self.source.rate as f64;
        let line = self.line();
        let hop = line.map_or((rate / 100.0) as i64, |l| l.hop as i64).max(1);
        let (lo, hi) = self.note_range();
        let first = lo.div_euclid(hop) * hop;
        let n = ((hi - first) / hop + 2) as usize;
        let mut semis = vec![0.0f32; n];
        let mut formants = vec![0.0f32; n];
        let edge = ((EDGE_SECONDS * rate) / hop as f64).max(1.0);
        for note in &self.pitch_notes {
            let (a, b) = (note.start.max(lo), note.end.min(hi));
            if b <= a {
                continue;
            }
            let (i0, i1) = (
                ((a - first) as f64 / hop as f64).ceil() as usize,
                (((b - first) as f64 / hop as f64).floor() as usize).min(n - 1),
            );
            if i1 < i0 {
                continue;
            }
            let centre = note.cents as f32 / 100.0;
            let shift = note.shift as f32 / 100.0;
            let formant = note.formant as f32 / 100.0;
            let drift_k = note.drift as f32 / 100.0 - 1.0;
            let vib_k = note.vibrato as f32 / 100.0 - 1.0;
            // The note's line, if it has one: as sung, and smoothed to
            // the drift alone.
            let parts = line
                .filter(|_| drift_k != 0.0 || vib_k != 0.0)
                .and_then(|l| note_line(l, a, b, first, hop, i0, i1));
            for i in i0..=i1 {
                let k = i - i0;
                // Fade the edit in and out at the note's ends.
                let w = ((k as f64 + 1.0) / edge)
                    .min((i1 - i + 1) as f64 / edge)
                    .min(1.0) as f32;
                let mut s = shift;
                if let Some((raw, slow)) = &parts {
                    s += drift_k * (slow[k] - centre) + vib_k * (raw[k] - slow[k]);
                }
                semis[i] = s * w;
                formants[i] = formant * w;
            }
        }
        Some(PitchBend {
            first,
            hop: hop as u32,
            semitones: semis,
            formants,
        })
    }

    /// The warp inside the clip that moves its notes in time: asset
    /// frames as they play now tied to asset frames as recorded, or
    /// `None` when no note is moved or resized.
    pub(crate) fn note_warp(&self) -> Option<Warp> {
        if !self
            .pitch_notes
            .iter()
            .any(|n| n.moved != 0 || n.resized != 0)
        {
            return None;
        }
        warp_of(&self.pitch_notes, self.note_range())
    }
}

/// The line under hops `i0..=i1` of a note from `a` to `b`, as sung and
/// smoothed, gaps filled from the nearest reading.
fn note_line(
    l: &PitchLine,
    a: i64,
    b: i64,
    first: i64,
    hop: i64,
    i0: usize,
    i1: usize,
) -> Option<(Vec<f32>, Vec<f32>)> {
    let mut raw: Vec<f32> = (i0..=i1)
        .map(|i| {
            let f = (first + i as i64 * hop).clamp(a, b);
            l.get(l.hop_of(f)).unwrap_or(f32::NAN)
        })
        .collect();
    let any = raw.iter().copied().find(|v| v.is_finite())?;
    // Fill forwards, then backwards from the first reading.
    let mut last = any;
    for v in &mut raw {
        if v.is_finite() {
            last = *v;
        } else {
            *v = last;
        }
    }
    let half = ((DRIFT_SECONDS * l.rate as f64 / l.hop as f64) / 2.0).round() as usize;
    let slow = pitch_line::smoothed(&raw, half.max(1));
    Some((raw, slow))
}

/// The warp `notes` make inside `lo..hi`: each note's ends tied from
/// where they were sung to where they play now, the clip's ends kept.
fn warp_of(notes: &[PitchNote], (lo, hi): (i64, i64)) -> Option<Warp> {
    let mut points = vec![WarpPoint { src: lo, out: lo }];
    let mut push = |src: i64, out: i64, edited: bool| {
        let last = points.last_mut().expect("starts with one");
        if src == last.src {
            // Two notes that touch: the one moved has its way.
            if edited {
                last.out = out;
            }
            return;
        }
        points.push(WarpPoint { src, out });
    };
    for n in notes {
        if n.start <= lo || n.end >= hi {
            continue;
        }
        let (a, b) = n.span();
        let edited = n.moved != 0 || n.resized != 0;
        push(n.start, a, edited);
        push(n.end, b, edited);
    }
    push(hi, hi, false);
    let ok = points.windows(2).all(|w| {
        let (ds, dout) = (w[1].src - w[0].src, w[1].out - w[0].out);
        ds > 0
            && dout > 0
            && (1.0 / MAX_NOTE_STRETCH..=MAX_NOTE_STRETCH).contains(&(ds as f64 / dout as f64))
    });
    if !ok {
        return None;
    }
    Warp::new(points).ok()
}

/// Moves note `i` of `notes` by `moved` and makes it `resized` longer,
/// as far towards that as the audio around it can be stretched, within
/// `range`. True if it changed.
pub(crate) fn retime(
    notes: &mut [PitchNote],
    i: usize,
    moved: i64,
    resized: i64,
    range: (i64, i64),
    min_len: i64,
) -> bool {
    let Some(&was) = notes.get(i) else {
        return false;
    };
    let len = was.end - was.start;
    let resized = resized.max(min_len - len);
    let fits = |notes: &mut [PitchNote], m: i64, r: i64| {
        notes[i].moved = m;
        notes[i].resized = r;
        let (a, b) = notes[i].span();
        a > range.0 && b < range.1 && warp_of(notes, range).is_some()
    };
    if fits(notes, moved, resized) {
        return notes[i] != was;
    }
    // As far as it goes: halve the way back from what was asked.
    let (mut good, mut bad) = (0.0f64, 1.0f64);
    for _ in 0..20 {
        let t = (good + bad) / 2.0;
        let m = was.moved + ((moved - was.moved) as f64 * t).round() as i64;
        let r = was.resized + ((resized - was.resized) as f64 * t).round() as i64;
        if fits(notes, m, r) {
            good = t;
        } else {
            bad = t;
        }
    }
    let m = was.moved + ((moved - was.moved) as f64 * good).round() as i64;
    let r = was.resized + ((resized - was.resized) as f64 * good).round() as i64;
    if !fits(notes, m, r) {
        notes[i].moved = was.moved;
        notes[i].resized = was.resized;
    }
    notes[i] != was
}

/// Holds a pitch or formant move within an octave either way.
pub(crate) fn clamp_cents(c: i64) -> i64 {
    c.clamp(-MAX_TRANSPOSE_CENTS, MAX_TRANSPOSE_CENTS)
}

/// Where a snap puts a note: on the nearest whole note, or on the
/// nearest note of a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapTo {
    Semitone,
    Key(Scale),
}

/// The note of `scale` nearest `heard` (in cents above MIDI 0), in
/// cents; between two equally near, the lower.
fn nearest_in_key(scale: Scale, heard: i64) -> i64 {
    let below = heard.div_euclid(100);
    ((below - 6)..=(below + 7))
        .filter(|m| (0..=127).contains(m) && scale.contains(*m as u8))
        .map(|m| m * 100)
        .min_by_key(|m| (m - heard).abs())
        .unwrap_or(heard)
}

/// The shift that snaps a note found at `cents`, moved by `shift` and
/// played `transpose` cents higher by its clip's transpose, onto the
/// nearest note `to` allows, judged as it is heard. `None` when it is
/// already within `min_cents` of that note, and so left alone.
pub(crate) fn snap_shift(
    cents: i64,
    shift: i64,
    transpose: i64,
    to: SnapTo,
    min_cents: i64,
) -> Option<i64> {
    let heard = cents + shift + transpose;
    let target = match to {
        SnapTo::Semitone => (heard as f64 / 100.0).round() as i64 * 100,
        SnapTo::Key(s) => nearest_in_key(s, heard),
    };
    ((target - heard).abs() > min_cents).then(|| clamp_cents(target - transpose - cents))
}

impl App {
    /// The clip the pitch editor shows: the first selected audio clip,
    /// by lane and index.
    pub(crate) fn pitch_clip(&self) -> Option<(usize, usize)> {
        let sel = &self.timeline.selected;
        self.tracks.iter().enumerate().find_map(|(l, t)| {
            t.clips
                .iter()
                .position(|c| sel.contains(&c.id))
                .map(|k| (l, k))
        })
    }

    /// The notes a pitch command acts on in the shown clip: those
    /// selected in the editor, or all when none are.
    fn pitch_targets(&self, c: &Clip) -> Vec<usize> {
        let sel: Vec<usize> = (self.pitch.clip == Some(c.id))
            .then(|| self.pitch.selected.iter().copied())
            .into_iter()
            .flatten()
            .filter(|&i| i < c.pitch_notes.len())
            .collect();
        if sel.is_empty() {
            (0..c.pitch_notes.len()).collect()
        } else {
            sel
        }
    }

    /// Does `f` to each targeted note of the shown clip, then rebuilds
    /// if any changed.
    fn each_pitch_note(&mut self, mut f: impl FnMut(&mut PitchNote) -> bool) {
        if !self.may_edit_song() {
            return;
        }
        let Some((l, k)) = self.pitch_clip() else {
            return;
        };
        let targets = self.pitch_targets(&self.tracks[l].clips[k]);
        let notes = &mut self.tracks[l].clips[k].pitch_notes;
        let mut changed = false;
        for i in targets {
            changed |= f(&mut notes[i]);
        }
        if changed {
            self.rebuild();
        }
    }

    /// Starts finding the notes of the shown clip, in the background.
    pub(crate) fn start_finding_notes(&mut self) {
        if !self.may_edit_song() {
            return;
        }
        let Some((l, k)) = self.pitch_clip() else {
            self.message = Some("Select an audio clip to find its notes.".into());
            return;
        };
        let c = self.tracks[l].clips[k].clone();
        self.start_line_job(&c, Some(c.id));
    }

    fn start_line_job(&mut self, c: &Clip, find_for: Option<u64>) {
        if self
            .pitch
            .jobs
            .iter()
            .any(|j| j.path == c.path && (find_for.is_none() || j.find == find_for))
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let clip = c.clone();
        std::thread::spawn(move || {
            let _ = tx.send(find(&clip));
        });
        self.pitch.jobs.push(Job {
            path: c.path.clone(),
            find: find_for,
            rx,
        });
    }

    /// Whether notes are being found.
    pub(crate) fn finding_notes(&self) -> bool {
        self.pitch.jobs.iter().any(|j| j.find.is_some())
    }

    /// Finds the notes of clip `id` now, replacing those it had (their
    /// pitch edits stay with the new notes that cover them). The number
    /// found, or why not.
    pub(crate) fn find_notes_now(&mut self, id: u64) -> Result<usize, String> {
        let c = (self.tracks.iter().flat_map(|t| &t.clips))
            .find(|c| c.id == id)
            .ok_or("There is no such clip.")?
            .clone();
        let found = find(&c)?;
        Ok(self.take_found(id, &c.path, found))
    }

    /// Puts what was found for the clips playing `path` in place, and
    /// the notes on clip `id`; how many notes it has.
    fn take_found(&mut self, id: u64, path: &std::path::Path, found: Found) -> usize {
        let line = Arc::new(found.line);
        self.pitch.lines.insert(path.to_path_buf(), line.clone());
        let mut n = 0;
        for c in self.tracks.iter_mut().flat_map(|t| &mut t.clips) {
            if c.path == path {
                c.pitch_line = Some(line.clone());
            }
            if c.id == id {
                let (lo, hi) = c.note_range();
                c.pitch_notes = notes_from(&found.notes, lo, hi, &c.pitch_notes);
                n = c.pitch_notes.len();
            }
        }
        if self.pitch.clip == Some(id) {
            self.pitch.selected.clear();
        }
        self.rebuild();
        n
    }

    /// Works out the pitch line of clip `id` now if it has notes and no
    /// line yet, so drift and vibrato edits have one to work from.
    pub(crate) fn ensure_line_now(&mut self, id: u64) -> Result<(), String> {
        let Some(c) = (self.tracks.iter().flat_map(|t| &t.clips)).find(|c| c.id == id) else {
            return Err("There is no such clip.".into());
        };
        if c.line().is_some() {
            return Ok(());
        }
        if let Some(line) = self.pitch.lines.get(&c.path).cloned() {
            let path = c.path.clone();
            self.give_line(&path, line);
            return Ok(());
        }
        let c = c.clone();
        let found = find(&c)?;
        let line = Arc::new(found.line);
        self.pitch.lines.insert(c.path.clone(), line.clone());
        self.give_line(&c.path, line);
        Ok(())
    }

    fn give_line(&mut self, path: &std::path::Path, line: Arc<PitchLine>) {
        for c in self.tracks.iter_mut().flat_map(|t| &mut t.clips) {
            if c.path == path {
                c.pitch_line = Some(line.clone());
            }
        }
    }

    /// Each frame: takes in finished jobs, and starts lines for clips
    /// with notes but none yet (as when a project has just opened).
    pub(crate) fn pitch_tick(&mut self, ctx: &egui::Context) {
        let mut done = Vec::new();
        self.pitch.jobs.retain(|j| match j.rx.try_recv() {
            Ok(r) => {
                done.push((j.path.clone(), j.find, r));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        let mut changed = false;
        for (path, find, r) in done {
            match r {
                Ok(found) => match find {
                    Some(id) => {
                        self.take_found(id, &path, found);
                    }
                    None => {
                        let line = Arc::new(found.line);
                        self.pitch.lines.insert(path.clone(), line.clone());
                        self.give_line(&path, line);
                        changed = true;
                    }
                },
                Err(e) => self.message = Some(format!("Couldn't find the notes: {e}")),
            }
        }
        // Lines for clips that need one.
        let wanting: Vec<Clip> = (self.tracks.iter().flat_map(|t| &t.clips))
            .filter(|c| !c.pitch_notes.is_empty() && c.line().is_none())
            .cloned()
            .collect();
        for c in wanting {
            if let Some(line) = self.pitch.lines.get(&c.path).cloned() {
                self.give_line(&c.path, line);
                changed = true;
            } else {
                self.start_line_job(&c, None);
            }
        }
        if changed {
            // Drift and vibrato edits play once the line is there.
            self.rebuild();
        }
        if !self.pitch.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Runs a pitch-editing command; false if `cmd` isn't one.
    pub(crate) fn run_pitch(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::FindPitchNotes => self.start_finding_notes(),
            Cmd::PitchNotesUp => self.each_pitch_note(|n| {
                let s = clamp_cents(n.shift + 100);
                std::mem::replace(&mut n.shift, s) != s
            }),
            Cmd::PitchNotesDown => self.each_pitch_note(|n| {
                let s = clamp_cents(n.shift - 100);
                std::mem::replace(&mut n.shift, s) != s
            }),
            Cmd::SnapPitchNotes => self.snap_pitch_notes(SnapTo::Semitone),
            Cmd::SnapPitchNotesToKey => match self.scale {
                Some(s) => self.snap_pitch_notes(SnapTo::Key(s)),
                None => return true,
            },
            Cmd::FlattenPitchDrift => self.each_pitch_note(|n| {
                let was = n.drift;
                n.drift = 0;
                was != 0
            }),
            Cmd::ResetPitchNotes => self.each_pitch_note(|n| {
                let was = *n;
                *n = PitchNote::found(n.start, n.end, n.cents);
                was != *n
            }),
            _ => return false,
        }
        true
    }

    /// Snaps the chosen notes of the shown clip (or all) `to` the
    /// nearest whole note or note of the key, as heard, leaving those
    /// within the pane's threshold.
    fn snap_pitch_notes(&mut self, to: SnapTo) {
        let Some((l, k)) = self.pitch_clip() else {
            return;
        };
        let transpose = self.tracks[l].clips[k].cents as i64;
        let min = self.pitch.min_cents;
        self.each_pitch_note(|n| match snap_shift(n.cents, n.shift, transpose, to, min) {
            Some(s) => std::mem::replace(&mut n.shift, s) != s,
            None => false,
        });
    }

    /// Whether a pitch command has notes to act on.
    pub(crate) fn has_pitch_notes(&self) -> bool {
        self.pitch_clip()
            .is_some_and(|(l, k)| !self.tracks[l].clips[k].pitch_notes.is_empty())
    }

    /// The pitch editor pane.
    pub(crate) fn pitch_editor_pane(&mut self, ui: &mut egui::Ui) {
        let Some((l, k)) = self.pitch_clip() else {
            ui.label(
                "Select an audio clip of a single voice or a single-note line, then find its \
                 notes to edit them one by one.",
            );
            return;
        };
        let id = self.tracks[l].clips[k].id;
        if self.pitch.clip != Some(id) {
            self.pitch.clip = Some(id);
            self.pitch.selected.clear();
            self.pitch.drag = None;
        }
        let may_edit = self.may_edit_song();
        let mut run = None;
        let mut min_cents = self.pitch.min_cents;
        ui.horizontal_wrapped(|ui| {
            let c = &self.tracks[l].clips[k];
            ui.label(RichText::new(c.name()).strong());
            ui.add_enabled_ui(may_edit && !self.finding_notes(), |ui| {
                let label = if c.pitch_notes.is_empty() {
                    "Find notes"
                } else {
                    "Find notes again"
                };
                // ai: find-pitch-notes
                if ui
                    .button(label)
                    .on_hover_text(self.commands.hover(Cmd::FindPitchNotes))
                    .clicked()
                {
                    run = Some(Cmd::FindPitchNotes);
                }
            });
            if self.finding_notes() {
                ui.spinner();
                ui.label("Finding notes…");
            }
            ui.add_enabled_ui(may_edit && !c.pitch_notes.is_empty(), |ui| {
                // ai: snap-pitch-notes-to-key
                let key = ui.add_enabled_ui(self.scale.is_some(), |ui| {
                    ui.button("Snap to key").on_hover_text(match self.scale {
                        Some(s) => format!(
                            "{} ({})",
                            self.commands.hover(Cmd::SnapPitchNotesToKey),
                            s.label()
                        ),
                        None => "Set the song's key first".to_string(),
                    })
                });
                if key.inner.clicked() {
                    run = Some(Cmd::SnapPitchNotesToKey);
                }
                ui.label("leave within");
                // ai: edit_pitch_notes
                ui.add(
                    egui::DragValue::new(&mut min_cents)
                        .range(0..=100)
                        .suffix(" ct"),
                )
                .on_hover_text("Snapping leaves notes this close to where they would go");
                for (cmd, text) in [
                    (Cmd::SnapPitchNotes, "Snap to notes"),
                    (Cmd::PitchNotesUp, "Up a semitone"),
                    (Cmd::PitchNotesDown, "Down a semitone"),
                    (Cmd::FlattenPitchDrift, "Flatten drift"),
                    (Cmd::ResetPitchNotes, "As sung"),
                ] {
                    // ai: snap-pitch-notes pitch-notes-up pitch-notes-down flatten-pitch-drift reset-pitch-notes
                    if ui
                        .button(text)
                        .on_hover_text(self.commands.hover(cmd))
                        .clicked()
                    {
                        run = Some(cmd);
                    }
                }
            });
        });
        self.pitch.min_cents = min_cents;
        if let Some(cmd) = run {
            self.run_pitch(cmd);
        }
        self.pitch_note_controls(ui, l, k, may_edit);
        self.pitch_canvas(ui, l, k, may_edit);
    }

    /// Sliders for the selected notes (or all): pitch, drift, vibrato
    /// and formants.
    fn pitch_note_controls(&mut self, ui: &mut egui::Ui, l: usize, k: usize, may_edit: bool) {
        let c = &self.tracks[l].clips[k];
        if c.pitch_notes.is_empty() {
            return;
        }
        let targets = self.pitch_targets(c);
        let first = c.pitch_notes[targets[0]];
        let what = if self.pitch.selected.is_empty() {
            "All notes".to_string()
        } else if targets.len() == 1 {
            format!(
                "Note {}: {}",
                targets[0] + 1,
                note_name(first.cents + first.shift)
            )
        } else {
            format!("{} notes", targets.len())
        };
        let (mut shift, mut drift, mut vib, mut formant) = (
            first.shift as f64 / 100.0,
            first.drift as f64,
            first.vibrato as f64,
            first.formant as f64 / 100.0,
        );
        let mut changed = (false, false, false, false);
        ui.add_enabled_ui(may_edit, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(what).weak());
                let p = ui.label("Pitch");
                // ai: edit_pitch_notes
                changed.0 = ui
                    .add(
                        egui::DragValue::new(&mut shift)
                            .range(-12.0..=12.0)
                            .speed(0.01)
                            .fixed_decimals(2)
                            .suffix(" st"),
                    )
                    .labelled_by(p.id)
                    .on_hover_text("How far the notes are moved, in semitones (0.01 is a cent)")
                    .changed();
                // ai: edit_pitch_notes
                changed.1 = ui
                    .add(
                        egui::Slider::new(&mut drift, 0.0..=100.0)
                            .suffix(" %")
                            .text("Drift kept"),
                    )
                    .on_hover_text(
                        "How much of the slow wander off the note stays: 0 holds it flat",
                    )
                    .changed();
                // ai: edit_pitch_notes
                changed.2 = ui
                    .add(
                        egui::Slider::new(&mut vib, 0.0..=MAX_VIBRATO_PCT as f64)
                            .suffix(" %")
                            .text("Vibrato"),
                    )
                    .on_hover_text("How much vibrato: 0 takes it out, over 100 deepens it")
                    .changed();
                let f = ui.label("Formants");
                // ai: edit_pitch_notes
                changed.3 = ui
                    .add(
                        egui::DragValue::new(&mut formant)
                            .range(-12.0..=12.0)
                            .speed(0.05)
                            .fixed_decimals(1)
                            .suffix(" st"),
                    )
                    .labelled_by(f.id)
                    .on_hover_text("Moves the voice's character: down sounds bigger, up smaller")
                    .changed();
            });
        });
        if changed != (false, false, false, false) {
            let shift = clamp_cents((shift * 100.0).round() as i64);
            let formant = clamp_cents((formant * 100.0).round() as i64);
            let (drift, vib) = (drift.round() as i64, vib.round() as i64);
            let id = self.tracks[l].clips[k].id;
            if changed.1 || changed.2 {
                let _ = self.ensure_line_now(id);
            }
            self.each_pitch_note(|n| {
                let was = *n;
                if changed.0 {
                    n.shift = shift;
                }
                if changed.1 {
                    n.drift = drift.clamp(0, 100);
                }
                if changed.2 {
                    n.vibrato = vib.clamp(0, MAX_VIBRATO_PCT);
                }
                if changed.3 {
                    n.formant = formant;
                }
                was != *n
            });
        }
    }

    /// The notes as blobs over the pitch line: click to choose, drag to
    /// move in pitch (by semitones; Alt for cents) and time, drag a
    /// note's end to make it longer or shorter.
    fn pitch_canvas(&mut self, ui: &mut egui::Ui, l: usize, k: usize, may_edit: bool) {
        let c = &self.tracks[l].clips[k];
        let (lo, hi) = c.note_range();
        let size = vec2(
            ui.available_width().max(200.0),
            ui.available_height().max(240.0),
        );
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let resp = resp.on_hover_text(
            "Click a note to choose it (Shift for more). Drag it up or down to move its pitch \
             (hold Alt for cents) and sideways to move it in time; drag its right end to make it \
             longer or shorter.",
        );
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
        if c.pitch_notes.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No notes found yet",
                egui::FontId::proportional(14.0),
                visuals.weak_text_color(),
            );
            return;
        }
        // The pitches shown: the notes', with room around.
        let (mut top, mut bottom) = (f32::MIN, f32::MAX);
        for n in &c.pitch_notes {
            for v in [n.cents, n.cents + n.shift] {
                top = top.max(v as f32 / 100.0);
                bottom = bottom.min(v as f32 / 100.0);
            }
        }
        let mid = (top + bottom) / 2.0;
        let span = (top - bottom + 4.0).max(12.0);
        let (top, bottom) = (mid + span / 2.0, mid - span / 2.0);
        let x_of = |f: i64| rect.left() + (f - lo) as f32 / (hi - lo).max(1) as f32 * rect.width();
        let y_of = |m: f32| rect.top() + (top - m) / (top - bottom) * rect.height();
        let row = rect.height() / (top - bottom);
        // Semitone rows: the black keys shaded, each C named.
        let mut m = bottom.floor() as i32;
        while (m as f32) <= top {
            let y = y_of(m as f32);
            if [1, 3, 6, 8, 10].contains(&m.rem_euclid(12)) {
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(rect.left(), y - row / 2.0),
                        pos2(rect.right(), y + row / 2.0),
                    ),
                    0.0,
                    visuals.faint_bg_color,
                );
            }
            if m.rem_euclid(12) == 0 {
                painter.text(
                    pos2(rect.left() + 2.0, y),
                    egui::Align2::LEFT_CENTER,
                    note_name(m as i64 * 100),
                    egui::FontId::proportional(10.0),
                    visuals.weak_text_color(),
                );
            }
            m += 1;
        }
        // The line as sung, faint.
        if let Some(line) = c.line() {
            let step = (line.hop as i64).max(1);
            let mut prev: Option<Pos2> = None;
            let mut f = lo;
            while f < hi {
                match line.get(line.hop_of(f)) {
                    Some(v) if v > bottom && v < top => {
                        let p = pos2(x_of(f), y_of(v));
                        if let Some(q) = prev {
                            painter
                                .line_segment([q, p], Stroke::new(1.0, visuals.weak_text_color()));
                        }
                        prev = Some(p);
                    }
                    _ => prev = None,
                }
                f += step;
            }
        }
        let accent = visuals.selection.bg_fill;
        let mut blobs: Vec<Rect> = Vec::with_capacity(c.pitch_notes.len());
        for (i, n) in c.pitch_notes.iter().enumerate() {
            let (a, b) = n.span();
            let y = y_of((n.cents + n.shift) as f32 / 100.0);
            let r = Rect::from_min_max(
                pos2(x_of(a), y - row * 0.45),
                pos2(x_of(b).max(x_of(a) + 3.0), y + row * 0.45),
            );
            blobs.push(r);
            if n.is_edited() {
                // Where it was sung.
                let y0 = y_of(n.cents as f32 / 100.0);
                let r0 = Rect::from_min_max(
                    pos2(x_of(n.start), y0 - row * 0.45),
                    pos2(x_of(n.end), y0 + row * 0.45),
                );
                painter.rect_stroke(
                    r0,
                    row * 0.3,
                    Stroke::new(1.0, visuals.weak_text_color()),
                    egui::StrokeKind::Inside,
                );
            }
            let chosen = self.pitch.selected.contains(&i);
            let fill = if chosen {
                accent
            } else {
                accent.gamma_multiply(0.55)
            };
            painter.rect_filled(r, row * 0.3, fill);
            if chosen {
                painter.rect_stroke(
                    r,
                    row * 0.3,
                    Stroke::new(1.5, visuals.strong_text_color()),
                    egui::StrokeKind::Outside,
                );
            }
            // The line as it plays now, over the blob.
            if let Some(line) = c.line()
                && let Some((raw, slow)) = note_line(
                    line,
                    n.start,
                    n.end,
                    0,
                    line.hop as i64,
                    line.hop_of(n.start),
                    line.hop_of(n.end)
                        .saturating_sub(1)
                        .max(line.hop_of(n.start)),
                )
            {
                let centre = n.cents as f32 / 100.0;
                let (dk, vk) = (n.drift as f32 / 100.0 - 1.0, n.vibrato as f32 / 100.0 - 1.0);
                let i0 = line.hop_of(n.start);
                let stretch = (b - a) as f32 / (n.end - n.start).max(1) as f32;
                let pts: Vec<Pos2> = (raw.iter().zip(&slow).enumerate())
                    .map(|(j, (&r, &sl))| {
                        let f = ((i0 + j) as i64 * line.hop as i64).clamp(n.start, n.end);
                        let v = r + n.shift as f32 / 100.0 + dk * (sl - centre) + vk * (r - sl);
                        let x = x_of(a)
                            + (f - n.start) as f32 * stretch * rect.width()
                                / (hi - lo).max(1) as f32;
                        pos2(x, y_of(v))
                    })
                    .collect();
                painter.add(egui::Shape::line(
                    pts,
                    Stroke::new(1.5, visuals.strong_text_color()),
                ));
            }
            if r.width() > 40.0 {
                painter.text(
                    r.left_center() + vec2(3.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    note_name(((n.cents + n.shift) as f64 / 100.0).round() as i64 * 100),
                    egui::FontId::proportional(10.0),
                    Color32::WHITE,
                );
            }
        }
        // Choosing and dragging.
        let at = resp.interact_pointer_pos();
        let hit = |p: Pos2| blobs.iter().position(|r| r.expand(2.0).contains(p));
        let shift_held = ui.input(|i| i.modifiers.shift);
        if resp.clicked()
            && let Some(p) = at
        {
            match hit(p) {
                Some(i) if shift_held => {
                    if !self.pitch.selected.remove(&i) {
                        self.pitch.selected.insert(i);
                    }
                }
                Some(i) => {
                    self.pitch.selected = BTreeSet::from([i]);
                }
                None => self.pitch.selected.clear(),
            }
        }
        if !may_edit {
            return;
        }
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(i) = hit(p)
        {
            if !self.pitch.selected.contains(&i) {
                self.pitch.selected = BTreeSet::from([i]);
            }
            let what = if (blobs[i].right() - p.x).abs() < 6.0 {
                DragWhat::Length
            } else {
                DragWhat::Move
            };
            self.pitch.drag = Some(Drag {
                from: p,
                before: c.pitch_notes.clone(),
                what,
            });
        }
        let frames_per_px = (hi - lo) as f32 / rect.width().max(1.0);
        if let (Some(d), Some(p)) = (&self.pitch.drag, resp.interact_pointer_pos())
            && resp.dragged()
        {
            let dx = ((p.x - d.from.x) * frames_per_px).round() as i64;
            let dy = (d.from.y - p.y) / row;
            let cents = if ui.input(|i| i.modifiers.alt) {
                (dy * 100.0).round() as i64
            } else {
                dy.round() as i64 * 100
            };
            let min_len = (MIN_SECONDS * c.source.rate as f64) as i64;
            let mut notes = d.before.clone();
            for &i in &self.pitch.selected {
                if i >= notes.len() {
                    continue;
                }
                let n = d.before[i];
                match d.what {
                    DragWhat::Move => {
                        notes[i].shift = clamp_cents(n.shift + cents);
                        if dx != 0 {
                            retime(&mut notes, i, n.moved + dx, n.resized, (lo, hi), min_len);
                        }
                    }
                    DragWhat::Length => {
                        retime(&mut notes, i, n.moved, n.resized + dx, (lo, hi), min_len);
                    }
                }
            }
            self.tracks[l].clips[k].pitch_notes = notes;
        }
        if resp.drag_stopped() && self.pitch.drag.take().is_some() {
            // One rebuild (and one undo step) for the whole drag.
            self.rebuild();
        }
        if let Some(p) = resp.hover_pos() {
            let cursor = match hit(p) {
                Some(i) if (blobs[i].right() - p.x).abs() < 6.0 => {
                    egui::CursorIcon::ResizeHorizontal
                }
                Some(_) => egui::CursorIcon::Grab,
                None => egui::CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn note(start: i64, end: i64) -> PitchNote {
        PitchNote::found(start, end, 6_000)
    }

    #[test]
    fn names_read_as_notes_and_cents() {
        assert_eq!(note_name(6_900), "A4");
        assert_eq!(note_name(6_012), "C4 +12 ct");
        assert_eq!(note_name(6_088), "C♯4 −12 ct");
        let semi = |c, s| snap_shift(c, s, 0, SnapTo::Semitone, 0);
        assert_eq!(semi(6_012, 0), Some(-12));
        assert_eq!(semi(6_012, 140), Some(188));
        assert_eq!(semi(6_000, 0), None, "already there");
    }

    #[test]
    fn snapping_to_the_key_goes_to_its_nearest_note_as_heard() {
        use daw_project::ScaleKind;
        let c_major = SnapTo::Key(Scale::new(0, ScaleKind::Major));
        // C♯4 −45 ct: the nearest whole note is C♯, out of C major; the
        // key's nearest is C.
        assert_eq!(snap_shift(6_055, 0, 0, SnapTo::Semitone, 0), Some(45));
        assert_eq!(snap_shift(6_055, 0, 0, c_major, 0), Some(-55));
        // F♯4 sits between F and G, a semitone from each: the lower.
        assert_eq!(snap_shift(6_600, 0, 0, c_major, 0), Some(-100));
        assert_eq!(snap_shift(6_620, 0, 0, c_major, 0), Some(80));
        // Judged as heard: sung at F♯4 +55 ct in a clip played 6
        // semitones lower, it is heard at C♯4 −45 ct and goes to C4.
        let s = snap_shift(6_655, 0, -600, c_major, 0).unwrap();
        assert_eq!(6_655 + s - 600, 6_000);
        // A note's own move counts: moved up onto D, it is left there.
        assert_eq!(snap_shift(6_010, 190, 0, c_major, 0), None);
        assert_eq!(snap_shift(6_010, 170, 0, c_major, 0), Some(190));
        // Notes within the threshold are left alone; the rest move.
        assert_eq!(snap_shift(6_010, 0, 0, c_major, 30), None);
        assert_eq!(snap_shift(6_640, 0, 0, c_major, 30), Some(60));
        // Wide gaps: A minor pentatonic has none between A and C.
        let pent = SnapTo::Key(Scale::new(9, ScaleKind::MinorPentatonic));
        assert_eq!(snap_shift(5_840, 0, 0, pent, 0), Some(-140));
        assert_eq!(snap_shift(5_860, 0, 0, pent, 0), Some(140));
    }

    #[test]
    fn a_moved_note_squeezes_the_audio_around_it_and_no_further_than_it_can() {
        let range = (0, 100_000);
        let mut notes = vec![
            note(10_000, 20_000),
            note(20_000, 30_000),
            note(50_000, 60_000),
        ];
        // Later by 2,000: the first note's end pushes into the second.
        assert!(retime(&mut notes, 0, 2_000, 0, range, 100));
        let w = warp_of(&notes, range).unwrap();
        assert_eq!(w.src_at(12_000.0), 10_000.0);
        assert_eq!(w.src_at(22_000.0), 20_000.0);
        assert_eq!(w.src_at(30_000.0), 30_000.0);
        // Far too far: held where the audio after it would squeeze 3×.
        assert!(retime(&mut notes, 2, 39_000, 0, range, 100));
        let m = notes[2].moved;
        assert!(m > 20_000 && m < 39_000, "{m}");
        assert!(warp_of(&notes, range).is_some());
        // Shorter than the shortest isn't allowed.
        // Squeezed at most 3× however short it is asked to be.
        retime(&mut notes, 1, 0, -20_000, range, 500);
        let (a, b) = notes[1].span();
        assert!(b - a >= 3_333 && b - a < 10_000, "{a}..{b}");
    }

    #[test]
    fn found_notes_keep_the_edits_of_the_notes_they_replace() {
        let mut old = note(1_000, 9_000);
        old.shift = -30;
        old.vibrato = 0;
        old.moved = 400;
        let found = [
            pitch_line::LineNote {
                start: 1_200,
                end: 8_800,
                midi: 60.2,
            },
            pitch_line::LineNote {
                start: 12_000,
                end: 30_000,
                midi: 62.0,
            },
        ];
        let n = notes_from(&found, 0, 20_000, &[old]);
        assert_eq!(n.len(), 2);
        assert_eq!((n[0].cents, n[0].shift, n[0].vibrato), (6_020, -30, 0));
        assert_eq!(n[0].moved, 0, "times are found afresh");
        assert_eq!(
            (n[1].start, n[1].end),
            (12_000, 20_000),
            "kept inside the clip"
        );
        assert!(!n[1].is_edited());
    }

    mod in_the_app {
        use std::f32::consts::TAU;

        use serde_json::json;

        use crate::Track;
        use crate::agent::driver::Driver;
        use crate::timeline::{self, Clip};
        use daw_engine::stream::StreamSource;
        use daw_engine::time::Frames;

        const RATE: u32 = 48_000;

        /// A sung-like line saved as a take: each note `(midi, seconds)`
        /// after a short rest, with a few harmonics and `vib` semitones
        /// of vibrato at 5.5 Hz.
        fn sung(notes: &[(f32, f32)], vib: f32) -> Clip {
            let mut x = Vec::new();
            let mut phase = 0.0f32;
            for &(midi, secs) in notes {
                x.extend(std::iter::repeat_n(0.0, (RATE as f32 * 0.1) as usize));
                let n = (secs * RATE as f32) as usize;
                for t in 0..n {
                    let s = t as f32 / RATE as f32;
                    let m = midi + vib * (TAU * 5.5 * s).sin();
                    phase += TAU * 440.0 * 2f32.powf((m - 69.0) / 12.0) / RATE as f32;
                    let env = (s / 0.02).min(1.0) * ((secs - s) / 0.02).clamp(0.0, 1.0);
                    x.push(env * (0.4 * phase.sin() + 0.15 * (2.0 * phase).sin()));
                }
            }
            x.extend(std::iter::repeat_n(0.0, RATE as usize / 5));
            let dir = std::env::temp_dir().join(format!("daw-pitch-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("{}.wav", timeline::next_id()));
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for v in x {
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
            let src = StreamSource::open(&path).unwrap();
            Clip::new(src, path, 0, Some(0))
        }

        /// A track holding `c` in a running app; the clip's id.
        fn with_clip(d: &mut Driver, c: Clip) -> String {
            let key = c.key.clone();
            let app = d.app();
            app.tracks.push(Track::new("Vocal".into()));
            app.tracks[0].clips.push(c);
            app.rebuild();
            key
        }

        /// What the app plays, rendered offline, left channel.
        fn render(d: &mut Driver) -> Vec<f32> {
            let app = d.app();
            let n = app.tracks[0].clips[0].len;
            let g = app.session().compile(512).unwrap();
            let [l, _] = daw_engine::engine::render_offline(g, Frames(0), n, 512);
            l
        }

        /// The pitch line of `x` over `a..b` seconds: its middle and how
        /// far it wanders, in semitones.
        fn pitch_over(x: &[f32], a: f64, b: f64) -> (f32, f32) {
            let line = daw_engine::pitch_line::line(x, RATE);
            let (i0, i1) = (
                line.hop_of((a * RATE as f64) as i64),
                line.hop_of((b * RATE as f64) as i64),
            );
            let mut v: Vec<f32> = (i0..i1).filter_map(|i| line.get(i)).collect();
            assert!(v.len() > 5, "no pitch from {a} to {b}");
            v.sort_by(f32::total_cmp);
            (v[v.len() / 2], v[v.len() - 2] - v[1])
        }

        #[test]
        fn an_assistant_finds_a_vocals_notes_and_moves_one_and_it_is_heard_saved_and_undone() {
            let mut d = Driver::new();
            let key = with_clip(&mut d, sung(&[(60.0, 0.4), (62.0, 0.4), (64.0, 0.4)], 0.0));
            let v = d.call("get_pitch_notes", json!({"clip": key})).unwrap();
            assert_eq!(v["found"], false);
            assert!(
                d.call("edit_pitch_notes", json!({"clip": key, "snap": true}))
                    .is_err(),
                "not found yet"
            );
            let v = d
                .call("edit_pitch_notes", json!({"clip": key, "find": true}))
                .unwrap();
            let notes = v["notes"].as_array().unwrap();
            assert_eq!(notes.len(), 3, "{v}");
            assert!(notes[1]["pitch"].as_str().unwrap().starts_with("D4"), "{v}");
            assert!((notes[1]["start_seconds"].as_f64().unwrap() - 0.6).abs() < 0.03);
            // The middle note up to F, as one undo step.
            let v = d
                .call(
                    "edit_pitch_notes",
                    json!({"clip": key, "notes": [1], "to_midi": 65}),
                )
                .unwrap();
            // Only the note changed comes back, and how many there are.
            assert_eq!(v["count"], 3, "{v}");
            assert_eq!(v["notes"].as_array().unwrap().len(), 1, "{v}");
            assert_eq!(v["notes"][0]["index"], 1);
            assert_eq!(v["notes"][0]["now"], "F4");
            // Again changes nothing, so nothing comes back; all: true
            // gives every note, the others where they were.
            let v = d
                .call(
                    "edit_pitch_notes",
                    json!({"clip": key, "notes": [1], "to_midi": 65, "all": true}),
                )
                .unwrap();
            assert_eq!(v["notes"].as_array().unwrap().len(), 3, "{v}");
            assert!(
                v["notes"][0]["now"].as_str().unwrap().starts_with("C4"),
                "the others stay"
            );
            let v = d
                .call(
                    "edit_pitch_notes",
                    json!({"clip": key, "notes": [1], "to_midi": 65}),
                )
                .unwrap();
            assert_eq!(v["notes"].as_array().unwrap().len(), 0, "{v}");
            // Heard: the middle note sounds at F, the others where they were.
            let out = render(&mut d);
            let (mid, _) = pitch_over(&out, 0.7, 0.9);
            assert!((mid - 65.0).abs() < 0.15, "{mid}");
            let (first, _) = pitch_over(&out, 0.2, 0.4);
            assert!((first - 60.0).abs() < 0.15, "{first}");
            // Saved with the project.
            let saved = d.app().project().tracks[0].clips[0].pitch_notes.clone();
            assert_eq!(saved.len(), 3);
            assert_eq!(saved[1].cents + saved[1].shift, 6_500);
            // Bad calls change nothing.
            assert!(
                d.call(
                    "edit_pitch_notes",
                    json!({"clip": key, "notes": [7], "snap": true})
                )
                .is_err()
            );
            assert!(
                d.call("edit_pitch_notes", json!({"clip": key, "vibrato_pct": 900}))
                    .is_err()
            );
            d.command("undo");
            assert_eq!(d.app().tracks[0].clips[0].pitch_notes[1].shift, 0);
        }

        #[test]
        fn a_transposed_clip_that_follows_tempo_reads_as_heard_and_on_the_timeline() {
            let mut d = Driver::new();
            let notes = [(60.0, 0.4), (62.0, 0.4), (64.0, 0.4), (65.0, 0.4)];
            let key = with_clip(&mut d, sung(&notes, 0.0));
            // Two semitones down; played at 60 bpm and heard at the song's
            // 120, so twice as fast; placed at 2 s.
            d.call("edit_clip", json!({"clip": key, "transpose_semitones": -2}))
                .unwrap();
            d.call(
                "stretch_clips",
                json!({"clips": [key], "follow_tempo": true, "tempo": 60}),
            )
            .unwrap();
            d.call("edit_clip", json!({"clip": key, "seconds": 2.0}))
                .unwrap();
            let v = d
                .call("edit_pitch_notes", json!({"clip": key, "find": true}))
                .unwrap();
            assert_eq!(v["transpose_semitones"], -2.0, "{v}");
            let n = &v["notes"][1];
            assert!(n["pitch"].as_str().unwrap().starts_with("D4"), "{v}");
            assert!(n["now"].as_str().unwrap().starts_with("D4"), "{v}");
            assert!(n["plays"].as_str().unwrap().starts_with("C4"), "heard: {v}");
            assert!(
                (n["plays_midi"].as_f64().unwrap() - 60.0).abs() < 0.1,
                "{v}"
            );
            // 0.6 s into the file, heard 0.3 s after the clip's start.
            let s = n["start_seconds"].as_f64().unwrap();
            assert!((s - 0.6).abs() < 0.03, "file seconds: {v}");
            let t = n["timeline_seconds"].as_f64().unwrap();
            assert!((t - 2.3).abs() < 0.02, "timeline seconds: {v}");
            let e = n["timeline_end_seconds"].as_f64().unwrap();
            let len = n["seconds"].as_f64().unwrap();
            assert!((e - t - len / 2.0).abs() < 0.01, "heard twice as fast: {v}");
            // Put on a heard D: two up in the file, so it plays at D.
            let v = d
                .call(
                    "edit_pitch_notes",
                    json!({"clip": key, "notes": [1], "to_plays_midi": 62}),
                )
                .unwrap();
            // Only the note changed comes back.
            let n = &v["notes"][0];
            assert_eq!(n["index"], 1, "{v}");
            assert!(n["now"].as_str().unwrap().starts_with("E4"), "{v}");
            assert!(n["plays"].as_str().unwrap().starts_with("D4"), "{v}");
            assert!(
                d.call(
                    "edit_pitch_notes",
                    json!({"clip": key, "to_midi": 60, "to_plays_midi": 60})
                )
                .is_err(),
                "one or the other"
            );
            // A transpose with cents: snapping lands the heard pitch on a
            // whole note, not the file's.
            d.call(
                "edit_clip",
                json!({"clip": key, "transpose_semitones": -1.5}),
            )
            .unwrap();
            let v = d
                .call(
                    "edit_pitch_notes",
                    json!({"clip": key, "notes": [2], "snap": true}),
                )
                .unwrap();
            assert_eq!(v["notes"][0]["index"], 2, "{v}");
            let heard = v["notes"][0]["plays_midi"].as_f64().unwrap();
            assert!(
                (heard - heard.round()).abs() < 0.005,
                "on a whole note: {v}"
            );
            // The clip's offset and length are in the file's own seconds; it
            // ends on the timeline after half of that.
            let v = d
                .call(
                    "edit_clip",
                    json!({"clip": key, "offset_seconds": 0.5, "len_seconds": 1.0}),
                )
                .unwrap();
            assert_eq!(v["offset_seconds"], 0.5, "{v}");
            assert_eq!(v["len_seconds"], 1.0, "{v}");
            assert!(
                (v["end_seconds"].as_f64().unwrap() - 2.5).abs() < 0.001,
                "{v}"
            );
            let p = d.project();
            let c = &p["tracks"][0]["clips"][0];
            assert_eq!(c["offset_seconds"], 0.5, "{c}");
            assert!(
                (c["end_seconds"].as_f64().unwrap() - 2.5).abs() < 0.001,
                "{c}"
            );
        }

        #[test]
        fn an_assistant_snaps_a_vocal_to_the_songs_key_leaving_notes_already_close() {
            let mut d = Driver::new();
            // A C♯ sung 45 cents flat, and an E 10 cents sharp.
            let key = with_clip(&mut d, sung(&[(60.55, 0.4), (64.1, 0.4)], 0.0));
            d.call("edit_pitch_notes", json!({"clip": key, "find": true}))
                .unwrap();
            let e = d
                .call("edit_pitch_notes", json!({"clip": key, "snap": "key"}))
                .unwrap_err();
            assert!(e.contains("no key"), "{e}");
            d.call("set_key", json!({"key": "C major"})).unwrap();
            let e = d
                .call("edit_pitch_notes", json!({"clip": key, "min_cents": 30}))
                .unwrap_err();
            assert!(e.contains("snap"), "{e}");
            let v = d
                .call(
                    "edit_pitch_notes",
                    json!({"clip": key, "snap": "key", "min_cents": 30}),
                )
                .unwrap();
            assert_eq!(v["key"], "C major", "{v}");
            assert_eq!(v["snapped"], 1, "{v}");
            assert_eq!(v["notes"][0]["now"], "C4", "{v}");
            assert_eq!(
                v["notes"].as_array().unwrap().len(),
                1,
                "within 30 ct: left, so not listed: {v}"
            );
            // Heard: the first note now sounds at C, not C♯.
            let out = render(&mut d);
            let (first, _) = pitch_over(&out, 0.2, 0.4);
            assert!((first - 60.0).abs() < 0.15, "{first}");
            let (second, _) = pitch_over(&out, 0.7, 0.9);
            assert!((second - 64.1).abs() < 0.15, "{second}");
            // One undo step.
            d.command("undo");
            assert!(
                d.app().tracks[0].clips[0]
                    .pitch_notes
                    .iter()
                    .all(|n| n.shift == 0)
            );
        }

        #[test]
        fn drift_and_vibrato_can_be_taken_out_and_notes_moved_in_time() {
            let mut d = Driver::new();
            let key = with_clip(&mut d, sung(&[(57.0, 0.6), (60.0, 0.4)], 0.5));
            d.call("edit_pitch_notes", json!({"clip": key, "find": true}))
                .unwrap();
            let (_, wide) = pitch_over(&render(&mut d), 0.25, 0.55);
            assert!(wide > 0.6, "vibrato as sung: {wide}");
            d.call(
                "edit_pitch_notes",
                json!({"clip": key, "notes": [0], "vibrato_pct": 0}),
            )
            .unwrap();
            let (mid, flat) = pitch_over(&render(&mut d), 0.25, 0.55);
            assert!(flat < wide / 2.0, "vibrato taken out: {flat} from {wide}");
            assert!((mid - 57.0).abs() < 0.2, "{mid}");
            // The second note 0.1 s later: there is no pitch where it
            // used to start, and it ends where it did.
            let v = d
                .call(
                    "edit_pitch_notes",
                    json!({"clip": key, "notes": [1], "move_seconds": 0.1}),
                )
                .unwrap();
            let moved = (v["notes"].as_array().unwrap().iter())
                .find(|n| n["index"] == 1)
                .unwrap_or_else(|| panic!("{v}"));
            assert_eq!(moved["moved_seconds"], 0.1);
            let out = render(&mut d);
            let line = daw_engine::pitch_line::line(&out, RATE);
            let at = |s: f64| line.get(line.hop_of((s * RATE as f64) as i64));
            assert!(at(0.84).is_none(), "{:?}", at(0.84));
            assert!(at(1.0).is_some_and(|m| (m - 60.0).abs() < 0.3));
            let line = d
                .call("get_pitch_notes", json!({"clip": key, "line": true}))
                .unwrap();
            assert!(line["notes"][0]["line"].as_array().unwrap().len() >= 10);
        }

        #[test]
        fn the_pane_shows_the_notes_and_names_its_controls() {
            use egui_kittest::Harness;
            use egui_kittest::kittest::Queryable as _;
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| crate::App::new(Vec::new()));
            let c = sung(&[(60.0, 0.4), (64.0, 0.4)], 0.2);
            let id = c.id;
            let app = h.state_mut();
            app.tracks.push(Track::new("Vocal".into()));
            app.tracks[0].clips.push(c);
            app.rebuild();
            app.find_notes_now(id).unwrap();
            app.timeline.selected.insert(id);
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, crate::commands::Cmd::PitchEditor);
            h.run();
            for name in [
                "Find notes again",
                "Snap to notes",
                "Snap to key",
                "As sung",
                "Pitch",
                "Formants",
            ] {
                h.get_by_label(name);
            }
            h.get_by_label("Up a semitone").click();
            h.run();
            let n = &h.state().tracks[0].clips[0].pitch_notes;
            assert!(n.iter().all(|n| n.shift == 100), "{n:?}");
        }

        #[test]
        fn the_commands_act_on_the_chosen_notes_or_all() {
            let mut d = Driver::new();
            let mut c = sung(&[(60.3, 0.4), (62.0, 0.4)], 0.0);
            let id = c.id;
            c.pitch_notes = vec![
                daw_project::PitchNote::found(4_800, 24_000, 6_030),
                daw_project::PitchNote::found(28_800, 48_000, 6_200),
            ];
            with_clip(&mut d, c);
            d.app().timeline.selected.insert(id);
            d.command("snap-pitch-notes");
            let n = d.app().tracks[0].clips[0].pitch_notes.clone();
            assert_eq!((n[0].shift, n[1].shift), (-30, 0));
            d.app().pitch.clip = Some(id);
            d.app().pitch.selected.insert(1);
            d.command("pitch-notes-up");
            d.command("pitch-notes-up");
            d.command("pitch-notes-down");
            let n = d.app().tracks[0].clips[0].pitch_notes.clone();
            assert_eq!((n[0].shift, n[1].shift), (-30, 100), "only the chosen one");
            d.command("flatten-pitch-drift");
            assert_eq!(d.app().tracks[0].clips[0].pitch_notes[1].drift, 0);
            d.app().pitch.selected.clear();
            d.command("reset-pitch-notes");
            assert!(
                d.app().tracks[0].clips[0]
                    .pitch_notes
                    .iter()
                    .all(|n| !n.is_edited())
            );
            // To the key: off until the song has one; the threshold
            // leaves the first note (30 ct from C) where it is.
            assert!(!d.app().can_run(crate::commands::Cmd::SnapPitchNotesToKey));
            d.call("set_key", json!({"key": "F major"})).unwrap();
            d.app().pitch.min_cents = 40;
            d.command("snap-pitch-notes-to-key");
            let n = d.app().tracks[0].clips[0].pitch_notes.clone();
            assert_eq!((n[0].shift, n[1].shift), (0, 0), "both close enough");
            d.app().pitch.min_cents = 0;
            d.command("snap-pitch-notes-to-key");
            let n = d.app().tracks[0].clips[0].pitch_notes.clone();
            assert_eq!((n[0].shift, n[1].shift), (-30, 0));
        }
    }
}
