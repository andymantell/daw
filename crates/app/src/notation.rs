// SPDX-License-Identifier: GPL-3.0-or-later
//! Notation lanes (roadmap 3.8, plan B5): a track's notes shown as tab,
//! a standard staff or a drum staff in a band under its row on the
//! timeline, laid out by the notation crate along the timeline so every
//! note sits where it plays, following the tempo map, bars and loop.
//! Built at risk on decision 0009, which is still proposed.
//!
//! Notation is a view of the notes, not a separate score: the lane
//! shows the track's note clips as they play, and typing tab writes
//! notes (with their string and fret) into them. Notes without a string
//! and fret get the assigner's choice for showing, until "Choose strings
//! and frets" writes it into them.
//!
//! Every action here is an `App` method that takes what it acts on, so
//! the command table, menus and keys (and later the agent interface)
//! all call the same code.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use daw_engine::time::{Frames, TempoMap, Ticks};
use daw_notation::drums::DrumMap;
use daw_notation::fretting::{self, AssignOptions};
use daw_notation::layout::{self, Anchor, Item, Page};
use daw_notation::score::{
    self, Bar, Clef, Duration, Event, NoteValue, Part, PartKind, Score, Strum, TabSetup, TimeSig,
};
use daw_notation::tuning::Tuning;
use daw_project::{Articulation, Notation, NotationView, Note, NoteClip, PPQ, Technique};
use eframe::egui::{self, Align2, Color32, FontFamily, FontId, Pos2, Rect, Stroke, pos2, vec2};

use crate::{App, Track, user_dir};

/// The egui font family the music font is drawn with.
pub(crate) const MUSIC_FONT: &str = "bravura";
pub(crate) const MUSIC_FONT_DATA: &[u8] = include_bytes!("../fonts/bravura/Bravura.otf");
const TEXT_FONT_DATA: &[u8] =
    include_bytes!("../fonts/ibm-plex-sans-condensed/IBMPlexSansCondensed-Regular.ttf");

/// The staff space in a lane, in points.
pub(crate) const SPACE: f32 = 6.5;

/// Bars narrower than this, in points, are too tight to read: the lane
/// says to zoom in instead.
const MIN_BAR_PX: f32 = 36.0;

/// The General MIDI drum channel (10, counted from 1).
const DRUM_CHANNEL: u8 = 9;

// ---------------------------------------------------------------------------
// Notes to notation.

/// A track's notes as a notation part, with where each bar is.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub part: Part,
    /// Each bar's start and end, in ticks on the timeline.
    pub bars: Vec<(i64, i64)>,
    /// The project note each drawn note came from, by (bar, event,
    /// note); a note tied over a bar line is listed at each piece.
    pub ids: HashMap<(usize, usize, usize), String>,
    /// How tall the lane is, in points.
    pub height: f32,
}

/// The tuning a setting describes, made safe for the notation crate.
fn tuning_of(n: &Notation) -> Tuning {
    let strings: Vec<u8> = n
        .tuning
        .iter()
        .copied()
        .filter(|p| *p < 128)
        .take(12)
        .collect();
    if strings.is_empty() {
        Tuning::guitar_standard()
    } else {
        Tuning::new(&n.tuning_name, &strings)
    }
}

/// The notation part kind a setting shows.
fn kind_of(n: &Notation) -> PartKind {
    match n.view {
        NotationView::Tab => PartKind::Tab(TabSetup {
            tuning: tuning_of(n),
            capo: n.capo,
            show_staff: n.staff,
            show_rhythm: n.rhythm,
        }),
        NotationView::Staff => PartKind::Staff { clef: Clef::Treble },
        NotationView::Drums => PartKind::Drums(DrumMap::general_midi()),
    }
}

/// A note's techniques as the notation crate has them, and its strum.
fn techniques_of(n: &Note) -> (Vec<score::Technique>, Option<Strum>) {
    use score::Technique as T;
    let mut out = Vec::new();
    let mut strum = None;
    for t in &n.techniques {
        out.push(match t {
            Technique::HammerOn => T::HammerOn,
            Technique::PullOff => T::PullOff,
            Technique::Slide => T::Slide,
            Technique::Vibrato => T::Vibrato,
            Technique::PalmMute => T::PalmMute,
            Technique::LetRing => T::LetRing,
            Technique::Dead => T::Dead,
            Technique::Ghost => T::Ghost,
            Technique::NaturalHarmonic => T::NaturalHarmonic,
            Technique::ArtificialHarmonic => T::ArtificialHarmonic,
            Technique::Tap => T::Tap,
            Technique::Slap => T::Slap,
            Technique::Pop => T::Pop,
            Technique::TremoloPicking => T::TremoloPicking,
            Technique::StrumDown => {
                strum = Some(Strum::Down);
                continue;
            }
            Technique::StrumUp => {
                strum = Some(Strum::Up);
                continue;
            }
        });
    }
    if n.bend > 0.0 {
        out.push(T::Bend {
            semitones: n.bend as f32,
            release: false,
        });
    }
    if let Some(a) = n.articulation {
        out.extend(match a {
            Articulation::Accent => Some(T::Accent),
            Articulation::Ghost => Some(T::Ghost),
            Articulation::Flam => Some(T::Flam),
            Articulation::Drag => Some(T::Drag),
            Articulation::Roll => Some(T::Roll),
            Articulation::Open => Some(T::Open),
            Articulation::Closed => Some(T::Closed),
            // The pedal hi-hat has its own key and place on the staff.
            Articulation::Pedal => None,
            Articulation::Choke => Some(T::Choke),
            Articulation::Rimshot => Some(T::Rimshot),
            Articulation::CrossStick => Some(T::CrossStick),
        });
    }
    (out, strum)
}

/// A project note as the notation crate draws it. A string and fret the
/// note carries are kept (and locked, as the user chose them) when they
/// still sound its pitch; a string alone keeps the note on it.
fn score_note(n: &Note, kind: &PartKind) -> (score::Note, Option<Strum>) {
    let (techniques, strum) = techniques_of(n);
    let mut out = score::Note::new(n.pitch);
    out.techniques = techniques;
    if let PartKind::Tab(setup) = kind
        && let Some(s) = n.string
        && let Some(open) = setup.tuning.open_pitch(s, setup.capo)
        && n.pitch >= open
        && n.pitch - open <= 30
    {
        out.string = Some(s);
        out.fret = Some(n.pitch - open);
        out.locked = n.fret.is_some();
    }
    (out, strum)
}

/// One note's share of a bar, in ticks from the bar's start, already
/// moved to the grid.
#[derive(Clone)]
struct Piece {
    start: i64,
    end: i64,
    note: score::Note,
    strum: Option<Strum>,
    id: String,
    /// Feet on a drum staff: stems down, the second voice.
    feet: bool,
}

/// Quarter-note segments of a bar: whether each is in triplets, picked
/// by which grid the notes starting in it fit better.
fn triplet_segments(starts: &[i64], bar_len: i64) -> Vec<bool> {
    let segs = ((bar_len + PPQ - 1) / PPQ).max(1) as usize;
    let mut out = vec![false; segs];
    for (k, t) in out.iter_mut().enumerate() {
        let from = k as i64 * PPQ;
        // Only whole quarters can hold triplets.
        if from + PPQ > bar_len {
            continue;
        }
        let err = |unit: i64| -> i64 {
            (starts.iter())
                .filter(|&&s| s >= from && s < from + PPQ)
                .map(|&s| {
                    let r = s - from;
                    (r - (r as f64 / unit as f64).round() as i64 * unit).abs()
                })
                .sum()
        };
        let (straight, three) = (err(PPQ / 4), err(PPQ / 3));
        *t = three < straight && straight > PPQ / 32;
    }
    out
}

/// `t` (ticks in a bar) moved to the grid of its segment.
fn to_grid(t: i64, segs: &[bool]) -> i64 {
    let k = (t / PPQ).clamp(0, segs.len() as i64 - 1);
    let unit = if segs[k as usize] { PPQ / 3 } else { PPQ / 4 };
    let from = k * PPQ;
    from + ((t - from) as f64 / unit as f64).round() as i64 * unit
}

/// The grid step at `t` in a bar.
fn unit_at(t: i64, segs: &[bool]) -> i64 {
    let k = (t / PPQ).clamp(0, segs.len() as i64 - 1) as usize;
    if segs[k] { PPQ / 3 } else { PPQ / 4 }
}

/// Plain and dotted lengths, longest first, as (ticks, value, dots).
const LENGTHS: [(i64, NoteValue, u8); 10] = [
    (3840, NoteValue::Whole, 0),
    (2880, NoteValue::Half, 1),
    (1920, NoteValue::Half, 0),
    (1440, NoteValue::Quarter, 1),
    (960, NoteValue::Quarter, 0),
    (720, NoteValue::Eighth, 1),
    (480, NoteValue::Eighth, 0),
    (360, NoteValue::Sixteenth, 1),
    (240, NoteValue::Sixteenth, 0),
    (120, NoteValue::ThirtySecond, 0),
];

