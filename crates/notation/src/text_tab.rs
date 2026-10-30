// SPDX-License-Identifier: GPL-3.0-or-later
//! Plain-text tab output, the kind people paste into messages and forums:
//! one line per string with fret numbers on dashes, wrapped to a width.
//!
//! Marks: `h` hammer-on, `p` pull-off, `/` and `\` slides, `b` bend (with
//! the target fret) and `r` release, `~` vibrato, `x` dead note, `(5)`
//! ghost or tied note, `<12>` natural harmonic, `t` tapped. Palm mutes and
//! let ring are written on a line above the strings.

use crate::score::{Part, PartKind, Score, TICKS_PER_QUARTER, Technique, bar_positions};
use crate::tuning::pitch_class_name;

/// Write every tab part of a score, each line at most `width` characters
/// wide (a single bar wider than that is still kept whole).
pub fn write(score: &Score, width: usize) -> String {
    let mut out = String::new();
    if !score.title.is_empty() {
        out.push_str(&score.title);
        out.push('\n');
        if !score.composer.is_empty() {
            out.push_str(&score.composer);
            out.push('\n');
        }
        out.push('\n');
    }
    for part in &score.parts {
        if matches!(part.kind, PartKind::Tab(_)) {
            out.push_str(&write_part(part, width));
            out.push('\n');
        }
    }
    out
}

/// One bar as text: a label line and one line per string, all the same
/// length, without bar lines.
struct BarText {
    label: String,
    strings: Vec<String>,
}

fn write_part(part: &Part, width: usize) -> String {
    let PartKind::Tab(setup) = &part.kind else {
        return String::new();
    };
    let n = setup.tuning.string_count();
    let names: Vec<String> = setup
        .tuning
        .strings
        .iter()
        .map(|&p| pitch_class_name(p).to_owned())
        .collect();
    let name_w = names.iter().map(String::len).max().unwrap_or(1);

    let mut out = format!("{}\nTuning: {}", part.name, setup.tuning.letters());
    if setup.capo > 0 {
        out.push_str(&format!("   Capo {}", setup.capo));
    }
    out.push_str("\n\n");

    let bars: Vec<BarText> = part.bars.iter().map(|b| bar_text(b, n)).collect();
    let mut start = 0;
    while start < bars.len() {
        let mut used = name_w + 1;
        let mut end = start;
        while end < bars.len() {
            let w = bars[end].strings[0].chars().count() + 1;
            if end > start && used + w > width {
                break;
            }
            used += w;
            end += 1;
        }
        let line = &bars[start..end];
        let label: String = line.iter().map(|b| format!("{} ", b.label)).collect();
        if !label.trim().is_empty() {
            out.push_str(&format!("{:name_w$} {}\n", "", label.trim_end()));
        }
        for (s, name) in names.iter().enumerate() {
            out.push_str(&format!("{name:<name_w$}|"));
            for b in line {
                out.push_str(&b.strings[s]);
                out.push('|');
            }
            out.push('\n');
        }
        out.push('\n');
        start = end;
    }
    out
}

fn bar_text(bar: &crate::score::Bar, strings: usize) -> BarText {
    let mut cols: Vec<(u32, Vec<String>, String, u32)> = Vec::new();
    for (tick, e) in bar_positions(bar) {
        let event = &bar.events[e];
        if cols.last().is_none_or(|c| c.0 != tick) {
            cols.push((tick, vec![String::new(); strings], String::new(), u32::MAX));
        }
        let col = cols.last_mut().expect("pushed");
        col.3 = col.3.min(event.duration.ticks());
        for note in &event.notes {
            let (Some(s), Some(fret)) = (note.string, note.fret) else { continue };
            let Some(cell) = col.1.get_mut(usize::from(s.max(1) - 1)) else { continue };
            *cell = token(note, fret);
            if note.has(&Technique::PalmMute) {
                col.2 = "PM".into();
            } else if note.has(&Technique::LetRing) && col.2.is_empty() {
                col.2 = "LR".into();
            }
        }
    }
    let mut lines = vec![String::from("-"); strings];
    let mut label = String::from(" ");
    for (_, cells, mark, shortest) in &cols {
        // Longer notes get more dashes after them.
        let sixteenths = (*shortest / (TICKS_PER_QUARTER / 4)).clamp(1, 16) as usize;
        let room = 1 + sixteenths.ilog2() as usize;
        let w = cells.iter().map(|c| c.chars().count()).max().unwrap_or(1).max(mark.len()).max(1);
        for (line, cell) in lines.iter_mut().zip(cells) {
            line.push_str(cell);
            line.push_str(&"-".repeat(w - cell.chars().count() + room));
        }
        label.push_str(mark);
        label.push_str(&" ".repeat(w - mark.len() + room));
    }
    if cols.is_empty() {
        for line in &mut lines {
            line.push_str("--------");
        }
        label.push_str("        ");
    }
    BarText { label, strings: lines }
}

fn token(note: &crate::score::Note, fret: u8) -> String {
    let mut t = if note.has(&Technique::Dead) {
        "x".to_owned()
    } else if note.has(&Technique::NaturalHarmonic) {
        format!("<{fret}>")
    } else if note.has(&Technique::Ghost) {
        format!("({fret})")
    } else {
        fret.to_string()
    };
    if note.has(&Technique::Tap) {
        t.insert(0, 't');
    }
    for tech in &note.techniques {
        match tech {
            Technique::HammerOn => t.push('h'),
            Technique::PullOff => t.push('p'),
            Technique::Slide => t.push('/'),
            Technique::Vibrato => t.push('~'),
            Technique::Bend { semitones, release } => {
                let target = f32::from(fret) + semitones;
                t.push_str(&format!("b{}", target.round() as i32));
                if *release {
                    t.push_str(&format!("r{fret}"));
                }
            }
            _ => {}
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{Bar, Duration, Event, Note, NoteValue, TabSetup};
    use crate::tuning::Tuning;

    #[test]
    fn writes_strings_with_marks() {
        let q = Duration::new(NoteValue::Quarter);
        let part = Part {
            name: "Bass".into(),
            kind: PartKind::Tab(TabSetup {
                tuning: Tuning::bass_4(),
                capo: 0,
                show_staff: false,
                show_rhythm: false,
            }),
            bars: vec![
                Bar {
                    events: vec![
                        Event::chord(1, q, vec![Note::at(28, 4, 0).with(Technique::PalmMute)]),
                        Event::chord(1, q, vec![Note::at(33, 4, 5).with(Technique::HammerOn)]),
                        Event::chord(1, q, vec![Note::at(35, 4, 7)]),
                        Event::chord(1, q, vec![Note::at(45, 2, 7).with(Technique::Bend { semitones: 2.0, release: true })]),
                    ],
                    ..Bar::default()
                };
                30
            ],
        };
        let score = Score {
            title: "Line".into(),
            composer: String::new(),
            parts: vec![part],
        };
        let text = write(&score, 60);
        assert!(text.starts_with("Line\n\nBass\nTuning: EADG\n"));
        assert!(text.contains("|-0----5h---7-----------|"), "{text}");
        assert!(text.contains("7b9r7"));
        assert!(text.contains("PM"));
        for line in text.lines() {
            assert!(line.chars().count() <= 60, "too wide: {line}");
        }
        // Strings are named from the highest down.
        let first_bar_lines: Vec<&str> = text.lines().filter(|l| l.contains('|')).take(4).collect();
        assert!(first_bar_lines[0].starts_with("G|"));
        assert!(first_bar_lines[3].starts_with("E|"));
    }
}
