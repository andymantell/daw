// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading and writing MusicXML 4.0 (partwise), including tab parts (a TAB
//! clef with string tunings and capo, string and fret on each note) and
//! percussion parts (unpitched notes tied to instruments with key numbers).
//!
//! Techniques MusicXML has elements for use them (hammer-on, pull-off,
//! slide, bend and release, harmonics, tapping, accents, tremolo, the wavy
//! line for vibrato, open and stopped for hi-hats, grace notes for flams
//! and drags). The rest (palm mute, let ring, slap, pop, choke, rimshot,
//! cross-stick) are written as `other-technical` with their name, which is
//! the format's way to carry techniques it has no element for.

use std::collections::HashMap;
use std::fmt::Write;

use roxmltree::{Document, Node};

use crate::drums::{DrumMap, Notehead};
use crate::layout::spell;
use crate::score::{
    Bar, ChordDiagram, ChordSymbol, Clef, Duration, Event, Note, NoteValue, Part, PartKind, Score,
    Strum, TICKS_PER_QUARTER, TabSetup, Technique, TimeSig, Tuplet,
};
use crate::tuning::Tuning;

const STEPS: [&str; 7] = ["C", "D", "E", "F", "G", "A", "B"];
const STEP_SEMITONES: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// Techniques written as `other-technical` text.
const OTHER_TECHNICAL: [(Technique, &str); 7] = [
    (Technique::PalmMute, "palm mute"),
    (Technique::LetRing, "let ring"),
    (Technique::Slap, "slap"),
    (Technique::Pop, "pop"),
    (Technique::Choke, "choke"),
    (Technique::Rimshot, "rimshot"),
    (Technique::CrossStick, "cross stick"),
];

const CHORD_KINDS: [(&str, &str); 14] = [
    ("", "major"),
    ("m", "minor"),
    ("7", "dominant"),
    ("maj7", "major-seventh"),
    ("m7", "minor-seventh"),
    ("dim", "diminished"),
    ("dim7", "diminished-seventh"),
    ("aug", "augmented"),
    ("m7b5", "half-diminished"),
    ("sus2", "suspended-second"),
    ("sus4", "suspended-fourth"),
    ("5", "power"),
    ("6", "major-sixth"),
    ("m6", "minor-sixth"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError(pub String);

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MusicXML: {}", self.0)
    }
}

impl std::error::Error for ReadError {}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Write a score as a MusicXML 4.0 partwise document.
pub fn write(score: &Score) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    s.push_str("<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n");
    s.push_str("<score-partwise version=\"4.0\">\n");
    if !score.title.is_empty() {
        let _ = writeln!(
            s,
            "  <work><work-title>{}</work-title></work>",
            esc(&score.title)
        );
    }
    s.push_str("  <identification>\n");
    if !score.composer.is_empty() {
        let _ = writeln!(
            s,
            "    <creator type=\"composer\">{}</creator>",
            esc(&score.composer)
        );
    }
    s.push_str("    <encoding><software>daw-notation</software><encoding-date>2026-01-01</encoding-date></encoding>\n");
    s.push_str("  </identification>\n  <part-list>\n");
    for (i, part) in score.parts.iter().enumerate() {
        let id = format!("P{}", i + 1);
        let _ = writeln!(s, "    <score-part id=\"{id}\">");
        let _ = writeln!(s, "      <part-name>{}</part-name>", esc(&part.name));
        if let PartKind::Drums(map) = &part.kind {
            for key in used_keys(part) {
                let name = map
                    .entry(key)
                    .map_or_else(|| format!("Note {key}"), |e| e.name.clone());
                let _ = writeln!(
                    s,
                    "      <score-instrument id=\"{id}-I{key}\"><instrument-name>{}</instrument-name></score-instrument>",
                    esc(&name)
                );
            }
            for key in used_keys(part) {
                let _ = writeln!(
                    s,
                    "      <midi-instrument id=\"{id}-I{key}\"><midi-channel>10</midi-channel><midi-unpitched>{}</midi-unpitched></midi-instrument>",
                    u32::from(key) + 1
                );
            }
        }
        s.push_str("    </score-part>\n");
    }
    s.push_str("  </part-list>\n");
    for (i, part) in score.parts.iter().enumerate() {
        write_part(&mut s, part, &format!("P{}", i + 1));
    }
    s.push_str("</score-partwise>\n");
    s
}