/// The span `a..b` of a bar as written lengths, each with where it
/// starts: triplet eighths inside triplet quarters, and otherwise the
/// longest plain or dotted length that starts on its own beat part and
/// doesn't cut into a triplet quarter.
fn lengths(mut a: i64, b: i64, segs: &[bool]) -> Vec<(i64, Duration)> {
    let mut out = Vec::new();
    let in_triplets = |t: i64| segs.get((t / PPQ) as usize).copied().unwrap_or(false);
    while a < b {
        let seg_end = (a / PPQ + 1) * PPQ;
        if in_triplets(a) && !(a % PPQ == 0 && b >= seg_end) {
            let d = if a % PPQ == 0 && b - a >= PPQ * 2 / 3 {
                Duration::triplet(NoteValue::Quarter)
            } else {
                Duration::triplet(NoteValue::Eighth)
            };
            out.push((a, d));
            a += i64::from(d.ticks());
            continue;
        }
        // Off the beat, only as far as the next beat, so the beat
        // still shows.
        let room = if a % PPQ == 0 {
            b - a
        } else {
            (seg_end - a).min(b - a)
        };
        let fits = |len: i64, base: i64| {
            if len > room || a % base.min(PPQ) != 0 {
                return false;
            }
            // Over a triplet quarter only whole.
            let mut t = (a / PPQ + 1) * PPQ;
            while t < a + len {
                if in_triplets(t) && t + PPQ > a + len {
                    return false;
                }
                t += PPQ;
            }
            true
        };
        let pick = LENGTHS.iter().find(|(len, _, dots)| {
            let base = if *dots > 0 { len * 2 / 3 } else { *len };
            fits(*len, base)
        });
        match pick {
            Some(&(len, value, dots)) => {
                out.push((
                    a,
                    Duration {
                        value,
                        dots,
                        tuplet: None,
                    },
                ));
                a += len;
            }
            None => {
                // Off every grid (it can't be after moving to it): the
                // nearest length, and done.
                out.push((a, Duration::from_ticks((b - a) as u32)));
                break;
            }
        }
    }
    out
}

/// One voice of a bar as events: chords where notes start together,
/// held until the next starts (or they end), and rests between.
fn voice_events(
    voice: u8,
    pieces: &[Piece],
    bar_len: i64,
    segs: &[bool],
    ids: &mut Vec<(usize, usize, String)>,
    first_event: usize,
) -> Vec<Event> {
    let mut groups: BTreeMap<i64, Vec<&Piece>> = BTreeMap::new();
    for p in pieces {
        groups.entry(p.start).or_default().push(p);
    }
    let onsets: Vec<i64> = groups.keys().copied().collect();
    let mut out: Vec<Event> = Vec::new();
    let rest = |out: &mut Vec<Event>, a: i64, b: i64| {
        for (_, d) in lengths(a, b, segs) {
            out.push(Event::rest(voice, d));
        }
    };
    let mut pos = 0;
    for (k, (&at, group)) in groups.iter().enumerate() {
        if at < pos {
            continue;
        }
        if at > pos {
            rest(&mut out, pos, at);
        }
        let next = onsets.get(k + 1).copied().unwrap_or(bar_len);
        let end = group.iter().map(|p| p.end).max().unwrap_or(at).min(next);
        let end = end.max(at + unit_at(at, segs)).min(bar_len);
        let parts = lengths(at, end, segs);
        let tied_over = |p: &Piece| p.note.tie;
        for (j, (_, d)) in parts.iter().enumerate() {
            let last = j + 1 == parts.len();
            let mut notes: Vec<score::Note> = Vec::new();
            for (ni, p) in group.iter().enumerate() {
                let mut n = p.note.clone();
                n.tie = !last || tied_over(p);
                if j > 0 {
                    // Marks belong to the note's first piece.
                    n.techniques.retain(|t| {
                        matches!(t, score::Technique::PalmMute | score::Technique::LetRing)
                    });
                }
                ids.push((first_event + out.len(), ni, p.id.clone()));
                notes.push(n);
            }
            let mut e = Event::chord(voice, *d, notes);
            if j == 0 {
                e.strum = group.iter().find_map(|p| p.strum);
            }
            out.push(e);
        }
        pos = end;
    }
    if pos < bar_len && !pieces.is_empty() {
        rest(&mut out, pos, bar_len);
    }
    out
}

/// A track's notes (placed on the timeline, in ticks) as a part shown
/// with `n`, with bars from the tempo map up to at least `until`.
pub(crate) fn build(notes: &[Note], n: &Notation, map: &TempoMap, name: &str, until: i64) -> Built {
    let kind = kind_of(n);
    let drums = DrumMap::general_midi();
    let last = notes
        .iter()
        .map(Note::end)
        .max()
        .unwrap_or(0)
        .max(until)
        .max(1);
    let bar_count = map.bar_beat(Ticks(last - 1)).bar.max(1) as usize;
    let mut bars_at = Vec::with_capacity(bar_count);
    for b in 1..=bar_count as i64 {
        bars_at.push((map.bar_start(b).0, map.bar_start(b + 1).0));
    }

    // Each note into the bar it starts in; pieces held over a bar line
    // carry on into the next.
    let mut by_bar: Vec<Vec<&Note>> = vec![Vec::new(); bar_count];
    for note in notes {
        let bar = (map.bar_beat(Ticks(note.start.max(0))).bar - 1) as usize;
        if let Some(v) = by_bar.get_mut(bar) {
            v.push(note);
        }
    }
    let mut part = Part {
        name: name.to_string(),
        kind: kind.clone(),
        bars: Vec::with_capacity(bar_count),
    };
    let mut ids = HashMap::new();
    let mut carried: Vec<Piece> = Vec::new();
    let mut last_sig = None;
    for (b, &(from, to)) in bars_at.iter().enumerate() {
        let bar_len = to - from;
        let sig = map.signature_at(Ticks(from));
        let starts: Vec<i64> = by_bar[b].iter().map(|n| n.start - from).collect();
        let segs = triplet_segments(&starts, bar_len);
        let mut pieces: Vec<Piece> = std::mem::take(&mut carried);
        let mut next_carry = Vec::new();
        for note in &by_bar[b] {
            let start = to_grid(note.start - from, &segs);
            let end_abs = note.end() - from;
            let end = to_grid(end_abs.min(bar_len), &segs).max(start + unit_at(start, &segs));
            let (mut sn, strum) = score_note(note, &kind);
            let feet = matches!(n.view, NotationView::Drums) && !drums.look_up(note.pitch).2;
            if start >= bar_len {
                // Moved onto the next bar line.
                next_carry.push(Piece {
                    start: 0,
                    end: (end_abs - bar_len).max(PPQ / 4),
                    note: sn,
                    strum,
                    id: note.id.clone(),
                    feet,
                });
                continue;
            }
            // Drums are hits: no ties over the bar.
            let over = n.view != NotationView::Drums && end_abs > bar_len + PPQ / 8;
            sn.tie = over;
            if over {
                let mut rest = score_note(note, &kind).0;
                rest.techniques.clear();
                next_carry.push(Piece {
                    start: 0,
                    end: end_abs - bar_len,
                    note: rest,
                    strum: None,
                    id: note.id.clone(),
                    feet,
                });
            }
            pieces.push(Piece {
                start,
                end: end.min(bar_len),
                note: sn,
                strum,
                id: note.id.clone(),
                feet,
            });
        }
        carried = next_carry;
        for p in &mut pieces {
            p.end = p.end.min(bar_len);
            if p.end <= p.start {
                p.end = (p.start + unit_at(p.start, &segs)).min(bar_len);
            }
        }
        let (hands, feet): (Vec<Piece>, Vec<Piece>) = pieces.into_iter().partition(|p| !p.feet);
        let mut found = Vec::new();
        let mut events = voice_events(1, &hands, bar_len, &segs, &mut found, 0);
        if hands.is_empty() {
            events = lengths(0, bar_len, &segs)
                .into_iter()
                .map(|(_, d)| Event::rest(1, d))
                .collect();
        }
        if !feet.is_empty() {
            let at = events.len();
            events.extend(voice_events(2, &feet, bar_len, &segs, &mut found, at));
        }
        for (e, ni, id) in found {
            ids.insert((b, e, ni), id);
        }
        let time = TimeSig {
            beats: sig.beats.min(255) as u8,
            beat_type: sig.unit.min(255) as u8,
        };
        part.bars.push(Bar {
            time: (last_sig != Some(time)).then_some(time),
            events,
            ..Bar::default()
        });
        last_sig = Some(time);
    }
    if matches!(part.kind, PartKind::Tab(_)) {
        fretting::assign(&mut part, &AssignOptions::default());
    }
    let height = layout::strip_height(&part, SPACE);
    Built {
        part,
        bars: bars_at,
        ids,
        height,
    }
}

/// A fingerprint of what a lane shows, to know when to build it again.
pub(crate) fn print(notes: &[Note], n: &Notation, map: &TempoMap, until: i64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for note in notes {
        (note.start, note.len, note.pitch, note.string, note.fret).hash(&mut h);
        note.channel.hash(&mut h);
        note.bend.to_bits().hash(&mut h);
        for t in &note.techniques {
            t.name().hash(&mut h);
        }
        note.articulation.map(|a| a as u8).hash(&mut h);
    }
    (n.view.name(), &n.tuning, n.capo, n.staff, n.rhythm).hash(&mut h);
    for (at, tempo, glide) in map.tempo_changes() {
        (at.0, tempo.as_f64().to_bits(), glide).hash(&mut h);
    }
    for (at, bar, sig) in map.signature_changes() {
        (at.0, bar, sig.beats, sig.unit).hash(&mut h);
    }
    until.hash(&mut h);
    h.finish()
}

