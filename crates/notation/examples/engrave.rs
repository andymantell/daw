// SPDX-License-Identifier: GPL-3.0-or-later
//! Engrave a demonstration score to SVG pages, a PDF, MusicXML and text tab,
//! for reading the layout by eye: `cargo run -p daw-notation --example
//! engrave -- <out dir>`.

use daw_notation::drums::DrumMap;
use daw_notation::fretting::{AssignOptions, assign};
use daw_notation::layout::{LayoutOptions, layout};
use daw_notation::render::{Fonts, pdf, svg};
use daw_notation::score::*;
use daw_notation::tuning::Tuning;

const MUSIC: &[u8] = include_bytes!("../../app/fonts/bravura/Bravura.otf");
const TEXT: &[u8] =
    include_bytes!("../../app/fonts/ibm-plex-sans-condensed/IBMPlexSansCondensed-Regular.ttf");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "target/notation-demo".into());
    std::fs::create_dir_all(&out)?;
    let score = demo();
    let fonts = Fonts::new(MUSIC, TEXT)?;
    let pages = layout(&score, &LayoutOptions::default());
    for (i, page) in pages.iter().enumerate() {
        std::fs::write(format!("{out}/page-{}.svg", i + 1), svg::page_to_svg(page, &fonts))?;
    }
    std::fs::write(format!("{out}/demo.pdf"), pdf::pages_to_pdf(&pages, &fonts))?;
    std::fs::write(format!("{out}/demo.musicxml"), daw_notation::musicxml::write(&score))?;
    std::fs::write(format!("{out}/demo.txt"), daw_notation::text_tab::write(&score, 80))?;
    println!("{} pages written to {out}", pages.len());
    Ok(())
}

fn d(v: NoteValue) -> Duration {
    Duration::new(v)
}