fn used_keys(part: &Part) -> Vec<u8> {
    let mut keys: Vec<u8> = part
        .bars
        .iter()
        .flat_map(|b| &b.events)
        .flat_map(|e| &e.notes)
        .map(|n| n.pitch)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

fn write_part(s: &mut String, part: &Part, id: &str) {
    let _ = writeln!(s, "  <part id=\"{id}\">");
    let times = part.time_signatures();
    let two_staves = matches!(&part.kind, PartKind::Tab(t) if t.show_staff);
    let mut key = 0;
    for (b, bar) in part.bars.iter().enumerate() {
        let _ = writeln!(s, "    <measure number=\"{}\">", b + 1);
        if let Some(k) = bar.key {
            key = k;
        }
        if b == 0 || bar.time.is_some() || bar.key.is_some() {
            s.push_str("      <attributes>\n");
            if b == 0 {
                let _ = writeln!(s, "        <divisions>{TICKS_PER_QUARTER}</divisions>");
            }
            if b == 0 || bar.key.is_some() {
                let _ = writeln!(s, "        <key><fifths>{key}</fifths></key>");
            }
            if b == 0 || bar.time.is_some() {
                let t = times[b];
                let _ = writeln!(
                    s,
                    "        <time><beats>{}</beats><beat-type>{}</beat-type></time>",
                    t.beats, t.beat_type
                );
            }
            if b == 0 {
                write_clefs(s, part, two_staves);
            }
            s.push_str("      </attributes>\n");
        }
        if let Some(tempo) = bar.tempo {
            let _ = writeln!(
                s,
                "      <direction placement=\"above\"><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>{tempo}</per-minute></metronome></direction-type><sound tempo=\"{tempo}\"/></direction>"
            );
        }
        let staves: &[Option<u8>] = if two_staves {
            &[Some(1), Some(2)]
        } else {
            &[None]
        };
        let mut voices: Vec<u8> = bar.events.iter().map(|e| e.voice).collect();
        voices.sort_unstable();
        voices.dedup();
        let mut last_len: Option<u32> = None;
        for &staff in staves {
            for &v in &voices {
                if let Some(len) = last_len {
                    let _ = writeln!(s, "      <backup><duration>{len}</duration></backup>");
                }
                write_voice(s, part, bar, b, v, staff, id, last_len.is_none());
                last_len = Some(
                    bar.events
                        .iter()
                        .filter(|e| e.voice == v)
                        .map(|e| e.duration.ticks())
                        .sum(),
                );
            }
        }
        if b + 1 == part.bars.len() {
            s.push_str(
                "      <barline location=\"right\"><bar-style>light-heavy</bar-style></barline>\n",
            );
        }
        s.push_str("    </measure>\n");
    }
    s.push_str("  </part>\n");
}

fn write_clefs(s: &mut String, part: &Part, two_staves: bool) {
    match &part.kind {
        PartKind::Tab(setup) => {
            let (sign, line) = if setup.tuning.is_bass() {
                ("F", 4)
            } else {
                ("G", 2)
            };
            let tab_staff = if two_staves {
                s.push_str("        <staves>2</staves>\n");
                let _ = writeln!(
                    s,
                    "        <clef number=\"1\"><sign>{sign}</sign><line>{line}</line><clef-octave-change>-1</clef-octave-change></clef>"
                );
                " number=\"2\""
            } else {
                ""
            };
            let _ = writeln!(
                s,
                "        <clef{tab_staff}><sign>TAB</sign><line>5</line></clef>"
            );
            let _ = writeln!(s, "        <staff-details{tab_staff}>");
            let _ = writeln!(
                s,
                "          <staff-lines>{}</staff-lines>",
                setup.tuning.string_count()
            );
            // Staff lines count from the bottom, so line 1 is the lowest string.
            for (line, &pitch) in setup.tuning.strings.iter().rev().enumerate() {
                let (step, alter) = spell(i32::from(pitch), 0);
                let alter = if alter != 0 {
                    format!("<tuning-alter>{alter}</tuning-alter>")
                } else {
                    String::new()
                };
                let _ = writeln!(
                    s,
                    "          <staff-tuning line=\"{}\"><tuning-step>{}</tuning-step>{alter}<tuning-octave>{}</tuning-octave></staff-tuning>",
                    line + 1,
                    STEPS[step.rem_euclid(7) as usize],
                    step.div_euclid(7)
                );
            }
            if setup.capo > 0 {
                let _ = writeln!(s, "          <capo>{}</capo>", setup.capo);
            }
            s.push_str("        </staff-details>\n");
        }
        PartKind::Staff { clef } => {
            let (sign, line, change) = match clef {
                Clef::Treble => ("G", 2, 0),
                Clef::Treble8vb => ("G", 2, -1),
                Clef::Bass => ("F", 4, 0),
                Clef::Bass8vb => ("F", 4, -1),
            };
            let change = if change != 0 {
                format!("<clef-octave-change>{change}</clef-octave-change>")
            } else {
                String::new()
            };
            let _ = writeln!(
                s,
                "        <clef><sign>{sign}</sign><line>{line}</line>{change}</clef>"
            );
        }
        PartKind::Drums(_) => s.push_str("        <clef><sign>percussion</sign></clef>\n"),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_voice(
    s: &mut String,
    part: &Part,
    bar: &Bar,
    b: usize,
    v: u8,
    staff: Option<u8>,
    id: &str,
    harmonies: bool,
) {
    let tab = matches!(part.kind, PartKind::Tab(_)) && staff != Some(1);
    let voice_no = match staff {
        Some(2) => v + 4,
        _ => v,
    };
    let mut tick = 0;
    let mut chords: Vec<&ChordSymbol> = if harmonies {
        bar.chords.iter().collect()
    } else {
        Vec::new()
    };
    chords.sort_by_key(|c| c.tick);
    let events: Vec<(usize, &Event)> = bar
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.voice == v)
        .collect();
    // Notes that end a tie, hammer-on, pull-off or slide.
    let prev_of = |k: usize| -> Option<&Event> {
        if k > 0 {
            Some(events[k - 1].1)
        } else {
            let prev = part.bars.get(b.checked_sub(1)?)?;
            prev.events.iter().rfind(|e| e.voice == v)
        }
    };
    for (k, (_, e)) in events.iter().enumerate() {
        let len = e.duration.ticks();
        while let Some(c) = chords.first().filter(|c| c.tick < tick + len) {
            write_harmony(s, c, c.tick.saturating_sub(tick));
            chords.remove(0);
        }
        let prev = prev_of(k);
        if e.is_rest() {
            s.push_str("      <note><rest/>");
            let _ = write!(s, "<duration>{len}</duration><voice>{voice_no}</voice>");
            write_type(s, e.duration);
            if let Some(st) = staff {
                let _ = write!(s, "<staff>{st}</staff>");
            }
            s.push_str("</note>\n");
        }
        for (n, note) in e.notes.iter().enumerate() {
            // Grace notes before the first note of a chord for flams and drags.
            let graces = if note.has(&Technique::Drag) {
                2
            } else if note.has(&Technique::Flam) {
                1
            } else {
                0
            };
            for _ in 0..graces {
                s.push_str("      <note><grace slash=\"yes\"/>");
                write_pitch(s, part, note.pitch);
                let _ = write!(
                    s,
                    "{}<voice>{voice_no}</voice><type>eighth</type>",
                    instrument(part, id, note.pitch)
                );
                if let Some(st) = staff {
                    let _ = write!(s, "<staff>{st}</staff>");
                }
                s.push_str("</note>\n");
            }
            s.push_str("      <note>");
            if n > 0 {
                s.push_str("<chord/>");
            }
            write_pitch(s, part, note.pitch);
            let _ = write!(s, "<duration>{len}</duration>");
            let tie_stop =
                prev.is_some_and(|p| p.notes.iter().any(|m| m.tie && m.pitch == note.pitch));
            if tie_stop {
                s.push_str("<tie type=\"stop\"/>");
            }
            if note.tie {
                s.push_str("<tie type=\"start\"/>");
            }
            s.push_str(&instrument(part, id, note.pitch));
            let _ = write!(s, "<voice>{voice_no}</voice>");
            write_type(s, e.duration);
            if matches!(part.kind, PartKind::Drums(_)) {
                let _ = write!(s, "<stem>{}</stem>", if v % 2 == 1 { "up" } else { "down" });
            }
            let head = notehead(part, note);
            let ghost = note.has(&Technique::Ghost);
            if head != Notehead::Normal || ghost {
                let paren = if ghost { " parentheses=\"yes\"" } else { "" };
                let _ = write!(s, "<notehead{paren}>{}</notehead>", head.musicxml());
            }
            if let Some(st) = staff {
                let _ = write!(s, "<staff>{st}</staff>");
            }
            write_notations(s, note, e, prev, tie_stop, tab);
            s.push_str("</note>\n");
        }
        tick += len;
    }
    for c in chords {
        write_harmony(s, c, 0);
    }
}

fn instrument(part: &Part, id: &str, key: u8) -> String {
    match part.kind {
        PartKind::Drums(_) => format!("<instrument id=\"{id}-I{key}\"/>"),
        _ => String::new(),
    }
}

fn notehead(part: &Part, note: &Note) -> Notehead {
    if note.has(&Technique::Dead) {
        return Notehead::X;
    }
    match &part.kind {
        PartKind::Drums(map) => {
            if note.has(&Technique::CrossStick) {
                Notehead::CircleX
            } else if note.has(&Technique::Rimshot) {
                Notehead::Slashed
            } else {
                map.look_up(note.pitch).1
            }
        }
        _ => Notehead::Normal,
    }
}

fn write_pitch(s: &mut String, part: &Part, pitch: u8) {
    match &part.kind {
        PartKind::Drums(map) => {
            let place = i32::from(map.look_up(pitch).0);
            let step = 4 * 7 + 2 + place;
            let _ = write!(
                s,
                "<unpitched><display-step>{}</display-step><display-octave>{}</display-octave></unpitched>",
                STEPS[step.rem_euclid(7) as usize],
                step.div_euclid(7)
            );
        }
        _ => {
            let (step, alter) = spell(i32::from(pitch), 0);
            let alter = if alter != 0 {
                format!("<alter>{alter}</alter>")
            } else {
                String::new()
            };
            let _ = write!(
                s,
                "<pitch><step>{}</step>{alter}<octave>{}</octave></pitch>",
                STEPS[step.rem_euclid(7) as usize],
                step.div_euclid(7)
            );
        }
    }
}

fn write_type(s: &mut String, d: Duration) {
    let _ = write!(s, "<type>{}</type>", d.value.name());
    for _ in 0..d.dots {
        s.push_str("<dot/>");
    }
    if let Some(t) = d.tuplet {
        let _ = write!(
            s,
            "<time-modification><actual-notes>{}</actual-notes><normal-notes>{}</normal-notes></time-modification>",
            t.actual, t.normal
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn write_notations(
    s: &mut String,
    note: &Note,
    e: &Event,
    prev: Option<&Event>,
    tie_stop: bool,
    tab: bool,
) {
    let mut notations = String::new();
    if tie_stop {
        notations.push_str("<tied type=\"stop\"/>");
    }
    if note.tie {
        notations.push_str("<tied type=\"start\"/>");
    }
    let prev_on_string = |t: &Technique| {
        prev.and_then(|p| {
            p.notes
                .iter()
                .find(|m| m.string.is_some() && m.string == note.string)
        })
        .is_some_and(|m| m.has(t))
    };
    if prev_on_string(&Technique::Slide) {
        notations.push_str("<slide type=\"stop\"/>");
    }
    if note.has(&Technique::Slide) {
        notations.push_str("<slide type=\"start\" line-type=\"solid\"/>");
    }
    let mut technical = String::new();
    if tab && let (Some(st), Some(f)) = (note.string, note.fret) {
        let _ = write!(technical, "<string>{st}</string><fret>{f}</fret>");
    }
    for (t, name, text) in [
        (Technique::HammerOn, "hammer-on", "H"),
        (Technique::PullOff, "pull-off", "P"),
    ] {
        if prev_on_string(&t) {
            let _ = write!(technical, "<{name} type=\"stop\"/>");
        }
        if note.has(&t) {
            let _ = write!(technical, "<{name} type=\"start\">{text}</{name}>");
        }
    }
    for t in &note.techniques {
        match t {
            Technique::Bend { semitones, release } => {
                let _ = write!(
                    technical,
                    "<bend><bend-alter>{semitones}</bend-alter></bend>"
                );
                if *release {
                    let _ = write!(
                        technical,
                        "<bend><bend-alter>-{semitones}</bend-alter><release/></bend>"
                    );
                }
            }
            Technique::NaturalHarmonic => technical.push_str("<harmonic><natural/></harmonic>"),
            Technique::ArtificialHarmonic => {
                technical.push_str("<harmonic><artificial/></harmonic>")
            }
            Technique::Tap => technical.push_str("<tap/>"),
            Technique::Open => technical.push_str("<open/>"),
            Technique::Closed => technical.push_str("<stopped/>"),
            _ => {}
        }
    }
    for (t, name) in &OTHER_TECHNICAL {
        if note.has(t) {
            let _ = write!(technical, "<other-technical>{name}</other-technical>");
        }
    }
    let mut ornaments = String::new();
    if note.has(&Technique::Vibrato) {
        ornaments.push_str("<wavy-line type=\"start\"/><wavy-line type=\"stop\"/>");
    }
    if note.has(&Technique::TremoloPicking) || note.has(&Technique::Roll) {
        ornaments.push_str("<tremolo type=\"single\">3</tremolo>");
    }
    if !ornaments.is_empty() {
        let _ = write!(notations, "<ornaments>{ornaments}</ornaments>");
    }
    if !technical.is_empty() {
        let _ = write!(notations, "<technical>{technical}</technical>");
    }
    if note.has(&Technique::Accent) {
        notations.push_str("<articulations><accent/></articulations>");
    }
    // A downstroke plays the low strings first: an arpeggio going up.
    if let Some(dir) = e.strum {
        let d = if dir == Strum::Down { "up" } else { "down" };
        let _ = write!(notations, "<arpeggiate direction=\"{d}\"/>");
    }
    if !notations.is_empty() {
        let _ = write!(s, "<notations>{notations}</notations>");
    }
}

fn write_harmony(s: &mut String, c: &ChordSymbol, offset: u32) {
    let mut chars = c.name.chars();
    let Some(root @ 'A'..='G') = chars.next() else {
        let _ = writeln!(
            s,
            "      <harmony><root><root-step>C</root-step></root><kind text=\"{}\">none</kind></harmony>",
            esc(&c.name)
        );
        return;
    };
    let rest: String = chars.collect();
    let (alter, suffix) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest.as_str()),
    };
    let kind = CHORD_KINDS
        .iter()
        .find(|(t, _)| *t == suffix)
        .map_or("other", |(_, k)| k);
    let _ = write!(s, "      <harmony><root><root-step>{root}</root-step>");
    if alter != 0 {
        let _ = write!(s, "<root-alter>{alter}</root-alter>");
    }
    let _ = write!(s, "</root><kind text=\"{}\">{kind}</kind>", esc(suffix));
    if let Some(d) = &c.diagram {
        let _ = write!(
            s,
            "<frame><frame-strings>{}</frame-strings><frame-frets>4</frame-frets>",
            d.frets.len()
        );
        if d.first_fret > 1 {
            let _ = write!(s, "<first-fret>{}</first-fret>", d.first_fret);
        }
        for (i, f) in d.frets.iter().enumerate() {
            if let Some(f) = f {
                let _ = write!(
                    s,
                    "<frame-note><string>{}</string><fret>{f}</fret></frame-note>",
                    i + 1
                );
            }
        }
        s.push_str("</frame>");
    }
    if offset > 0 {
        let _ = write!(s, "<offset>{offset}</offset>");
    }
    s.push_str("</harmony>\n");
}

// Reading.

fn child<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text_of<'a>(n: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    child(n, name).and_then(|c| c.text()).map(str::trim)
}

fn num<T: std::str::FromStr>(n: Node<'_, '_>, name: &str) -> Option<T> {
    text_of(n, name).and_then(|t| t.parse().ok())
}

struct PartState {
    divisions: u32,
    clef_sign: Option<String>,
    clef_change: i32,
    transpose: i32,
    staves: u8,
    tab_staff: Option<u8>,
    tuning: Vec<(u8, u8)>,
    capo: u8,
}

/// Read a MusicXML partwise document. Timewise documents, compressed
/// files and multi-measure layout details are not read.
pub fn read(xml: &str) -> Result<Score, ReadError> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = Document::parse_with_options(xml, opts).map_err(|e| ReadError(e.to_string()))?;
    let root = doc.root_element();
    if !root.has_tag_name("score-partwise") {
        return Err(ReadError(format!(
            "<{}> is not a partwise score",
            root.tag_name().name()
        )));
    }
    let title = child(root, "work")
        .and_then(|w| text_of(w, "work-title"))
        .or_else(|| text_of(root, "movement-title"))
        .unwrap_or("")
        .to_owned();
    let composer = child(root, "identification")
        .and_then(|i| {
            i.children()
                .find(|c| c.has_tag_name("creator") && c.attribute("type") == Some("composer"))
        })
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_owned();

    let mut names = HashMap::new();
    let mut instruments: HashMap<String, u8> = HashMap::new();
    if let Some(list) = child(root, "part-list") {
        for sp in list.children().filter(|c| c.has_tag_name("score-part")) {
            let id = sp.attribute("id").unwrap_or("").to_owned();
            names.insert(id, text_of(sp, "part-name").unwrap_or("").to_owned());
            for mi in sp.children().filter(|c| c.has_tag_name("midi-instrument")) {
                if let (Some(iid), Some(u)) = (mi.attribute("id"), num::<u8>(mi, "midi-unpitched"))
                {
                    instruments.insert(iid.to_owned(), u.saturating_sub(1));
                }
            }
        }
    }

    let mut parts = Vec::new();
    for p in root.children().filter(|c| c.has_tag_name("part")) {
        let id = p.attribute("id").unwrap_or("");
        parts.push(read_part(
            p,
            names.get(id).cloned().unwrap_or_default(),
            &instruments,
        )?);
    }
    Ok(Score {
        title,
        composer,
        parts,
    })
}

fn read_part(
    p: Node<'_, '_>,
    name: String,
    instruments: &HashMap<String, u8>,
) -> Result<Part, ReadError> {
    let mut st = PartState {
        divisions: 1,
        clef_sign: None,
        clef_change: 0,
        transpose: 0,
        staves: 1,
        tab_staff: None,
        tuning: Vec::new(),
        capo: 0,
    };
    let map = DrumMap::general_midi();
    let mut bars = Vec::new();
    let mut kind: Option<PartKind> = None;

    for m in p.children().filter(|c| c.has_tag_name("measure")) {
        let mut bar = Bar::default();
        // Per voice: how far its events reach, so gaps become rests.
        let mut reach: HashMap<u8, u32> = HashMap::new();
        let mut cursor: u32 = 0;
        let mut last_start: u32 = 0;
        let mut graces: u8 = 0;
        for el in m.children().filter(Node::is_element) {
            match el.tag_name().name() {
                "attributes" => read_attributes(el, &mut st, &mut bar),
                "direction" => {
                    if let Some(t) = el
                        .descendants()
                        .find(|d| d.has_tag_name("sound"))
                        .and_then(|s| s.attribute("tempo"))
                        .and_then(|t| t.parse().ok())
                    {
                        bar.tempo = Some(t);
                    }
                }
                "sound" => {
                    if let Some(t) = el.attribute("tempo").and_then(|t| t.parse().ok()) {
                        bar.tempo = Some(t);
                    }
                }
                "backup" => {
                    cursor =
                        cursor.saturating_sub(ticks(num(el, "duration").unwrap_or(0), st.divisions))
                }
                "forward" => cursor += ticks(num(el, "duration").unwrap_or(0), st.divisions),
                "harmony" => {
                    let offset = num::<i64>(el, "offset").unwrap_or(0);
                    let at = (i64::from(cursor) + i64::from(ticks_i(offset, st.divisions))).max(0)
                        as u32;
                    if let Some(c) = read_harmony(el, at) {
                        bar.chords.push(c);
                    }
                }
                "note" => {
                    if kind.is_none() {
                        kind = Some(part_kind(&st, &map));
                    }
                    let staff: u8 = num(el, "staff").unwrap_or(1);
                    if st.staves > 1 && st.tab_staff.is_some_and(|t| t != staff) {
                        // The tab staff carries the same notes with positions.
                        if child(el, "chord").is_none() && child(el, "grace").is_none() {
                            cursor += ticks(num(el, "duration").unwrap_or(0), st.divisions);
                        }
                        continue;
                    }
                    if child(el, "grace").is_some() {
                        graces = graces.saturating_add(1);
                        continue;
                    }
                    let is_chord = child(el, "chord").is_some();
                    let len = ticks(num(el, "duration").unwrap_or(0), st.divisions);
                    let voice = ((num::<u8>(el, "voice").unwrap_or(1).max(1) - 1) % 4) + 1;
                    let start = if is_chord { last_start } else { cursor };
                    let duration = read_duration(el, len);
                    let note = read_note(
                        el,
                        &st,
                        instruments,
                        &map,
                        kind.as_ref(),
                        std::mem::take(&mut graces),
                    );
                    if is_chord {
                        if let Some(e) = bar.events.iter_mut().rev().find(|e| e.voice == voice)
                            && let Some(n) = note
                        {
                            e.notes.push(n);
                        }
                        continue;
                    }
                    let r = reach.entry(voice).or_insert(0);
                    if start > *r {
                        bar.events
                            .push(Event::rest(voice, Duration::from_ticks(start - *r)));
                    }
                    *r = start + duration.ticks();
                    let strum = el
                        .descendants()
                        .find(|d| d.has_tag_name("arpeggiate"))
                        .map(|a| {
                            if a.attribute("direction") == Some("down") {
                                Strum::Up
                            } else {
                                Strum::Down
                            }
                        });
                    bar.events.push(Event {
                        voice,
                        duration,
                        notes: note.into_iter().collect(),
                        strum,
                    });
                    last_start = start;
                    cursor = start + len;
                }
                _ => {}
            }
        }
        bars.push(bar);
    }
    let kind = kind.unwrap_or_else(|| part_kind(&st, &map));
    Ok(Part { name, kind, bars })
}

fn ticks(duration: u32, divisions: u32) -> u32 {
    (u64::from(duration) * u64::from(TICKS_PER_QUARTER) / u64::from(divisions.max(1))) as u32
}

fn ticks_i(duration: i64, divisions: u32) -> i32 {
    (duration * i64::from(TICKS_PER_QUARTER) / i64::from(divisions.max(1))) as i32
}

fn read_attributes(el: Node<'_, '_>, st: &mut PartState, bar: &mut Bar) {
    if let Some(d) = num(el, "divisions") {
        st.divisions = d;
    }
    if let Some(k) = child(el, "key").and_then(|k| num::<i8>(k, "fifths")) {
        bar.key = Some(k);
    }
    if let Some(t) = child(el, "time")
        && let (Some(beats), Some(beat_type)) = (num(t, "beats"), num(t, "beat-type"))
    {
        bar.time = Some(TimeSig { beats, beat_type });
    }
    if let Some(n) = num(el, "staves") {
        st.staves = n;
    }
    for clef in el.children().filter(|c| c.has_tag_name("clef")) {
        let number: u8 = clef
            .attribute("number")
            .and_then(|n| n.parse().ok())
            .unwrap_or(1);
        let sign = text_of(clef, "sign").unwrap_or("G");
        if sign == "TAB" {
            st.tab_staff = Some(number);
        } else if st.clef_sign.is_none() || number == 1 {
            st.clef_sign = Some(sign.to_owned());
            st.clef_change = num(clef, "clef-octave-change").unwrap_or(0);
        }
    }
    if let Some(t) = child(el, "transpose") {
        st.transpose = num::<i32>(t, "chromatic").unwrap_or(0)
            + 12 * num::<i32>(t, "octave-change").unwrap_or(0);
    }
    for sd in el.children().filter(|c| c.has_tag_name("staff-details")) {
        let mut tuning = Vec::new();
        for t in sd.children().filter(|c| c.has_tag_name("staff-tuning")) {
            let line: u8 = t
                .attribute("line")
                .and_then(|l| l.parse().ok())
                .unwrap_or(1);
            let step = text_of(t, "tuning-step")
                .and_then(|s| STEPS.iter().position(|&x| x == s))
                .unwrap_or(0);
            let alter: f32 = num(t, "tuning-alter").unwrap_or(0.0);
            let octave: i32 = num(t, "tuning-octave").unwrap_or(3);
            let pitch = (octave + 1) * 12 + STEP_SEMITONES[step] + alter.round() as i32;
            tuning.push((line, pitch.clamp(0, 127) as u8));
        }
        if !tuning.is_empty() {
            tuning.sort_by_key(|&(l, _)| std::cmp::Reverse(l));
            st.tuning = tuning;
            if st.tab_staff.is_none() {
                st.tab_staff = Some(
                    sd.attribute("number")
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(1),
                );
            }
        }
        if let Some(c) = num(sd, "capo") {
            st.capo = c;
        }
    }
}

fn part_kind(st: &PartState, map: &DrumMap) -> PartKind {
    if !st.tuning.is_empty() {
        let strings: Vec<u8> = st.tuning.iter().map(|&(_, p)| p).collect();
        let tuning = Tuning::presets()
            .into_iter()
            .find(|t| t.strings == strings)
            .unwrap_or_else(|| Tuning::new("Custom", &strings));
        return PartKind::Tab(TabSetup {
            tuning,
            capo: st.capo,
            show_staff: st.staves > 1,
            show_rhythm: true,
        });
    }
    match (st.clef_sign.as_deref(), st.clef_change) {
        (Some("percussion"), _) => PartKind::Drums(map.clone()),
        (Some("F"), -1) => PartKind::Staff {
            clef: Clef::Bass8vb,
        },
        (Some("F"), _) => PartKind::Staff { clef: Clef::Bass },
        (Some("G"), -1) => PartKind::Staff {
            clef: Clef::Treble8vb,
        },
        _ => PartKind::Staff { clef: Clef::Treble },
    }
}

fn read_duration(el: Node<'_, '_>, len: u32) -> Duration {
    let Some(value) = text_of(el, "type").and_then(NoteValue::from_name) else {
        return Duration::from_ticks(len);
    };
    let dots = el
        .children()
        .filter(|c| c.has_tag_name("dot"))
        .count()
        .min(3) as u8;
    let tuplet = child(el, "time-modification").and_then(|t| {
        Some(Tuplet {
            actual: num(t, "actual-notes")?,
            normal: num(t, "normal-notes")?,
        })
    });
    Duration {
        value,
        dots,
        tuplet,
    }
}

fn read_note(
    el: Node<'_, '_>,
    st: &PartState,
    instruments: &HashMap<String, u8>,
    map: &DrumMap,
    kind: Option<&PartKind>,
    graces: u8,
) -> Option<Note> {
    if child(el, "rest").is_some() {
        return None;
    }
    let head = child(el, "notehead");
    let head_kind = head
        .and_then(|h| h.text())
        .map_or(Notehead::Normal, |t| Notehead::from_musicxml(t.trim()));
    let pitch = if let Some(p) = child(el, "pitch") {
        let step = text_of(p, "step").and_then(|s| STEPS.iter().position(|&x| x == s))?;
        let alter: f32 = num(p, "alter").unwrap_or(0.0);
        let octave: i32 = num(p, "octave")?;
        (octave + 1) * 12 + STEP_SEMITONES[step] + alter.round() as i32 + st.transpose
    } else {
        let u = child(el, "unpitched")?;
        let from_instrument = child(el, "instrument")
            .and_then(|i| i.attribute("id"))
            .and_then(|id| instruments.get(id).copied());
        match from_instrument {
            Some(k) => i32::from(k),
            None => {
                let step = text_of(u, "display-step")
                    .and_then(|s| STEPS.iter().position(|&x| x == s))
                    .unwrap_or(0);
                let octave: i32 = num(u, "display-octave").unwrap_or(4);
                let place = octave * 7 + step as i32 - (4 * 7 + 2);
                i32::from(
                    map.key_at(place.clamp(-20, 20) as i8, head_kind)
                        .unwrap_or(38),
                )
            }
        }
    };
    let mut note = Note::new(pitch.clamp(0, 127) as u8);
    note.tie = el
        .children()
        .any(|c| c.has_tag_name("tie") && c.attribute("type") == Some("start"));
    let drums = matches!(kind, Some(PartKind::Drums(_)));
    if head_kind == Notehead::X && !drums {
        note.techniques.push(Technique::Dead);
    }
    if head.and_then(|h| h.attribute("parentheses")) == Some("yes") {
        note.techniques.push(Technique::Ghost);
    }
    match graces {
        0 => {}
        1 => note.techniques.push(Technique::Flam),
        _ => note.techniques.push(Technique::Drag),
    }
    for n in el.children().filter(|c| c.has_tag_name("notations")) {
        for t in n.children().filter(Node::is_element) {
            match t.tag_name().name() {
                "slide" | "glissando" if t.attribute("type") == Some("start") => {
                    note.techniques.push(Technique::Slide)
                }
                "technical" => read_technical(t, &mut note),
                "ornaments" => {
                    if t.children().any(|o| {
                        o.has_tag_name("wavy-line") && o.attribute("type") == Some("start")
                    }) {
                        note.techniques.push(Technique::Vibrato);
                    }
                    if t.children().any(|o| o.has_tag_name("tremolo")) {
                        note.techniques.push(if drums {
                            Technique::Roll
                        } else {
                            Technique::TremoloPicking
                        });
                    }
                }
                "articulations"
                    if t.children()
                        .any(|a| a.has_tag_name("accent") || a.has_tag_name("strong-accent")) =>
                {
                    note.techniques.push(Technique::Accent);
                }
                _ => {}
            }
        }
    }
    if drums && note.has(&Technique::Open) && note.pitch == 46 {
        // Open is implied by the open hi-hat key.
        note.techniques.retain(|t| *t != Technique::Open);
    }
    Some(note)
}

fn read_technical(t: Node<'_, '_>, note: &mut Note) {
    let mut bend: Option<f32> = None;
    let mut release = false;
    for el in t.children().filter(Node::is_element) {
        let start = el.attribute("type") != Some("stop");
        match el.tag_name().name() {
            "string" => note.string = el.text().and_then(|s| s.trim().parse().ok()),
            "fret" => note.fret = el.text().and_then(|s| s.trim().parse().ok()),
            "hammer-on" if start => note.techniques.push(Technique::HammerOn),
            "pull-off" if start => note.techniques.push(Technique::PullOff),
            "bend" => {
                let alter: f32 = num(el, "bend-alter").unwrap_or(0.0);
                if child(el, "release").is_some() {
                    release = true;
                    bend.get_or_insert(alter.abs());
                } else if alter > 0.0 {
                    bend = Some(alter);
                }
            }
            "harmonic" => note.techniques.push(if child(el, "artificial").is_some() {
                Technique::ArtificialHarmonic
            } else {
                Technique::NaturalHarmonic
            }),
            "tap" => note.techniques.push(Technique::Tap),
            "open" => note.techniques.push(Technique::Open),
            "stopped" => note.techniques.push(Technique::Closed),
            "other-technical" => {
                let text = el.text().unwrap_or("").trim();
                if let Some((t, _)) = OTHER_TECHNICAL
                    .iter()
                    .find(|(_, n)| n.eq_ignore_ascii_case(text))
                {
                    note.techniques.push(t.clone());
                }
            }
            _ => {}
        }
    }
    if let Some(semitones) = bend {
        note.techniques.push(Technique::Bend { semitones, release });
    }
}

fn read_harmony(el: Node<'_, '_>, tick: u32) -> Option<ChordSymbol> {
    let root = child(el, "root")?;
    let step = text_of(root, "root-step")?;
    let alter: i32 = num(root, "root-alter").unwrap_or(0);
    let kind = child(el, "kind");
    let suffix = kind
        .and_then(|k| k.attribute("text"))
        .map(str::to_owned)
        .or_else(|| {
            let k = kind?.text()?.trim();
            CHORD_KINDS
                .iter()
                .find(|(_, n)| *n == k)
                .map(|(t, _)| (*t).to_owned())
        })
        .unwrap_or_default();
    let acc = match alter {
        1 => "#",
        -1 => "b",
        _ => "",
    };
    let diagram = child(el, "frame").map(|f| {
        let strings: usize = num(f, "frame-strings").unwrap_or(6);
        let mut frets = vec![None; strings];
        for fnote in f.children().filter(|c| c.has_tag_name("frame-note")) {
            if let (Some(s), Some(fr)) = (num::<usize>(fnote, "string"), num::<u8>(fnote, "fret"))
                && (1..=strings).contains(&s)
            {
                frets[s - 1] = Some(fr);
            }
        }
        ChordDiagram {
            frets,
            first_fret: num(f, "first-fret").unwrap_or(1),
        }
    });
    Some(ChordSymbol {
        tick,
        name: format!("{step}{acc}{suffix}"),
        diagram,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(score: &Score) -> Score {
        let xml = write(score);
        read(&xml).unwrap_or_else(|e| panic!("{e}\n{xml}"))
    }

    fn tab_part(show_staff: bool) -> Part {
        let q = Duration::new(NoteValue::Quarter);
        Part {
            name: "Guitar".into(),
            kind: PartKind::Tab(TabSetup {
                tuning: Tuning::guitar_drop_d(),
                capo: 3,
                show_staff,
                show_rhythm: true,
            }),
            bars: vec![
                Bar {
                    time: Some(TimeSig {
                        beats: 4,
                        beat_type: 4,
                    }),
                    key: Some(2),
                    tempo: Some(120.0),
                    chords: vec![ChordSymbol {
                        tick: 960,
                        name: "F#m7".into(),
                        diagram: Some(ChordDiagram {
                            frets: vec![Some(2), Some(2), Some(2), Some(2), None, Some(2)],
                            first_fret: 2,
                        }),
                    }],
                    events: vec![
                        Event::chord(1, q, vec![Note::at(41, 6, 0).with(Technique::PalmMute)]),
                        Event::chord(1, q, vec![Note::at(46, 6, 5).with(Technique::HammerOn)]),
                        Event::chord(1, q, vec![Note::at(48, 6, 7).with(Technique::Slide)]),
                        Event {
                            strum: Some(Strum::Down),
                            ..Event::chord(
                                1,
                                q,
                                vec![
                                    Note::at(53, 5, 5).with(Technique::LetRing),
                                    Note::at(60, 4, 7).with(Technique::Vibrato),
                                ],
                            )
                        },
                    ],
                },
                Bar {
                    events: vec![
                        Event::chord(
                            1,
                            Duration::triplet(NoteValue::Eighth),
                            vec![Note::at(70, 2, 8).with(Technique::Bend {
                                semitones: 2.0,
                                release: true,
                            })],
                        ),
                        Event::chord(
                            1,
                            Duration::triplet(NoteValue::Eighth),
                            vec![Note::at(67, 1, 0).with(Technique::NaturalHarmonic)],
                        ),
                        Event::chord(
                            1,
                            Duration::triplet(NoteValue::Eighth),
                            vec![Note::at(79, 1, 12).with(Technique::Tap)],
                        ),
                        Event::chord(
                            1,
                            Duration::dotted(NoteValue::Quarter),
                            vec![Note {
                                tie: true,
                                ..Note::at(62, 3, 2).with(Technique::Dead)
                            }],
                        ),
                        Event::chord(
                            1,
                            Duration::new(NoteValue::Eighth),
                            vec![
                                Note::at(62, 3, 2)
                                    .with(Technique::Ghost)
                                    .with(Technique::TremoloPicking),
                            ],
                        ),
                        Event::rest(1, Duration::new(NoteValue::Quarter)),
                    ],
                    ..Bar::default()
                },
            ],
        }
    }

    #[test]
    fn tab_round_trips_with_techniques() {
        for show_staff in [false, true] {
            let score = Score {
                title: "Riff & roll".into(),
                composer: "Someone".into(),
                parts: vec![tab_part(show_staff)],
            };
            let back = round_trip(&score);
            assert_eq!(back.title, score.title);
            assert_eq!(back.composer, score.composer);
            let (a, b) = (&score.parts[0], &back.parts[0]);
            assert_eq!(a.name, b.name);
            let PartKind::Tab(setup) = &b.kind else {
                panic!("not tab: {:?}", b.kind)
            };
            assert_eq!(setup.tuning.strings, Tuning::guitar_drop_d().strings);
            assert_eq!(setup.tuning.name, "Drop D");
            assert_eq!(setup.capo, 3);
            assert_eq!(setup.show_staff, show_staff);
            assert_eq!(a.bars, b.bars, "show_staff {show_staff}");
        }
    }

    #[test]
    fn drums_round_trip() {
        let e = Duration::new(NoteValue::Eighth);
        let h = Duration::new(NoteValue::Half);
        let part = Part {
            name: "Drums".into(),
            kind: PartKind::Drums(DrumMap::general_midi()),
            bars: vec![Bar {
                time: Some(TimeSig {
                    beats: 4,
                    beat_type: 4,
                }),
                key: Some(0),
                events: vec![
                    Event::chord(
                        1,
                        e,
                        vec![
                            Note::new(42).with(Technique::Closed),
                            Note::new(38).with(Technique::Accent),
                        ],
                    ),
                    Event::chord(1, e, vec![Note::new(46)]),
                    Event::chord(1, e, vec![Note::new(38).with(Technique::Flam)]),
                    Event::chord(1, e, vec![Note::new(38).with(Technique::Drag)]),
                    Event::chord(1, e, vec![Note::new(38).with(Technique::Ghost)]),
                    Event::chord(1, e, vec![Note::new(38).with(Technique::Roll)]),
                    Event::chord(1, e, vec![Note::new(37).with(Technique::CrossStick)]),
                    Event::chord(1, e, vec![Note::new(49).with(Technique::Choke)]),
                    Event::chord(2, h, vec![Note::new(36)]),
                    Event::chord(2, h, vec![Note::new(44)]),
                ],
                ..Bar::default()
            }],
        };
        let score = Score {
            title: String::new(),
            composer: String::new(),
            parts: vec![part],
        };
        let back = round_trip(&score);
        assert!(matches!(back.parts[0].kind, PartKind::Drums(_)));
        assert_eq!(score.parts[0].bars, back.parts[0].bars);
    }

    #[test]
    fn staff_parts_and_spelling() {
        let q = Duration::new(NoteValue::Quarter);
        let score = Score {
            title: String::new(),
            composer: String::new(),
            parts: vec![Part {
                name: "Bass".into(),
                kind: PartKind::Staff {
                    clef: Clef::Bass8vb,
                },
                bars: vec![Bar {
                    time: Some(TimeSig {
                        beats: 3,
                        beat_type: 4,
                    }),
                    key: Some(-3),
                    events: vec![
                        Event::chord(1, q, vec![Note::new(27)]),
                        Event::chord(1, q, vec![Note::new(30)]),
                        Event::chord(1, q, vec![Note::new(31)]),
                    ],
                    ..Bar::default()
                }],
            }],
        };
        let back = round_trip(&score);
        assert_eq!(
            back.parts[0].kind,
            PartKind::Staff {
                clef: Clef::Bass8vb
            }
        );
        assert_eq!(back.parts[0].bars, score.parts[0].bars);
    }

    #[test]
    fn reads_other_writers_conventions() {
        // Divisions of 2, a transposing part, drum notes placed by display
        // position only, a forward gap and a harmony without text.
        let xml = r#"<?xml version="1.0"?>
<score-partwise version="4.0">
  <movement-title>Other</movement-title>
  <part-list><score-part id="A"><part-name>Kit</part-name></score-part>
    <score-part id="B"><part-name>Gtr</part-name></score-part></part-list>
  <part id="A"><measure number="1">
    <attributes><divisions>2</divisions><time><beats>2</beats><beat-type>4</beat-type></time><clef><sign>percussion</sign></clef></attributes>
    <note><unpitched><display-step>C</display-step><display-octave>5</display-octave></unpitched><duration>2</duration><voice>1</voice><type>quarter</type></note>
    <note><unpitched><display-step>G</display-step><display-octave>5</display-octave></unpitched><duration>2</duration><voice>1</voice><type>quarter</type><notehead>x</notehead></note>
  </measure></part>
  <part id="B"><measure number="1">
    <attributes><divisions>2</divisions><clef><sign>G</sign><line>2</line></clef><transpose><chromatic>0</chromatic><octave-change>-1</octave-change></transpose></attributes>
    <harmony><root><root-step>A</root-step></root><kind>minor</kind></harmony>
    <forward><duration>2</duration></forward>
    <note><pitch><step>A</step><octave>3</octave></pitch><duration>2</duration><voice>1</voice></note>
  </measure></part>
</score-partwise>"#;
        let score = read(xml).expect("reads");
        assert_eq!(score.title, "Other");
        let kit = &score.parts[0];
        let keys: Vec<u8> = kit.bars[0]
            .events
            .iter()
            .map(|e| e.notes[0].pitch)
            .collect();
        assert_eq!(keys, [38, 42]);
        let gtr = &score.parts[1];
        assert_eq!(gtr.bars[0].chords[0].name, "Am");
        assert_eq!(gtr.bars[0].events.len(), 2);
        assert!(gtr.bars[0].events[0].is_rest());
        assert_eq!(gtr.bars[0].events[1].notes[0].pitch, 45);
        assert!(read("<score-timewise/>").is_err());
        assert!(read("not xml").is_err());
    }
}