/// Which view suits a track's notes: drums for notes on the drum
/// channel, bass tab for a track called bass, else guitar tab.
pub(crate) fn guess(name: &str, notes: &[Note]) -> Notation {
    let lower = name.to_lowercase();
    if (!notes.is_empty() && notes.iter().all(|n| n.channel == DRUM_CHANNEL))
        || lower.contains("drum")
        || lower.contains("kit")
    {
        Notation::drums()
    } else if lower.contains("bass") {
        Notation::bass()
    } else {
        Notation::guitar()
    }
}

// ---------------------------------------------------------------------------
// Notation to notes, for MusicXML coming in.

/// A score part's notes as project notes, in ticks from the score's
/// start, with ties joined into one note.
pub(crate) fn notes_of(part: &Part) -> Vec<Note> {
    use score::Technique as T;
    let times = part.time_signatures();
    let mut bar_at = Vec::with_capacity(part.bars.len());
    let mut at = 0i64;
    for t in &times {
        bar_at.push(at);
        at += i64::from(t.bar_ticks());
    }
    let drums = matches!(part.kind, PartKind::Drums(_));
    let mut out: Vec<Note> = Vec::new();
    // Notes tied on, by pitch: where in `out` they are.
    let mut open: HashMap<u8, usize> = HashMap::new();
    for (b, tick, e) in part.positions() {
        let event = &part.bars[b].events[e];
        let start = bar_at[b] + i64::from(tick);
        let len = i64::from(event.duration.ticks()).max(1);
        for sn in &event.notes {
            if let Some(&i) = open.get(&sn.pitch)
                && out[i].end() == start
            {
                out[i].len += len;
                if !sn.tie {
                    open.remove(&sn.pitch);
                }
                continue;
            }
            let mut n = Note::new(start, len, sn.pitch, 96);
            if drums {
                n.channel = DRUM_CHANNEL;
            }
            n.string = sn.string;
            n.fret = sn.fret;
            for t in &sn.techniques {
                let tech = match t {
                    T::HammerOn => Some(Technique::HammerOn),
                    T::PullOff => Some(Technique::PullOff),
                    T::Slide => Some(Technique::Slide),
                    T::Vibrato => Some(Technique::Vibrato),
                    T::PalmMute => Some(Technique::PalmMute),
                    T::LetRing => Some(Technique::LetRing),
                    T::Dead => Some(Technique::Dead),
                    T::Ghost if !drums => Some(Technique::Ghost),
                    T::NaturalHarmonic => Some(Technique::NaturalHarmonic),
                    T::ArtificialHarmonic => Some(Technique::ArtificialHarmonic),
                    T::Tap => Some(Technique::Tap),
                    T::Slap => Some(Technique::Slap),
                    T::Pop => Some(Technique::Pop),
                    T::TremoloPicking => Some(Technique::TremoloPicking),
                    T::Bend { semitones, .. } => {
                        n.bend = f64::from(*semitones);
                        None
                    }
                    _ => None,
                };
                if let Some(tech) = tech {
                    n.techniques.push(tech);
                    continue;
                }
                n.articulation = n.articulation.or(match t {
                    T::Accent => Some(Articulation::Accent),
                    T::Ghost => Some(Articulation::Ghost),
                    T::Flam => Some(Articulation::Flam),
                    T::Drag => Some(Articulation::Drag),
                    T::Roll => Some(Articulation::Roll),
                    T::Open => Some(Articulation::Open),
                    T::Closed => Some(Articulation::Closed),
                    T::Choke => Some(Articulation::Choke),
                    T::Rimshot => Some(Articulation::Rimshot),
                    T::CrossStick => Some(Articulation::CrossStick),
                    _ => None,
                });
            }
            match event.strum {
                Some(Strum::Down) => n.techniques.push(Technique::StrumDown),
                Some(Strum::Up) => n.techniques.push(Technique::StrumUp),
                None => {}
            }
            if sn.tie {
                open.insert(sn.pitch, out.len());
            }
            out.push(n);
        }
    }
    out.sort_by_key(|n| (n.start, n.pitch));
    out
}

/// How a score part is shown on its track.
pub(crate) fn notation_of(part: &Part) -> Notation {
    match &part.kind {
        PartKind::Tab(setup) => Notation {
            view: NotationView::Tab,
            tuning: setup.tuning.strings.clone(),
            tuning_name: setup.tuning.name.clone(),
            capo: setup.capo,
            staff: setup.show_staff,
            rhythm: setup.show_rhythm,
        },
        PartKind::Staff { .. } => Notation::staff(),
        PartKind::Drums(_) => Notation::drums(),
    }
}

// ---------------------------------------------------------------------------
// Drawing.

/// A lane's band as the timeline draws it.
pub(crate) struct Band<'a> {
    pub built: &'a Built,
    pub notation: Notation,
    /// The tab entry cursor: the tick and string, and the step typed
    /// notes last.
    pub cursor: Option<(i64, u8, i64)>,
    pub practice_muted: bool,
}

impl Band<'_> {
    pub(crate) fn height(&self) -> f32 {
        self.built.height.max(44.0)
    }
}

/// What a band says it shows, beside it.
fn label(n: &Notation) -> String {
    match n.view {
        NotationView::Tab => {
            let letters: Vec<&str> = (n.tuning.iter().rev())
                .map(|&p| daw_notation::tuning::pitch_class_name(p))
                .collect();
            let mut s = format!("Tab {}", letters.join(""));
            if n.capo > 0 {
                s.push_str(&format!(" capo {}", n.capo));
            }
            s
        }
        NotationView::Staff => "Staff".into(),
        NotationView::Drums => "Drums".into(),
    }
}

/// Draws laid-out items with their origin at `origin`.
fn paint(painter: &egui::Painter, items: &[Item], origin: Pos2, ink: Color32, paper: Color32) {
    let at = |p: (f32, f32)| origin + vec2(p.0, p.1);
    for item in items {
        match item {
            Item::Line {
                from,
                to,
                width,
                dashed,
            } => {
                let stroke = Stroke::new(width.max(0.6), ink);
                if *dashed {
                    painter.extend(egui::Shape::dashed_line(
                        &[at(*from), at(*to)],
                        stroke,
                        3.0,
                        2.0,
                    ));
                } else {
                    painter.line_segment([at(*from), at(*to)], stroke);
                }
            }
            Item::Glyph { at: p, glyph, size } => {
                let font = FontId::new(*size, FontFamily::Name(MUSIC_FONT.into()));
                draw_text(
                    painter,
                    &glyph.to_string(),
                    font,
                    at(*p),
                    Anchor::Start,
                    ink,
                    None,
                );
            }
            Item::Text {
                at: p,
                text,
                size,
                anchor,
                knockout,
                ..
            } => {
                let font = FontId::new(*size, FontFamily::Name("ibm-plex-sans-condensed".into()));
                let behind = knockout.then_some(paper);
                draw_text(painter, text, font, at(*p), *anchor, ink, behind);
            }
            Item::Curve {
                from,
                c1,
                c2,
                to,
                width,
            } => {
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    [at(*from), at(*c1), at(*c2), at(*to)],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(width.max(0.6), ink),
                ));
            }
            Item::Polygon { points } => {
                let points: Vec<Pos2> = points.iter().map(|p| at(*p)).collect();
                painter.add(egui::Shape::convex_polygon(points, ink, Stroke::NONE));
            }
            Item::Circle {
                centre,
                radius,
                fill,
            } => {
                if *fill {
                    painter.circle_filled(at(*centre), *radius, ink);
                } else {
                    painter.circle_stroke(at(*centre), *radius, Stroke::new(0.7, ink));
                }
            }
        }
    }
}

/// Text (or a music glyph) with its baseline at `at`, cleared behind if
/// `behind` is given.
fn draw_text(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    at: Pos2,
    anchor: Anchor,
    ink: Color32,
    behind: Option<Color32>,
) {
    let galley = painter.layout_no_wrap(text.to_string(), font, ink);
    let Some(row) = galley.rows.first() else {
        return;
    };
    let baseline = row.pos.y + row.glyphs.first().map_or(row.height(), |g| g.pos.y);
    let w = galley.size().x;
    let x = match anchor {
        Anchor::Start => at.x,
        Anchor::Middle => at.x - w / 2.0,
        Anchor::End => at.x - w,
    };
    let top_left = pos2(x, at.y - baseline);
    if let Some(paper) = behind {
        let h = galley.size().y;
        let r = Rect::from_min_size(pos2(x - 1.0, at.y - h * 0.62), vec2(w + 2.0, h * 0.7));
        painter.rect_filled(r, 1.0, paper);
    }
    painter.galley(top_left, galley, ink);
}

