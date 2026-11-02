// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders a few lines through the Acid Three synth to WAV files, for
//! listening (`docs/review-queue/3.4-acid-three.md`), driving the
//! instrument with note events as a note clip would: velocity for
//! accents, overlap for slides.
//!
//! `cargo run -p daw-instruments --release --example acid_listen -- <dir>`

use std::path::Path;
use std::sync::Arc;

use daw_engine::midi::{Event, Instrument};
use daw_instruments::acid::{self, at};
use daw_instruments::params::Params;

const RATE: u32 = 48_000;
const BPM: f32 = 128.0;

fn write(dir: &Path, name: &str, left: &[f32], right: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let peak = left
        .iter()
        .chain(right)
        .fold(0.0f32, |m, v| m.max(v.abs()))
        .max(1e-6);
    let rms = (left.iter().map(|v| v * v).sum::<f32>() / left.len().max(1) as f32).sqrt();
    let gain = 0.8 / peak;
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for (l, r) in left.iter().zip(right) {
        w.write_sample((l * gain * 32767.0) as i16).unwrap();
        w.write_sample((r * gain * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    println!(
        "wrote {name} (peak {:.1} dBFS, rms {:.1} dBFS before scaling)",
        20.0 * peak.log10(),
        20.0 * rms.max(1e-9).log10()
    );
}

/// A step: (note, accent, slide into the next). The note holds for
/// most of its sixteenth, or into the next one when it slides.
type Step = (u8, bool, bool);

/// The classic shape of a line: a root with octave jumps, a few
/// accents, slides on the way down.
const LINE: &[Step] = &[
    (33, true, false),
    (33, false, false),
    (45, false, true),
    (33, false, false),
    (33, true, false),
    (36, false, true),
    (35, false, false),
    (33, false, false),
    (33, true, false),
    (40, false, false),
    (33, false, true),
    (45, true, false),
    (33, false, false),
    (31, false, false),
    (33, true, true),
    (45, false, false),
];

/// Renders `line` `passes` times through a synth with `set` applied,
/// with `tweak` called at the start of each pass to move settings.
fn render(
    line: &[Step],
    passes: usize,
    set: &[(usize, f64)],
    mut tweak: impl FnMut(&Params, usize),
) -> Vec<f32> {
    let params = Arc::new(Params::new(acid::params()));
    for &(i, v) in set {
        params.set(i, v);
    }
    let mut s = acid::Acid::new(RATE, params.clone());
    let sixteenth = (RATE as f32 * 60.0 / BPM / 4.0) as usize;
    let gate = sixteenth * 3 / 4;
    let mut out = Vec::new();
    let mut block = |s: &mut acid::Acid, events: &[Event], n: usize| {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        out.extend(l);
    };
    let mut held: Option<u8> = None;
    for pass in 0..passes {
        tweak(&params, pass);
        for &(note, accent, slide) in line {
            let vel = if accent { 120 } else { 80 };
            // The new note starts; a slid-into note then lets the old
            // one go after it has started, so they overlap.
            let mut events = vec![Event::note_on(0, 0, note, vel, 0)];
            if let Some(old) = held.take() {
                events.push(Event::note_off(1, 0, old, 0));
            }
            if slide {
                block(&mut s, &events, sixteenth);
                held = Some(note);
            } else {
                block(&mut s, &events, gate);
                block(&mut s, &[Event::note_off(0, 0, note, 0)], sixteenth - gate);
            }
        }
    }
    if let Some(old) = held {
        block(&mut s, &[Event::note_off(0, 0, old, 0)], sixteenth);
    }
    // A second of tail.
    block(&mut s, &[], RATE as usize);
    out
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "acid-listen".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    // The line as the defaults play it, the cutoff opening and the
    // resonance rising over four passes: the classic squelch.
    let out = render(LINE, 4, &[(at::WAVE, 1.0)], |p, pass| {
        p.set(at::CUTOFF, 300.0 * 1.5f64.powi(pass as i32));
        p.set(at::RESONANCE, 0.55 + 0.12 * pass as f64);
        p.set(at::ENV_MOD, 0.45 + 0.1 * pass as f64);
    });
    write(dir, "acid-square-sweep.wav", &out, &out);

    // The same on the saw, with a longer decay and more drive.
    let out = render(
        LINE,
        4,
        &[(at::WAVE, 0.0), (at::DECAY, 0.6), (at::DRIVE, 0.35)],
        |p, pass| {
            p.set(at::CUTOFF, 250.0 * 1.6f64.powi(pass as i32));
            p.set(at::RESONANCE, 0.5 + 0.15 * pass as f64);
        },
    );
    write(dir, "acid-saw-drive.wav", &out, &out);

    // Accents alone: the same note eight times, plain, then accented,
    // then accented four in a row so the sweep piles up.
    let accents: Vec<Step> = (0..16)
        .map(|i| (33, i >= 8 && (i >= 12 || i % 2 == 0), false))
        .collect();
    let out = render(
        &accents,
        1,
        &[
            (at::WAVE, 1.0),
            (at::CUTOFF, 400.0),
            (at::RESONANCE, 0.75),
            (at::ENV_MOD, 0.4),
            (at::DECAY, 0.25),
        ],
        |_, _| {},
    );
    write(dir, "acid-accents.wav", &out, &out);

    // Slides alone: up an octave and back, then a slow slide time.
    let slides: Vec<Step> = vec![
        (33, false, true),
        (45, false, false),
        (33, false, true),
        (45, false, true),
        (40, false, true),
        (33, false, false),
        (33, true, true),
        (45, true, true),
        (48, false, false),
    ];
    let mut out = render(
        &slides,
        1,
        &[(at::WAVE, 0.0), (at::CUTOFF, 600.0), (at::RESONANCE, 0.5)],
        |_, _| {},
    );
    out.extend(render(
        &slides,
        1,
        &[
            (at::WAVE, 0.0),
            (at::CUTOFF, 600.0),
            (at::RESONANCE, 0.5),
            (at::SLIDE, 0.25),
        ],
        |_, _| {},
    ));
    write(dir, "acid-slides.wav", &out, &out);

    // Resonance from none to full on a held note with the envelope
    // sweeping, to hear the ladder sing and the bass thin as it does.
    let hold: Vec<Step> = vec![(33, false, false); 2];
    let mut out = Vec::new();
    for res in [0.0, 0.3, 0.6, 0.8, 0.95, 1.0] {
        out.extend(render(
            &hold,
            1,
            &[
                (at::WAVE, 0.0),
                (at::CUTOFF, 200.0),
                (at::RESONANCE, res),
                (at::ENV_MOD, 0.8),
                (at::DECAY, 1.2),
                (at::VINTAGE, 0.0),
            ],
            |_, _| {},
        ));
    }
    write(dir, "acid-resonance-steps.wav", &out, &out);
}