fn demo() -> Score {
    use NoteValue::*;
    use Technique::*;
    let tab = |tuning: Tuning, capo, staff| {
        PartKind::Tab(TabSetup {
            tuning,
            capo,
            show_staff: staff,
            show_rhythm: true,
        })
    };

    // Guitar: a riff with legato, slides, bends, palm mutes and a chord.
    let e8 = d(Eighth);
    let mut guitar = Vec::new();
    guitar.push(Bar {
        time: Some(TimeSig { beats: 4, beat_type: 4 }),
        key: Some(1),
        tempo: Some(96.0),
        chords: vec![ChordSymbol {
            tick: 0,
            name: "Em".into(),
            diagram: Some(ChordDiagram {
                frets: vec![Some(0), Some(0), Some(0), Some(2), Some(2), Some(0)],
                first_fret: 1,
            }),
        }],
        events: vec![
            Event::chord(1, e8, vec![Note::new(40).with(PalmMute)]),
            Event::chord(1, e8, vec![Note::new(40).with(PalmMute)]),
            Event::chord(1, e8, vec![Note::new(43).with(HammerOn)]),
            Event::chord(1, e8, vec![Note::new(45)]),
            Event::chord(1, e8, vec![Note::new(47).with(PullOff)]),
            Event::chord(1, e8, vec![Note::new(45)]),
            Event::chord(1, d(Quarter), vec![Note::new(50).with(Slide)]),
        ],
        ..Bar::default()
    });
    guitar.push(Bar {
        chords: vec![ChordSymbol {
            tick: 0,
            name: "G".into(),
            diagram: Some(ChordDiagram {
                frets: vec![Some(3), Some(0), Some(0), Some(0), Some(2), Some(3)],
                first_fret: 1,
            }),
        }],
        events: vec![
            Event {
                strum: Some(Strum::Down),
                ..Event::chord(
                    1,
                    d(Quarter),
                    [43, 47, 50, 55, 59, 67].iter().map(|&p| Note::new(p).with(LetRing)).collect(),
                )
            },
            Event::chord(1, Duration::dotted(Quarter), vec![Note::new(69).with(Bend { semitones: 2.0, release: true })]),
            Event::chord(1, e8, vec![Note::new(67).with(Vibrato)]),
            Event::chord(1, d(Sixteenth), vec![Note::new(64).with(Dead)]),
            Event::chord(1, d(Sixteenth), vec![Note::new(64)]),
            Event::chord(1, e8, vec![Note::new(76).with(NaturalHarmonic)]),
        ],
        ..Bar::default()
    });
    guitar.push(Bar {
        events: vec![
            Event::chord(1, Duration::triplet(Eighth), vec![Note::new(64)]),
            Event::chord(1, Duration::triplet(Eighth), vec![Note::new(67)]),
            Event::chord(1, Duration::triplet(Eighth), vec![Note::new(69)]),
            Event::chord(1, d(Quarter), vec![Note::new(71).with(Tap)]),
            Event::chord(1, d(Quarter), vec![Note::new(64).with(TremoloPicking)]),
            Event::rest(1, d(Quarter)),
        ],
        ..Bar::default()
    });
    guitar.push(Bar {
        events: vec![Event {
            notes: [40, 47, 52, 55, 59, 64].iter().map(|&p| Note::new(p)).collect(),
            ..Event::chord(1, d(Whole), vec![])
        }],
        ..Bar::default()
    });
    for _ in 0..4 {
        guitar.extend_from_slice(&guitar[..2].to_vec());
    }
    let mut guitar = Part {
        name: "Guitar".into(),
        kind: tab(Tuning::guitar_standard(), 0, true),
        bars: guitar,
    };
    assign(&mut guitar, &AssignOptions::default());

    // Bass: four strings, slap and pop.
    let mut bass = Part {
        name: "Bass".into(),
        kind: tab(Tuning::bass_4(), 0, false),
        bars: (0..4)
            .map(|i| Bar {
                time: (i == 0).then_some(TimeSig { beats: 4, beat_type: 4 }),
                events: vec![
                    Event::chord(1, e8, vec![Note::new(28).with(Slap)]),
                    Event::chord(1, e8, vec![Note::new(40).with(Pop)]),
                    Event::chord(1, e8, vec![Note::new(28).with(Slap)]),
                    Event::chord(1, e8, vec![Note::new(38).with(Ghost)]),
                    Event::chord(1, d(Quarter), vec![Note::new(31).with(Slide)]),
                    Event::chord(1, d(Quarter), vec![Note::new(33)]),
                ],
                ..Bar::default()
            })
            .collect(),
    };
    assign(&mut bass, &AssignOptions::default());

    // Drums: hands up, feet down.
    let s16 = d(Sixteenth);
    let drum_bar = |fill: bool| {
        let mut hands = Vec::new();
        if fill {
            for (i, p) in [38, 38, 48, 48, 47, 47, 43, 43].iter().enumerate() {
                let n = Note::new(*p);
                let n = if i == 0 { n.with(Accent) } else { n };
                hands.push(Event::chord(1, e8, vec![n]));
            }
        } else {
            for beat in 0..4 {
                hands.push(Event::chord(1, e8, vec![Note::new(if beat == 3 { 46 } else { 42 })]));
                let mut n = vec![Note::new(42)];
                if beat % 2 == 1 {
                    n.push(Note::new(38).with(Accent));
                }
                hands.push(Event::chord(1, s16, n));
                hands.push(Event::chord(1, s16, vec![Note::new(38).with(Ghost)]));
            }
        }
        let feet = vec![
            Event::chord(2, d(Quarter), vec![Note::new(36)]),
            Event::chord(2, d(Quarter), vec![Note::new(44)]),
            Event::chord(2, e8, vec![Note::new(36)]),
            Event::chord(2, e8, vec![Note::new(36)]),
            Event::chord(2, d(Quarter), vec![Note::new(44)]),
        ];
        hands.into_iter().chain(feet).collect::<Vec<_>>()
    };
    let mut drum_bars: Vec<Bar> = (0..7)
        .map(|i| Bar {
            time: (i == 0).then_some(TimeSig { beats: 4, beat_type: 4 }),
            events: drum_bar(i == 3),
            ..Bar::default()
        })
        .collect();
    drum_bars.push(Bar {
        events: vec![
            Event::chord(1, d(Quarter), vec![Note::new(49).with(Choke), Note::new(38).with(Flam)]),
            Event::chord(1, d(Quarter), vec![Note::new(37)]),
            Event::chord(1, d(Quarter), vec![Note::new(38).with(Drag)]),
            Event::chord(1, d(Quarter), vec![Note::new(38).with(Roll)]),
            Event::chord(2, d(Whole), vec![Note::new(36)]),
        ],
        ..Bar::default()
    });
    let drums = Part {
        name: "Drums".into(),
        kind: PartKind::Drums(DrumMap::general_midi()),
        bars: drum_bars,
    };

    Score {
        title: "Notation demo".into(),
        composer: "Test page".into(),
        parts: vec![guitar, bass, drums],
    }
}