/// Draws a band: what it shows in the header part, then the notation
/// along the lanes, in line with them.
pub(crate) fn draw_band(
    painter: &egui::Painter,
    look: &crate::theme::Look,
    band: &Band,
    r: Rect,
    left: f32,
    tick_x: &dyn Fn(i64) -> f32,
) {
    let tc = look.c();
    let (paper, ink) = (tc.lane, tc.text);
    let head = Rect::from_min_max(r.min, pos2(left, r.bottom()));
    painter.rect_filled(head, 0.0, tc.panel);
    painter.rect_filled(Rect::from_min_max(pos2(left, r.top()), r.max), 0.0, paper);
    let mut text = label(&band.notation);
    if band.practice_muted {
        text.push_str(" · muted to play");
    }
    painter.text(
        head.left_top() + vec2(16.0, 4.0),
        Align2::LEFT_TOP,
        text,
        look.font(look.small),
        tc.text_dim,
    );
    let rows = layout::strip_rows(&band.built.part, SPACE);
    // The strings' letters beside the tab, lowest at the bottom.
    if let Some(tab_top) = rows.tab_top {
        for (i, &p) in band.notation.tuning.iter().enumerate() {
            let y = r.top() + tab_top + i as f32 * rows.string_gap;
            painter.text(
                pos2(left - 6.0, y),
                Align2::RIGHT_CENTER,
                daw_notation::tuning::pitch_class_name(p),
                FontId::proportional(SPACE * 1.8),
                tc.text_dim,
            );
        }
    }

    // The bars in view, and one either side for marks that reach over.
    let bars = &band.built.bars;
    let vis: Vec<usize> = (0..bars.len())
        .filter(|&i| tick_x(bars[i].1) >= left && tick_x(bars[i].0) <= r.right())
        .collect();
    let Some((&first, &last)) = vis.first().zip(vis.last()) else {
        return;
    };
    let from = first.saturating_sub(1);
    let to = (last + 2).min(bars.len());
    let spans: Vec<(f32, f32)> = (from..to)
        .map(|i| (tick_x(bars[i].0) - r.left(), tick_x(bars[i].1) - r.left()))
        .collect();
    let narrowest = (first..=last)
        .map(|i| tick_x(bars[i].1) - tick_x(bars[i].0))
        .fold(f32::MAX, f32::min);
    let clipped = painter.with_clip_rect(
        painter
            .clip_rect()
            .intersect(Rect::from_min_max(pos2(left, r.top()), r.max)),
    );
    if narrowest < MIN_BAR_PX {
        clipped.text(
            pos2(left + 8.0, r.center().y),
            Align2::LEFT_CENTER,
            "Zoom in to read the notation",
            look.font(look.small),
            tc.text_dim,
        );
        return;
    }
    // The tab entry cursor, under the notation.
    if let Some((tick, string, step)) = band.cursor {
        let (x0, x1) = (tick_x(tick), tick_x(tick + step));
        clipped.rect_filled(
            Rect::from_x_y_ranges(x0..=x1.max(x0 + 4.0), r.top()..=r.bottom()),
            0.0,
            tc.selection,
        );
        if let Some(tab_top) = rows.tab_top {
            let y = r.top() + tab_top + f32::from(string.max(1) - 1) * rows.string_gap;
            clipped.rect_stroke(
                Rect::from_center_size(
                    pos2(x0 + 0.3 * SPACE + 0.59 * SPACE, y),
                    vec2(rows.string_gap * 1.2, rows.string_gap),
                ),
                2.0,
                Stroke::new(1.5, tc.accent),
                egui::StrokeKind::Middle,
            );
        }
    }
    let page: Page = layout::strip(&band.built.part, from..to, &spans, SPACE);
    paint(&clipped, &page.items, r.min, ink, paper);
}

// ---------------------------------------------------------------------------
// The app's side: lanes, tab entry and the actions.

/// Tab entry: where the next number typed goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    /// The track, by key.
    pub track: String,
    pub tick: i64,
    /// Counted from 1, the highest string.
    pub string: u8,
    /// How long typed notes are, and how far the cursor moves.
    pub step: i64,
    /// A digit just typed here, for two-digit frets, and when.
    typed: Option<(u8, f64)>,
}

/// Notation lanes' state: each track's notes laid out (kept until they
/// change) and tab entry.
#[derive(Default)]
pub(crate) struct Lanes {
    built: HashMap<String, (u64, Built)>,
    pub entry: Option<Entry>,
    /// The notation window, on a track by key.
    pub window: Option<String>,
}

/// Seconds within which a second digit makes a two-digit fret.
const TWO_DIGITS: f64 = 0.8;

impl App {
    /// The track notation commands act on: the one being typed into,
    /// else the note editor's, else the one with the selected note
    /// clip, else the first showing notation, else the first that plays
    /// notes.
    pub(crate) fn notation_track(&self) -> Option<usize> {
        let by_key = |k: &str| self.tracks.iter().position(|t| t.key == k);
        let by_clip =
            |c: &str| (self.tracks.iter()).position(|t| t.note_clips.iter().any(|n| n.id == c));
        (self.notation.entry.as_ref().and_then(|e| by_key(&e.track)))
            .or_else(|| self.notation.window.as_deref().and_then(by_key))
            .or_else(|| self.notes_editor.clip.as_deref().and_then(by_clip))
            .or_else(|| self.timeline.note_sel.as_deref().and_then(by_clip))
            .or_else(|| self.tracks.iter().position(|t| t.notation.is_some()))
            .or_else(|| self.tracks.iter().position(|t| t.kind.plays_notes()))
    }

    /// Shows a notation lane under a track, changes it, or (with `None`)
    /// takes it off.
    pub(crate) fn set_notation(&mut self, track: usize, n: Option<Notation>) {
        let Some(t) = self.tracks.get_mut(track) else {
            return;
        };
        if n.is_none()
            && self
                .notation
                .entry
                .as_ref()
                .is_some_and(|e| e.track == t.key)
        {
            self.notation.entry = None;
        }
        t.notation = n;
    }

    /// Shows the lane on the notation track, guessing the view from its
    /// notes, or takes it off.
    pub(crate) fn toggle_notation_lane(&mut self) {
        let Some(i) = self.notation_track() else {
            self.message = Some("Add an instrument track first: notation shows its notes.".into());
            return;
        };
        let t = &self.tracks[i];
        let n = match t.notation {
            Some(_) => None,
            None => Some(guess(&t.name, &placed(&t.note_clips))),
        };
        self.set_notation(i, n);
    }

    /// Tab, then a staff, then a drum staff.
    pub(crate) fn next_notation_view(&mut self) {
        let Some(i) = self.notation_track() else {
            return;
        };
        let next = match &self.tracks[i].notation {
            None => Notation::guitar(),
            Some(n) => {
                let view = match n.view {
                    NotationView::Tab => NotationView::Staff,
                    NotationView::Staff => NotationView::Drums,
                    NotationView::Drums => NotationView::Tab,
                };
                let mut n = n.clone();
                n.view = view;
                if view == NotationView::Tab && n.tuning.is_empty() {
                    n.tuning = Notation::guitar().tuning;
                    n.tuning_name = Notation::guitar().tuning_name;
                }
                n
            }
        };
        self.set_notation(i, Some(next));
    }

    /// Starts typing tab on the notation track at the playhead, or stops.
    pub(crate) fn toggle_tab_entry(&mut self) {
        if self.notation.entry.take().is_some() {
            return;
        }
        let Some(i) = self.notation_track() else {
            self.message = Some("Add an instrument track first: tab writes its notes.".into());
            return;
        };
        if !matches!(&self.tracks[i].notation, Some(n) if n.view == NotationView::Tab) {
            let t = &self.tracks[i];
            let mut n = t.notation.clone().unwrap_or_else(Notation::guitar);
            if n.tuning.is_empty() {
                n = Notation::guitar();
            }
            n.view = NotationView::Tab;
            self.set_notation(i, Some(n));
        }
        let map = self.tempo_map();
        let at = map
            .ticks_at(Frames(self.ctl.status().position().0.max(0)))
            .0;
        let step = PPQ / 2;
        self.notation.entry = Some(Entry {
            track: self.tracks[i].key.clone(),
            tick: snap_tick(&map, at, step),
            string: 1,
            step,
            typed: None,
        });
    }

    /// Writes a fret on a string at a tick of a track: the note that
    /// sounds there, `len` long, replacing one on that string there. It
    /// goes in the note clip under the tick, or a new bar-long one.
    pub(crate) fn tab_put(
        &mut self,
        track: usize,
        tick: i64,
        string: u8,
        fret: u8,
        len: i64,
    ) -> bool {
        let map = self.tempo_map();
        let Some(t) = self.tracks.get_mut(track) else {
            return false;
        };
        let Some(n) = t.notation.as_ref() else {
            return false;
        };
        let Some(pitch) = tuning_of(n).pitch(string, fret, n.capo) else {
            return false;
        };
        if pitch > 127 {
            return false;
        }
        clear_at(&mut t.note_clips, tick, string);
        let clip = clip_at(&mut t.note_clips, &map, tick);
        let mut note = Note::new(tick - clip.place(0), len.max(1), pitch, 96);
        note.string = Some(string);
        note.fret = Some(fret);
        clip.notes.push(note);
        // A note running past the clip's end makes the clip longer.
        clip.len = clip.len.max(tick + len - clip.start);
        clip.sort_notes();
        self.rebuild();
        true
    }

    /// Takes the note off a string at a tick of a track.
    pub(crate) fn tab_clear(&mut self, track: usize, tick: i64, string: u8) -> bool {
        let Some(t) = self.tracks.get_mut(track) else {
            return false;
        };
        let changed = clear_at(&mut t.note_clips, tick, string);
        if changed {
            self.rebuild();
        }
        changed
    }

