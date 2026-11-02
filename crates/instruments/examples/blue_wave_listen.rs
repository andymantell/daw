// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders the Blue Wave's sounds to stereo WAV files for listening:
//! each sound made in code playing chords or a line, then every table
//! swept from its first wave to its last on a held chord, the same
//! sweep in the eight-bit character, and a note played up the keyboard
//! in both characters so the aliasing can be heard coming in.
//!
//! `cargo run -p daw-instruments --release --example blue_wave_listen -- <dir>`

use std::path::Path;

use daw_engine::midi::Event;
use daw_instruments::blue_wave::at;
use daw_instruments::{Builtin, Kind, wavetable};

const RATE: u32 = 48_000;

fn write(dir: &Path, name: &str, l: &[f32], r: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let peak = (l.iter().chain(r))
        .fold(0.0f32, |m, v| m.max(v.abs()))
        .max(1e-6);
    let gain = 0.8 / peak;
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for (a, b) in l.iter().zip(r) {
        w.write_sample((a * gain * 32767.0) as i16).unwrap();
        w.write_sample((b * gain * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    println!("wrote {name} (peak before scaling {peak:.3})");
}

/// A note: (beat it starts, beats it lasts, pitch, velocity).
type Note = (f32, f32, u8, u8);

/// Renders `notes` at `bpm` on the instrument `b`, with `tweak` run on
/// each block of 512 frames, given the time in seconds.
fn render(
    b: &Builtin,
    notes: &[Note],
    bpm: f32,
    seconds: f32,
    tweak: impl Fn(f32),
) -> (Vec<f32>, Vec<f32>) {
    let mut i = b.make(RATE);
    let n = (RATE as f32 * seconds) as usize;
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    let frame = |beat: f32| (beat * 60.0 / bpm * RATE as f32) as usize;
    let mut events: Vec<(usize, Event)> = Vec::new();
    for (k, &(start, len, pitch, vel)) in notes.iter().enumerate() {
        events.push((frame(start), Event::note_on(0, 0, pitch, vel, k as u32)));
        events.push((frame(start + len), Event::note_off(0, 0, pitch, k as u32)));
    }
    events.sort_by_key(|e| e.0);
    let mut at = 0;
    let mut next = 0;
    while at < n {
        let end = (at + 512).min(n);
        tweak(at as f32 / RATE as f32);
        let mut block = Vec::new();
        while next < events.len() && events[next].0 < end {
            let (f, mut e) = events[next];
            e.frame = (f.max(at) - at) as u32;
            block.push(e);
            next += 1;
        }
        i.process(&block, &mut l[at..end], &mut r[at..end]);
        at = end;
    }
    (l, r)
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    // Chords: Am, F, C, G, held and overlapping a little.
    let chords: Vec<Note> = vec![
        (0.0, 3.9, 45, 90),
        (0.0, 3.9, 52, 90),
        (0.0, 3.9, 57, 90),
        (0.0, 3.9, 64, 90),
        (4.0, 3.9, 41, 90),
        (4.0, 3.9, 48, 90),
        (4.0, 3.9, 53, 90),
        (4.0, 3.9, 60, 90),
        (8.0, 3.9, 48, 95),
        (8.0, 3.9, 52, 95),
        (8.0, 3.9, 55, 95),
        (8.0, 3.9, 64, 95),
        (12.0, 4.0, 43, 100),
        (12.0, 4.0, 50, 100),
        (12.0, 4.0, 55, 100),
        (12.0, 4.0, 62, 100),
    ];
    let bells: Vec<Note> = vec![
        (0.0, 2.0, 72, 110),
        (1.0, 2.0, 76, 90),
        (2.0, 2.0, 79, 100),
        (3.0, 3.0, 84, 120),
        (4.5, 3.0, 67, 80),
        (5.0, 3.0, 74, 100),
    ];
    let keys: Vec<Note> = vec![
        (0.0, 0.45, 60, 70),
        (0.5, 0.45, 64, 90),
        (1.0, 0.45, 67, 110),
        (1.5, 0.45, 72, 127),
        (2.0, 1.9, 67, 90),
        (2.0, 1.9, 60, 70),
        (4.0, 0.45, 59, 80),
        (4.5, 0.45, 62, 95),
        (5.0, 0.45, 67, 110),
        (5.5, 2.4, 71, 120),
        (5.5, 2.4, 62, 90),
    ];
    let bass: Vec<Note> = vec![
        (0.0, 0.9, 33, 110),
        (1.0, 0.4, 33, 90),
        (1.5, 0.4, 36, 100),
        (2.0, 0.9, 40, 110),
        (3.0, 0.4, 38, 90),
        (3.5, 1.4, 31, 110),
        (5.0, 0.4, 31, 90),
        (5.5, 0.4, 33, 100),
        (6.0, 0.9, 36, 110),
        (7.0, 0.4, 38, 90),
        (7.4, 0.6, 40, 100),
        (8.0, 1.9, 33, 110),
    ];
    let lead: Vec<Note> = vec![
        (0.0, 0.9, 69, 100),
        (1.0, 0.9, 72, 100),
        (1.8, 1.2, 76, 110),
        (3.0, 0.9, 74, 100),
        (4.0, 1.9, 69, 110),
        (5.8, 2.2, 81, 120),
        (8.0, 2.0, 76, 100),
    ];
    let stabs: Vec<Note> = vec![
        (0.0, 0.4, 45, 110),
        (0.0, 0.4, 52, 110),
        (0.0, 0.4, 57, 110),
        (1.0, 0.4, 45, 100),
        (1.0, 0.4, 52, 100),
        (1.0, 0.4, 57, 100),
        (1.5, 0.4, 48, 115),
        (1.5, 0.4, 55, 115),
        (1.5, 0.4, 60, 115),
        (2.5, 1.4, 50, 120),
        (2.5, 1.4, 57, 120),
        (2.5, 1.4, 62, 120),
        (4.0, 3.9, 41, 110),
        (4.0, 3.9, 48, 110),
        (4.0, 3.9, 53, 110),
        (4.0, 3.9, 57, 110),
    ];

    for s in Kind::BlueWave.sounds() {
        let b = Builtin::new(Kind::BlueWave);
        b.set_sound(s.name).unwrap();
        let (notes, bpm, seconds) = match s.name {
            "Wave bells" => (&bells[..], 90.0, 8.0),
            "Eighties keys" | "Organ" => (&keys[..], 110.0, 6.0),
            "Digital bass" => (&bass[..], 124.0, 6.0),
            "Sync lead" | "Folded lead" => (&lead[..], 110.0, 7.0),
            "Eight-bit stab" => (&stabs[..], 112.0, 6.5),
            _ => (&chords[..], 84.0, 14.0),
        };
        let (l, r) = render(&b, notes, bpm, seconds, |_| {});
        let file = format!("blue-wave-{}.wav", s.name.to_lowercase().replace(' ', "-"));
        write(dir, &file, &l, &r);
    }

    // Every table swept by hand from its first wave to its sixtieth
    // over a held chord, two seconds a table, the filter open and
    // nothing else moving: what the tables hold.
    let held: Vec<Note> = vec![
        (0.0, 100.0, 45, 100),
        (0.0, 100.0, 52, 100),
        (0.0, 100.0, 61, 100),
    ];
    for character in 0..2 {
        let b = Builtin::new(Kind::BlueWave);
        let p = b.params().clone();
        p.set(at::CHARACTER, character as f64);
        p.set(at::MIX, 0.0);
        p.set(at::CUTOFF, 20_000.0);
        p.set(at::KEY_TRACK, 0.0);
        p.set(at::SUSTAIN, 1.0);
        p.set(at::SPREAD, 0.3);
        let tables = wavetable::NAMES.len();
        let each = 2.5;
        let (l, r) = render(&b, &held, 60.0, each * tables as f32, |t| {
            let table = ((t / each) as usize).min(tables - 1);
            let f = (t / each).fract();
            p.set(at::TABLE, table as f64);
            p.set(at::WAVE, (f * wavetable::LAST_OWN as f32) as f64);
        });
        let name = [
            "blue-wave-tables-smooth.wav",
            "blue-wave-tables-eight-bit.wav",
        ][character];
        write(dir, name, &l, &r);
    }

    // A saw played up the keyboard, smooth then eight-bit: the top
    // folding back is the eight-bit character.
    let up: Vec<Note> = (0..16)
        .map(|i| (i as f32 * 0.5, 0.45, 48 + 7 * (i % 8) as u8, 100))
        .collect();
    let b = Builtin::new(Kind::BlueWave);
    let p = b.params().clone();
    p.set(at::WAVE, 59.0);
    p.set(at::MIX, 0.0);
    p.set(at::CUTOFF, 20_000.0);
    p.set(at::KEY_TRACK, 0.0);
    p.set(at::SPREAD, 0.0);
    let (l, r) = render(&b, &up, 120.0, 4.5, |t| {
        p.set(at::CHARACTER, if t < 2.0 { 0.0 } else { 1.0 });
    });
    write(dir, "blue-wave-up-the-keyboard.wav", &l, &r);
}