    /// Moves notes of a track to the next string up (`by` -1, towards
    /// string 1) or down (+1), keeping their pitch: the same note
    /// played elsewhere on the neck. Notes it can't move stay.
    pub(crate) fn move_strings(&mut self, track: usize, ids: &[String], by: i8) -> usize {
        let Some(t) = self.tracks.get_mut(track) else {
            return 0;
        };
        let Some(n) = t.notation.clone().filter(|n| n.view == NotationView::Tab) else {
            return 0;
        };
        // Strings shown for notes without one: the assigner's choice.
        let shown = self
            .notation
            .built
            .get(&t.key)
            .map(|(_, b)| strings_shown(b));
        let tuning = tuning_of(&n);
        let mut moved = 0;
        for c in &mut t.note_clips {
            for note in c.notes.iter_mut().filter(|x| ids.contains(&x.id)) {
                let Some(now) = note
                    .string
                    .or_else(|| shown.as_ref().and_then(|s| s.get(&note.id).copied()))
                else {
                    continue;
                };
                let to = i16::from(now) + i16::from(by);
                if to < 1 || to as usize > tuning.string_count() {
                    continue;
                }
                let to = to as u8;
                let Some(open) = tuning.open_pitch(to, n.capo) else {
                    continue;
                };
                if note.pitch < open || note.pitch - open > 24 {
                    continue;
                }
                note.string = Some(to);
                note.fret = Some(note.pitch - open);
                moved += 1;
            }
        }
        if moved > 0 {
            self.rebuild();
        }
        moved
    }

    /// The notes `move_strings` acts on: the note editor's selection, or
    /// the notes under the tab cursor.
    fn string_targets(&self) -> Option<(usize, Vec<String>)> {
        if let Some(e) = &self.notation.entry {
            let i = self.tracks.iter().position(|t| t.key == e.track)?;
            let ids = (placed(&self.tracks[i].note_clips).into_iter())
                .filter(|n| n.start <= e.tick && n.end() > e.tick)
                .filter(|n| n.string == Some(e.string))
                .map(|n| n.id)
                .collect::<Vec<_>>();
            let ids = if ids.is_empty() {
                // A note without a string, shown on this one.
                let shown = self
                    .notation
                    .built
                    .get(&e.track)
                    .map(|(_, b)| strings_shown(b));
                (placed(&self.tracks[i].note_clips).into_iter())
                    .filter(|n| n.start <= e.tick && n.end() > e.tick)
                    .filter(|n| {
                        shown.as_ref().and_then(|s| s.get(&n.id)).copied() == Some(e.string)
                    })
                    .map(|n| n.id)
                    .collect()
            } else {
                ids
            };
            return Some((i, ids));
        }
        let clip = self.notes_editor.clip.as_deref()?;
        let i = (self.tracks.iter()).position(|t| t.note_clips.iter().any(|c| c.id == clip))?;
        let c = self.tracks[i].note_clips.iter().find(|c| c.id == clip)?;
        let picked = &self.notes_editor.selected;
        let ids = (c.notes.iter())
            .filter(|n| picked.is_empty() || picked.contains(&n.id))
            .map(|n| n.id.clone())
            .collect();
        Some((i, ids))
    }

    pub(crate) fn notes_to_string(&mut self, by: i8) {
        let Some((track, ids)) = self.string_targets() else {
            self.message = Some("Open a note clip in the note editor, or type tab, first.".into());
            return;
        };
        if self.tracks[track].notation.as_ref().map(|n| n.view) != Some(NotationView::Tab) {
            self.message = Some("Show the track as tab first.".into());
            return;
        }
        if self.move_strings(track, &ids, by) == 0 {
            self.message = Some("No note there can move to that string.".into());
        } else if let Some(e) = &mut self.notation.entry {
            e.string = (i16::from(e.string) + i16::from(by)).clamp(1, 12) as u8;
        }
    }

    /// Writes the assigner's strings and frets into a tab track's notes
    /// that have none. Returns how many it chose.
    pub(crate) fn assign_frets(&mut self, track: usize) -> usize {
        let map = self.tempo_map();
        let until = self.notation_until(&map);
        let Some(t) = self.tracks.get_mut(track) else {
            return 0;
        };
        let Some(n) = t.notation.clone().filter(|n| n.view == NotationView::Tab) else {
            return 0;
        };
        let built = build(&placed(&t.note_clips), &n, &map, &t.name, until);
        let chosen = strings_shown(&built);
        let tuning = tuning_of(&n);
        let mut count = 0;
        for c in &mut t.note_clips {
            for note in &mut c.notes {
                if note.string.is_some() && note.fret.is_some() {
                    continue;
                }
                if let Some(&s) = chosen.get(&note.id)
                    && let Some(open) = tuning.open_pitch(s, n.capo)
                    && note.pitch >= open
                {
                    note.string = Some(s);
                    note.fret = Some(note.pitch - open);
                    count += 1;
                }
            }
        }
        if count > 0 {
            self.rebuild();
        }
        count
    }

    /// Where the lanes' bars run to: the end of the song, at least four
    /// bars in.
    fn notation_until(&self, map: &TempoMap) -> i64 {
        let frames = (self.tracks.iter())
            .flat_map(|t| t.clips.iter().map(|c| c.end()))
            .max()
            .unwrap_or(0);
        let notes = (self.tracks.iter())
            .flat_map(|t| t.note_clips.iter().map(NoteClip::end))
            .max()
            .unwrap_or(0);
        map.ticks_at(Frames(frames))
            .0
            .max(notes)
            .max(map.bar_start(self.loop_bars.1).0)
            .max(map.bar_start(5).0)
    }

    /// Lays out the lanes whose notes changed since last time, and
    /// forgets those taken off.
    pub(crate) fn refresh_notation(&mut self, map: &TempoMap) {
        let until = self.notation_until(map);
        let keys: Vec<String> = (self.tracks.iter())
            .filter(|t| t.notation.is_some())
            .map(|t| t.key.clone())
            .collect();
        self.notation.built.retain(|k, _| keys.contains(k));
        for t in self.tracks.iter().filter(|t| t.notation.is_some()) {
            let n = t.notation.as_ref().expect("filtered");
            let notes = placed(&t.note_clips);
            let p = print(&notes, n, map, until);
            if self
                .notation
                .built
                .get(&t.key)
                .is_some_and(|(q, _)| *q == p)
            {
                continue;
            }
            let b = build(&notes, n, map, &t.name, until);
            self.notation.built.insert(t.key.clone(), (p, b));
        }
    }

    /// The bands under the lanes, for the timeline to draw. Borrows the
    /// layouts, which the caller takes out of `self` while drawing.
    pub(crate) fn bands<'a>(
        tracks: &[Track],
        built: &'a HashMap<String, (u64, Built)>,
        entry: Option<&Entry>,
    ) -> Vec<Option<Band<'a>>> {
        (tracks.iter())
            .map(|t| {
                let n = t.notation.as_ref()?;
                let (_, b) = built.get(&t.key)?;
                Some(Band {
                    built: b,
                    notation: n.clone(),
                    cursor: entry
                        .filter(|e| e.track == t.key)
                        .map(|e| (e.tick, e.string, e.step)),
                    practice_muted: t.practice_mute,
                })
            })
            .collect()
    }

    /// The timeline's layouts, taken out while it draws with them.
    pub(crate) fn take_notation_layouts(&mut self) -> HashMap<String, (u64, Built)> {
        std::mem::take(&mut self.notation.built)
    }

    pub(crate) fn put_notation_layouts(&mut self, built: HashMap<String, (u64, Built)>) {
        self.notation.built = built;
    }

    /// A click in a band: puts the tab cursor there, on the nearest
    /// string.
    pub(crate) fn band_clicked(&mut self, lane: usize, tick: i64, y: f32) {
        let map = self.tempo_map();
        let Some(t) = self.tracks.get(lane) else {
            return;
        };
        let Some(n) = &t.notation else {
            return;
        };
        if n.view != NotationView::Tab {
            return;
        }
        let Some((_, b)) = self.notation.built.get(&t.key) else {
            return;
        };
        let rows = layout::strip_rows(&b.part, SPACE);
        let strings = n.tuning.len().max(1) as f32;
        let string = rows.tab_top.map_or(1.0, |top| {
            ((y - top) / rows.string_gap)
                .round()
                .clamp(0.0, strings - 1.0)
                + 1.0
        }) as u8;
        let step = self.notation.entry.as_ref().map_or(PPQ / 2, |e| e.step);
        self.notation.entry = Some(Entry {
            track: t.key.clone(),
            tick: snap_tick(&map, tick - step / 2, step),
            string,
            step,
            typed: None,
        });
    }

    /// Tab entry's keys, taken before the commands see them: numbers
    /// write frets, the arrows move, Delete clears, `,` and `.` halve
    /// and double the step, Shift+Up/Down move the note to another
    /// string, Escape stops.
    pub(crate) fn notation_keys(&mut self, ctx: &egui::Context) {
        if self.notation.entry.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, Modifiers};
        let now = ctx.input(|i| i.time);
        let digits = [
            Key::Num0,
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        let mut typed = None;
        let mut moves: Vec<(i64, i8)> = Vec::new();
        let (mut clear, mut stop) = (false, false);
        let mut string_by = 0i8;
        let mut step_by = 0i32;
        ctx.input_mut(|i| {
            for (d, k) in digits.iter().enumerate() {
                if i.consume_key(Modifiers::NONE, *k) {
                    typed = Some(d as u8);
                }
            }
            let s = Modifiers::SHIFT;
            if i.consume_key(s, Key::ArrowUp) {
                string_by = -1;
            }
            if i.consume_key(s, Key::ArrowDown) {
                string_by = 1;
            }
            for (k, m) in [
                (Key::ArrowLeft, (-1, 0)),
                (Key::ArrowRight, (1, 0)),
                (Key::ArrowUp, (0, -1)),
                (Key::ArrowDown, (0, 1)),
            ] {
                if i.consume_key(Modifiers::NONE, k) {
                    moves.push(m);
                }
            }
            clear = i.consume_key(Modifiers::NONE, Key::Delete)
                | i.consume_key(Modifiers::NONE, Key::Backspace);
            stop = i.consume_key(Modifiers::NONE, Key::Escape);
            if i.consume_key(Modifiers::NONE, Key::Comma) {
                step_by = -1;
            }
            if i.consume_key(Modifiers::NONE, Key::Period) {
                step_by = 1;
            }
        });
        if stop {
            self.notation.entry = None;
            return;
        }
        if string_by != 0 {
            self.notes_to_string(string_by);
        }
        let Some(e) = self.notation.entry.clone() else {
            return;
        };
        let Some(track) = self.tracks.iter().position(|t| t.key == e.track) else {
            self.notation.entry = None;
            return;
        };
        let strings =
            (self.tracks[track].notation.as_ref()).map_or(6, |n| n.tuning.len().max(1)) as i16;
        let mut e = e;
        if step_by != 0 {
            e.step = if step_by < 0 {
                (e.step / 2).max(PPQ / 8)
            } else {
                (e.step * 2).min(PPQ * 4)
            };
        }
        for (dx, ds) in moves {
            e.tick = (e.tick + dx * e.step).max(0);
            e.string = (i16::from(e.string) + i16::from(ds)).clamp(1, strings) as u8;
            e.typed = None;
        }
        if clear {
            self.tab_clear(track, e.tick, e.string);
            e.typed = None;
        }
        if let Some(d) = typed {
            // A second digit soon after makes a two-digit fret.
            let fret = match e.typed {
                Some((first, at)) if now - at < TWO_DIGITS && first * 10 + d <= 24 => {
                    first * 10 + d
                }
                _ => d,
            };
            if self.tab_put(track, e.tick, e.string, fret, e.step) {
                // 0, 1 and 2 can start a two-digit fret.
                e.typed = (fret < 3).then_some((fret, now));
            } else {
                self.message = Some(format!(
                    "Fret {fret} can't be played on string {}.",
                    e.string
                ));
            }
        }
        self.notation.entry = Some(e);
    }

    // -- Files.

    /// The tracks showing notation as a score, laid out as on their
    /// lanes, with the song's starting tempo.
    pub(crate) fn notation_score(&self) -> Score {
        let map = self.tempo_map();
        let until = self.notation_until(&map);
        let parts = (self.tracks.iter())
            .filter_map(|t| {
                let n = t.notation.as_ref()?;
                let mut b = build(&placed(&t.note_clips), n, &map, &t.name, until).part;
                if let Some(first) = b.bars.first_mut() {
                    first.tempo = Some(self.tempo_bpm);
                }
                Some(b)
            })
            .collect();
        Score {
            title: (self.proj.folder.as_ref()).map_or_else(String::new, |f| f.name()),
            composer: String::new(),
            parts,
        }
    }

    /// Adds a track for each part of a MusicXML score, showing it as the
    /// score did. With no tracks yet, the score's tempo and time
    /// signatures become the song's. Returns how many tracks it added.
    pub(crate) fn import_musicxml(&mut self, xml: &str, fallback: &str) -> Result<usize, String> {
        let score = daw_notation::musicxml::read(xml).map_err(|e| e.to_string())?;
        if score.parts.is_empty() {
            return Err("it has no parts".into());
        }
        if self.tracks.is_empty() {
            let first = &score.parts[0];
            if let Some(bpm) = first.bars.iter().find_map(|b| b.tempo) {
                self.tempo_bpm = bpm.clamp(10.0, 999.0);
            }
            let times = first.time_signatures();
            if let Some(t) = times.first() {
                self.beats_per_bar = u32::from(t.beats.max(1));
                self.time.beat_unit = u32::from(t.beat_type.max(1));
            }
            self.time.meters.clear();
            for (i, b) in first.bars.iter().enumerate().skip(1) {
                if let Some(t) = b.time {
                    self.time.meters.push(daw_project::MeterChange {
                        id: daw_project::new_id(),
                        bar: i as i64 + 1,
                        beats: u32::from(t.beats.max(1)),
                        unit: u32::from(t.beat_type.max(1)),
                    });
                }
            }
        }
        let mut added = 0;
        for (i, part) in score.parts.iter().enumerate() {
            let notes = notes_of(part);
            let name = if part.name.is_empty() {
                format!("{fallback} {}", i + 1)
            } else {
                part.name.clone()
            };
            let mut t = Track::instrument(name.clone());
            let end = notes.iter().map(Note::end).max().unwrap_or(PPQ);
            let len = ((end + PPQ - 1) / PPQ) * PPQ;
            let mut clip = NoteClip::new(daw_project::new_id(), name, 0, len);
            clip.notes = notes;
            clip.sort_notes();
            t.note_clips.push(clip);
            t.notation = Some(notation_of(part));
            if let Some(inst) = &t.instrument {
                inst.make_into(self.rate, &mut self.pending_instruments);
            }
            self.tracks.push(t);
            added += 1;
        }
        self.rebuild();
        Ok(added)
    }

    pub(crate) fn pick_musicxml_file(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Import MusicXML")
            .set_directory(user_dir())
            .add_filter("MusicXML", &["musicxml", "xml"])
            .pick_file();
        if let Some(path) = picked {
            self.import_musicxml_file(&path);
        }
    }

    pub(crate) fn import_musicxml_file(&mut self, path: &Path) {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Part".into());
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.message = Some(format!("Could not read {}: {e}", path.display()));
                return;
            }
        };
        self.message = Some(match self.import_musicxml(&text, &stem) {
            Ok(n) => format!(
                "Imported {n} part{} from {}.",
                if n == 1 { "" } else { "s" },
                path.display()
            ),
            Err(e) => format!("Could not import {}: {e}", path.display()),
        });
    }

    /// Asks where, then writes the notation lanes as `what`.
    pub(crate) fn export_notation(&mut self, what: NotationFile) {
        let score = self.notation_score();
        if score.parts.is_empty() {
            self.message = Some("Show a notation lane under a track first.".into());
            return;
        }
        let (title, filter, ext) = match what {
            NotationFile::MusicXml => ("Export MusicXML", "MusicXML", "musicxml"),
            NotationFile::TextTab => ("Export tab as text", "Text", "txt"),
            NotationFile::Pdf => ("Print notation to PDF", "PDF", "pdf"),
        };
        let Some(dest) = rfd::FileDialog::new()
            .set_title(title)
            .set_directory(user_dir())
            .add_filter(filter, &[ext])
            .save_file()
        else {
            return;
        };
        let dest = if dest
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            dest
        } else {
            PathBuf::from(format!("{}.{ext}", dest.display()))
        };
        self.write_notation(what, &score, &dest);
    }

    pub(crate) fn write_notation(&mut self, what: NotationFile, score: &Score, dest: &Path) {
        let bytes = notation_bytes(what, score);
        self.message = Some(match bytes.map(|b| std::fs::write(dest, b)) {
            Ok(Ok(())) => format!("Wrote {}.", dest.display()),
            Ok(Err(e)) => format!("Could not write {}: {e}", dest.display()),
            Err(e) => format!("Could not write {}: {e}", dest.display()),
        });
    }

    // -- The window.

    pub(crate) fn open_notation_window(&mut self) {
        match self.notation_track() {
            Some(i) => self.notation.window = Some(self.tracks[i].key.clone()),
            None => {
                self.message =
                    Some("Add an instrument track first: notation shows its notes.".into());
            }
        }
    }

    /// The notation window: how the chosen track's lane shows its notes.
    pub(crate) fn notation_window(&mut self, ctx: &egui::Context) {
        let Some(key) = self.notation.window.clone() else {
            return;
        };
        let Some(i) = self.tracks.iter().position(|t| t.key == key) else {
            self.notation.window = None;
            return;
        };
        let mut open = true;
        let mut n = self.tracks[i].notation.clone();
        let mut assign = false;
        let mut mute = self.tracks[i].practice_mute;
        egui::Window::new(format!("Notation: {}", self.tracks[i].name))
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                let mut shown = n.is_some();
                ui.checkbox(&mut shown, "Show a notation lane under the track");
                if shown != n.is_some() {
                    n = shown
                        .then(|| guess(&self.tracks[i].name, &placed(&self.tracks[i].note_clips)));
                }
                let Some(n) = n.as_mut() else {
                    return;
                };
                ui.horizontal(|ui| {
                    for (v, label) in [
                        (NotationView::Tab, "Tab"),
                        (NotationView::Staff, "Staff"),
                        (NotationView::Drums, "Drums"),
                    ] {
                        ui.selectable_value(&mut n.view, v, label);
                    }
                });
                if n.view == NotationView::Tab {
                    if n.tuning.is_empty() {
                        *n = Notation {
                            view: NotationView::Tab,
                            ..Notation::guitar()
                        };
                    }
                    let shown = tuning_of(n);
                    egui::ComboBox::from_label("Tuning")
                        .selected_text(format!("{} ({})", shown.name, shown.letters()))
                        .show_ui(ui, |ui| {
                            for t in Tuning::presets() {
                                let label = format!("{} ({})", t.name, t.letters());
                                if ui.selectable_label(t.strings == n.tuning, label).clicked() {
                                    n.tuning = t.strings.clone();
                                    n.tuning_name = t.name.clone();
                                }
                            }
                        });
                    ui.add(egui::Slider::new(&mut n.capo, 0..=12).text("Capo"));
                    ui.checkbox(&mut n.staff, "Standard staff above the tab");
                    ui.checkbox(&mut n.rhythm, "Rhythm under the tab");
                    assign = ui
                        .button("Choose strings and frets")
                        .on_hover_text(
                            "Write the shown strings and frets into notes that have none",
                        )
                        .clicked();
                }
                ui.checkbox(&mut mute, "Practice-mute this part, to play it yourself");
            });
        if !open {
            self.notation.window = None;
        }
        if n != self.tracks[i].notation {
            self.set_notation(i, n);
        }
        if mute != self.tracks[i].practice_mute {
            self.set_practice_mute(i, mute);
        }
        if assign {
            let k = self.assign_frets(i);
            self.message = Some(format!("Chose strings and frets for {k} notes."));
        }
    }

    /// Mutes a track for practice, to play the part yourself; the mix
    /// keeps its own mute.
    pub(crate) fn set_practice_mute(&mut self, track: usize, on: bool) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.practice_mute = on;
            let mix = t.heard();
            self.send(daw_engine::engine::Command::SetMix { track, mix });
        }
    }
}

/// What the notation lanes can be written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotationFile {
    MusicXml,
    TextTab,
    /// Pages to print, with the drum legend.
    Pdf,
}

pub(crate) fn notation_bytes(what: NotationFile, score: &Score) -> Result<Vec<u8>, String> {
    Ok(match what {
        NotationFile::MusicXml => daw_notation::musicxml::write(score).into_bytes(),
        NotationFile::TextTab => daw_notation::text_tab::write(score, 80).into_bytes(),
        NotationFile::Pdf => {
            let fonts = daw_notation::render::Fonts::new(MUSIC_FONT_DATA, TEXT_FONT_DATA)
                .map_err(|e| e.to_string())?;
            let pages = layout::layout(score, &layout::LayoutOptions::default());
            daw_notation::render::pdf::pages_to_pdf(&pages, &fonts)
        }
    })
}

/// A track's note clips' notes as they play, placed on the timeline.
pub(crate) fn placed(clips: &[NoteClip]) -> Vec<Note> {
    let mut out: Vec<Note> = (clips.iter())
        .filter(|c| !c.muted)
        .flat_map(NoteClip::placed)
        .collect();
    out.sort_by(|a, b| (a.start, a.pitch, a.id.as_str()).cmp(&(b.start, b.pitch, b.id.as_str())));
    out
}

/// The string each drawn note is shown on, by project note.
fn strings_shown(b: &Built) -> HashMap<String, u8> {
    let mut out = HashMap::new();
    for ((bar, e, ni), id) in &b.ids {
        if let Some(s) = (b.part.bars.get(*bar))
            .and_then(|bar| bar.events.get(*e))
            .and_then(|e| e.notes.get(*ni))
            .and_then(|n| n.string)
        {
            out.entry(id.clone()).or_insert(s);
        }
    }
    out
}

/// `tick` moved to the nearest `step` from its bar's start.
fn snap_tick(map: &TempoMap, tick: i64, step: i64) -> i64 {
    let bar = map.bar_start(map.bar_beat(Ticks(tick.max(0))).bar).0;
    let step = step.max(1);
    (bar + ((tick - bar) as f64 / step as f64).round() as i64 * step).max(0)
}

/// Takes notes starting at `tick` on `string` out of the clips.
fn clear_at(clips: &mut [NoteClip], tick: i64, string: u8) -> bool {
    let mut changed = false;
    for c in clips {
        let before = c.notes.len();
        let at = tick - c.place(0);
        c.notes
            .retain(|n| !(n.start == at && n.string == Some(string)));
        changed |= c.notes.len() != before;
    }
    changed
}

/// The note clip under `tick`, or a new one a bar long from its bar.
fn clip_at<'a>(clips: &'a mut Vec<NoteClip>, map: &TempoMap, tick: i64) -> &'a mut NoteClip {
    if let Some(i) = clips
        .iter()
        .position(|c| !c.muted && c.start <= tick && c.end() > tick)
    {
        return &mut clips[i];
    }
    let bar = map.bar_beat(Ticks(tick.max(0))).bar;
    let (from, to) = (map.bar_start(bar).0, map.bar_start(bar + 1).0);
    let n = clips.len() + 1;
    clips.push(NoteClip::new(
        daw_project::new_id(),
        format!("Tab {n}"),
        from,
        to - from,
    ));
    clips.last_mut().expect("just pushed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_audio_io::SilentInput;
    use daw_engine::time::{Tempo, TimeSignature};
    use egui_kittest::Harness;

    use crate::commands::Cmd;
    use crate::{App, Audio};

    fn map() -> TempoMap {
        TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON)
    }

    fn values(bar: &Bar) -> Vec<(u8, NoteValue, bool)> {
        (bar.events.iter())
            .map(|e| (e.voice, e.duration.value, e.is_rest()))
            .collect()
    }

    #[test]
    fn notes_become_bars_of_chords_and_rests_on_the_tempo_maps_bars() {
        let notes = vec![
            Note::new(0, PPQ, 64, 90),
            Note::new(0, PPQ, 59, 90),
            // A little early, as played: moved to the beat.
            Note::new(PPQ * 2 - 30, PPQ / 2, 67, 90),
        ];
        let b = build(&notes, &Notation::guitar(), &map(), "Gtr", PPQ * 16);
        assert_eq!(b.bars.len(), 4, "up to the end asked for");
        assert_eq!(b.bars[1], (PPQ * 4, PPQ * 8));
        let bar = &b.part.bars[0];
        assert_eq!(bar.events[0].notes.len(), 2, "a chord");
        use NoteValue::*;
        assert_eq!(
            values(bar),
            [
                (1, Quarter, false),
                (1, Quarter, true),
                (1, Eighth, false),
                (1, Eighth, true),
                (1, Quarter, true)
            ]
        );
        assert_eq!(bar.time.map(|t| (t.beats, t.beat_type)), Some((4, 4)));
        assert!(b.part.bars[1].time.is_none(), "only where it changes");
        // An empty bar is a rest.
        assert_eq!(values(&b.part.bars[2]), [(1, Whole, true)]);
        // The tab has strings and frets for every note.
        for e in &bar.events {
            for n in &e.notes {
                assert!(n.string.is_some() && n.fret.is_some());
            }
        }
        assert_eq!(b.ids.len(), 3);
        assert!(b.height > 20.0);
    }

    #[test]
    fn triplets_and_notes_over_the_bar_line() {
        // Three triplet eighths on beat 1, then a note held over the bar.
        let notes = vec![
            Note::new(0, PPQ / 3, 60, 90),
            Note::new(PPQ / 3, PPQ / 3, 62, 90),
            Note::new(PPQ * 2 / 3, PPQ / 3, 64, 90),
            Note::new(PPQ * 3, PPQ * 2, 65, 90),
        ];
        let b = build(&notes, &Notation::staff(), &map(), "Keys", 0);
        let bar = &b.part.bars[0];
        let tuplets = (bar.events.iter())
            .filter(|e| e.duration.tuplet.is_some())
            .count();
        assert_eq!(tuplets, 3);
        let last = bar.events.last().unwrap();
        assert!(last.notes[0].tie, "tied into the next bar");
        let next = &b.part.bars[1].events[0];
        assert_eq!(
            (next.notes[0].pitch, next.duration.value),
            (65, NoteValue::Quarter)
        );
        assert!(!next.notes[0].tie);
        // The ticks add up to whole bars.
        for bar in &b.part.bars {
            let ticks: u32 = (bar.events.iter())
                .filter(|e| e.voice == 1)
                .map(|e| e.duration.ticks())
                .sum();
            assert_eq!(ticks, 3840);
        }
    }

    #[test]
    fn drums_put_feet_in_a_second_voice() {
        let notes: Vec<Note> = (0..4)
            .flat_map(|i| {
                let mut hat = Note::new(i * PPQ, PPQ / 2, 42, 90);
                hat.channel = 9;
                let mut kick = Note::new(i * PPQ, PPQ / 2, 36, 110);
                kick.channel = 9;
                [hat, kick]
            })
            .collect();
        assert_eq!(guess("Kit", &notes).view, NotationView::Drums);
        assert_eq!(guess("", &notes).view, NotationView::Drums);
        let b = build(&notes, &Notation::drums(), &map(), "Drums", 0);
        let bar = &b.part.bars[0];
        let voice = |v: u8| {
            bar.events
                .iter()
                .filter(|e| e.voice == v && !e.is_rest())
                .count()
        };
        assert_eq!((voice(1), voice(2)), (4, 4));
        assert!(matches!(b.part.kind, PartKind::Drums(_)));
    }

    #[test]
    fn positions_the_user_chose_are_kept() {
        let mut n = Note::new(0, PPQ, 64, 90);
        n.string = Some(2);
        n.fret = Some(5);
        let b = build(&[n.clone()], &Notation::guitar(), &map(), "Gtr", 0);
        let shown = &b.part.bars[0].events[0].notes[0];
        assert_eq!(
            (shown.string, shown.fret, shown.locked),
            (Some(2), Some(5), true)
        );
        // A capo moves the fret down; a string that can't sound it is
        // dropped and another chosen.
        let mut capo = Notation::guitar();
        capo.capo = 2;
        let b = build(&[n.clone()], &capo, &map(), "Gtr", 0);
        assert_eq!(b.part.bars[0].events[0].notes[0].fret, Some(3));
        n.pitch = 50;
        let b = build(&[n], &Notation::guitar(), &map(), "Gtr", 0);
        assert_ne!(b.part.bars[0].events[0].notes[0].string, Some(2));
    }

    #[test]
    fn musicxml_out_and_back_keeps_the_notes() {
        let mut notes = vec![
            Note::new(0, PPQ, 64, 90),
            Note::new(PPQ, PPQ / 2, 67, 90),
            Note::new(PPQ * 3 / 2, PPQ / 2, 69, 90),
            Note::new(PPQ * 3, PPQ * 2, 71, 90),
        ];
        notes[1].techniques.push(Technique::HammerOn);
        notes[2].bend = 2.0;
        let b = build(&notes, &Notation::guitar(), &map(), "Gtr", 0);
        let score = Score {
            title: "T".into(),
            composer: String::new(),
            parts: vec![b.part],
        };
        let xml = daw_notation::musicxml::write(&score);
        let back = daw_notation::musicxml::read(&xml).unwrap();
        let got = notes_of(&back.parts[0]);
        let key = |n: &Note| (n.start, n.len, n.pitch);
        assert_eq!(
            got.iter().map(key).collect::<Vec<_>>(),
            notes.iter().map(key).collect::<Vec<_>>()
        );
        assert!(got[1].techniques.contains(&Technique::HammerOn));
        assert_eq!(got[2].bend, 2.0);
        assert!(got.iter().all(|n| n.string.is_some() && n.fret.is_some()));
        assert_eq!(notation_of(&back.parts[0]).view, NotationView::Tab);
        assert_eq!(
            notation_of(&back.parts[0]).tuning,
            Notation::guitar().tuning
        );
        // Text tab and print come out too.
        assert!(notation_bytes(NotationFile::TextTab, &score).unwrap().len() > 50);
        let pdf = notation_bytes(NotationFile::Pdf, &score).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn lengths_fill_spans_with_written_values() {
        let straight = [false; 4];
        let d: Vec<u32> = (lengths(0, 3840, &straight).iter())
            .map(|(_, d)| d.ticks())
            .collect();
        assert_eq!(d, [3840]);
        let d: Vec<u32> = (lengths(PPQ, PPQ * 4, &straight).iter())
            .map(|(_, d)| d.ticks())
            .collect();
        assert_eq!(d.iter().sum::<u32>(), 2880);
        let trip = [false, true, false, false];
        // Over a whole triplet quarter, plain lengths do.
        let d = lengths(0, PPQ * 2, &trip);
        assert_eq!(d.len(), 1, "{d:?}");
        let d = lengths(0, PPQ + PPQ / 3, &trip);
        assert_eq!(d[0].1.ticks(), 960);
        assert_eq!(d[1].1.tuplet.map(|t| t.actual), Some(3));
        let d = lengths(PPQ + PPQ / 3, PPQ * 2, &trip);
        assert!(d.iter().all(|(_, d)| d.tuplet.is_some()));
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 800.0])
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
    fn typing_tab_writes_notes_that_sound_the_fret() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddInstrumentTrack);
        h.state_mut().run(&ctx, Cmd::TabEntry);
        h.run();
        {
            let app = h.state();
            let n = app.tracks[0].notation.as_ref().expect("a tab lane");
            assert_eq!(n.view, NotationView::Tab);
            let e = app.notation.entry.as_ref().expect("typing");
            assert_eq!((e.tick, e.string), (0, 1));
        }
        // 3 on the top string, then along an eighth and 1, 2 (fret 12)
        // on the string below.
        h.key_press(egui::Key::Num3);
        h.run();
        h.key_press(egui::Key::ArrowRight);
        h.key_press(egui::Key::ArrowDown);
        h.run();
        h.key_press(egui::Key::Num1);
        h.key_press(egui::Key::Num2);
        h.run();
        let notes = placed(&h.state().tracks[0].note_clips);
        let got: Vec<_> = notes
            .iter()
            .map(|n| (n.start, n.pitch, n.string, n.fret))
            .collect();
        assert_eq!(
            got,
            [(0, 67, Some(1), Some(3)), (PPQ / 2, 71, Some(2), Some(12))]
        );
        // Shift+Up moves it to the top string, same pitch.
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowUp);
        h.run();
        let n = placed(&h.state().tracks[0].note_clips)[1].clone();
        assert_eq!((n.pitch, n.string, n.fret), (71, Some(1), Some(7)));
        // Delete clears it; Escape stops typing.
        h.key_press(egui::Key::Delete);
        h.run();
        assert_eq!(placed(&h.state().tracks[0].note_clips).len(), 1);
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.state().notation.entry.is_none());
        // The lane is kept in the project.
        let p = h.state().project();
        assert_eq!(p.tracks[0].notation, Some(Notation::guitar()));
    }

    #[test]
    fn lane_commands_show_change_and_take_off_the_lane() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddInstrumentTrack);
        {
            let app = h.state_mut();
            let mut c = NoteClip::new("c".into(), "Riff".into(), 0, PPQ * 4);
            c.notes = vec![Note::new(0, PPQ, 45, 90), Note::new(PPQ, PPQ, 52, 90)];
            app.tracks[0].note_clips.push(c);
        }
        h.state_mut().run(&ctx, Cmd::NotationLane);
        h.run();
        assert_eq!(h.state().tracks[0].notation, Some(Notation::guitar()));
        assert!(
            h.state()
                .notation
                .built
                .contains_key(&h.state().tracks[0].key)
        );
        // The assigner's choice goes into the notes.
        h.state_mut().run(&ctx, Cmd::AssignFrets);
        let notes = &h.state().tracks[0].note_clips[0].notes;
        assert!(notes.iter().all(|n| n.string.is_some() && n.fret.is_some()));
        h.state_mut().run(&ctx, Cmd::NotationView);
        assert_eq!(
            h.state().tracks[0].notation.as_ref().unwrap().view,
            NotationView::Staff
        );
        h.state_mut().run(&ctx, Cmd::PracticeMutePart);
        assert!(h.state().tracks[0].practice_mute);
        h.state_mut().run(&ctx, Cmd::ContinuousScroll);
        assert!(h.state().timeline.continuous);
        h.run();
        h.state_mut().run(&ctx, Cmd::NotationLane);
        h.run();
        assert_eq!(h.state().tracks[0].notation, None);
        assert!(h.state().notation.built.is_empty());
    }

    #[test]
    fn musicxml_import_makes_a_track_per_part() {
        let mut h = harness();
        let mut drum = Note::new(0, PPQ, 38, 100);
        drum.channel = DRUM_CHANNEL;
        drum.articulation = Some(Articulation::Accent);
        let parts = vec![
            build(
                &[Note::new(0, PPQ, 40, 90)],
                &Notation::bass(),
                &map(),
                "Bass",
                PPQ * 8,
            )
            .part,
            build(&[drum], &Notation::drums(), &map(), "Drums", PPQ * 8).part,
        ];
        let mut score = Score {
            title: String::new(),
            composer: String::new(),
            parts,
        };
        score.parts[0].bars[0].tempo = Some(90.0);
        score.parts[0].bars[1].time = Some(TimeSig {
            beats: 3,
            beat_type: 4,
        });
        let xml = daw_notation::musicxml::write(&score);
        let app = h.state_mut();
        assert_eq!(app.import_musicxml(&xml, "song"), Ok(2));
        assert_eq!(app.tracks.len(), 2);
        assert_eq!(app.tracks[0].name, "Bass");
        assert_eq!(app.tracks[0].notation, Some(Notation::bass()));
        assert_eq!(
            app.tracks[1].notation.as_ref().map(|n| n.view),
            Some(NotationView::Drums)
        );
        let d = &app.tracks[1].note_clips[0].notes[0];
        assert_eq!(
            (d.pitch, d.channel, d.articulation),
            (38, 9, Some(Articulation::Accent))
        );
        // The first part's tempo and time signatures became the song's.
        assert_eq!(app.tempo_bpm, 90.0);
        assert_eq!(app.time.meters.len(), 1);
        assert_eq!((app.time.meters[0].bar, app.time.meters[0].beats), (2, 3));
        h.run();
        // The score out again has both parts.
        assert_eq!(h.state().notation_score().parts.len(), 2);
    }
}
