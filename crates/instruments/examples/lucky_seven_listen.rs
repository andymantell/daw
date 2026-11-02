// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders the Lucky Seven's sounds to WAV files for listening: each
//! sound made in code playing a line or chords fit for it, the tine
//! piano struck soft to hard, one carrier with its feedback stepped up,
//! a modulator's level swept, and the pitch envelope and LFO shapes.
//!
//! `cargo run -p daw-instruments --release --example lucky_seven_listen -- <dir>`

use std::path::Path;

use daw_engine::midi::Event;
use daw_instruments::{Builtin, Kind};

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

fn chord(beat: f32, len: f32, pitches: &[u8], vel: u8) -> Vec<Note> {
    pitches.iter().map(|&p| (beat, len, p, vel)).collect()
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    // Chords for the keys and pads: Cmaj7, Am7, Fmaj7, G.
    let mut chords: Vec<Note> = Vec::new();
    chords.extend(chord(0.0, 3.8, &[48, 55, 59, 64], 90));
    chords.extend(chord(4.0, 3.8, &[45, 52, 55, 60], 85));
    chords.extend(chord(8.0, 3.8, &[41, 48, 52, 57], 95));
    chords.extend(chord(12.0, 4.0, &[43, 50, 55, 59], 100));
    // A bass line.
    let bass: Vec<Note> = vec![
        (0.0, 0.7, 36, 110),
        (1.0, 0.4, 36, 90),
        (1.5, 0.4, 43, 100),
        (2.0, 0.7, 36, 115),
        (3.0, 0.4, 41, 95),
        (3.5, 0.4, 39, 90),
        (4.0, 0.7, 34, 110),
        (5.0, 0.4, 34, 90),
        (5.5, 0.4, 41, 100),
        (6.0, 0.7, 34, 115),
        (7.0, 0.4, 36, 95),
        (7.5, 0.4, 38, 90),
    ];
    // Brass: stabs then a held chord.
    let mut brass: Vec<Note> = Vec::new();
    brass.extend(chord(0.0, 0.5, &[48, 55, 60], 110));
    brass.extend(chord(1.0, 0.5, &[48, 55, 60], 100));
    brass.extend(chord(1.5, 0.5, &[50, 57, 62], 115));
    brass.extend(chord(2.5, 0.5, &[50, 57, 62], 100));
    brass.extend(chord(3.0, 4.5, &[53, 57, 60, 65], 120));
    // A plucked line and bells.
    let line: Vec<Note> = (0..16)
        .map(|i| {
            let p = [60, 64, 67, 71, 72, 71, 67, 64][i % 8] + if i >= 8 { 5 } else { 0 };
            (i as f32 * 0.5, 0.45, p, 70 + (i % 4) as u8 * 15)
        })
        .collect();
    let bells: Vec<Note> = vec![
        (0.0, 3.0, 72, 100),
        (1.0, 3.0, 76, 90),
        (2.0, 3.0, 79, 95),
        (3.0, 4.0, 84, 110),
        (4.5, 3.0, 67, 80),
        (6.0, 4.0, 72, 100),
    ];

    let b = Builtin::new(Kind::LuckySeven);
    for s in Kind::LuckySeven.sounds() {
        b.set_sound(s.name).unwrap();
        let (notes, bpm, seconds): (&[Note], f32, f32) = match s.name {
            "Tine piano" => (&chords, 88.0, 12.0),
            "Tubular bells" => (&bells, 80.0, 11.0),
            "Solid bass" => (&bass, 120.0, 5.0),
            "Brass section" => (&brass, 100.0, 7.0),
            "Pluck" => (&line, 110.0, 6.0),
            "Glass pad" => (&chords, 60.0, 19.0),
            _ => (&chords, 100.0, 11.0),
        };
        let (l, r) = render(&b, notes, bpm, seconds, |_| {});
        let file = format!(
            "lucky-seven-{}.wav",
            s.name.to_lowercase().replace(' ', "-")
        );
        write(dir, &file, &l, &r);
    }

    // The tine piano from soft to hard: the tine comes in with
    // velocity.
    b.set_sound("Tine piano").unwrap();
    let touch: Vec<Note> = (0..8)
        .map(|i| (i as f32, 0.9, 60, 30 + i as u8 * 14))
        .collect();
    let (l, r) = render(&b, &touch, 100.0, 6.0, |_| {});
    write(dir, "lucky-seven-velocity.wav", &l, &r);

    // One carrier with its feedback stepped from 0 to 7, a bar each.
    let b = Builtin::new(Kind::LuckySeven);
    b.params().set_key("op1_level", 0.0);
    b.params().set_key("op6_level", 99.0);
    b.params().set_key("op6_level3", 99.0);
    b.params().set_key("op6_rate4", 80.0);
    b.params().set_key("algorithm", 32.0);
    let steps: Vec<Note> = (0..8).map(|i| (i as f32, 0.9, 48, 100)).collect();
    let p = b.params().clone();
    let (l, r) = render(&b, &steps, 60.0, 8.5, move |t| {
        p.set_key("feedback", t.floor().min(7.0) as f64);
    });
    write(dir, "lucky-seven-feedback-steps.wav", &l, &r);

    // A modulator's level swept from 0 to 99 over a held note: from a
    // sine to a buzz.
    let b = Builtin::new(Kind::LuckySeven);
    b.params().set_key("op1_level3", 99.0);
    b.params().set_key("op2_level3", 99.0);
    b.params().set_key("op2_level", 0.0);
    let p = b.params().clone();
    let (l, r) = render(&b, &[(0.0, 7.5, 45, 100)], 60.0, 8.0, move |t| {
        p.set_key("op2_level", (t / 7.0 * 99.0).min(99.0) as f64);
    });
    write(dir, "lucky-seven-modulator-sweep.wav", &l, &r);

    // The pitch envelope: a fifth down into each note, then the LFO's
    // six shapes on a held tone, two seconds each.
    let b = Builtin::new(Kind::LuckySeven);
    b.params().set_key("op2_level", 60.0);
    b.params().set_key("op1_level3", 99.0);
    b.params().set_key("op2_level3", 99.0);
    b.params().set_key("pitch_level4", 36.0);
    b.params().set_key("pitch_level1", 50.0);
    b.params().set_key("pitch_rate1", 60.0);
    let (l, r) = render(&b, &line[..8], 90.0, 5.0, |_| {});
    write(dir, "lucky-seven-pitch-envelope.wav", &l, &r);
    let b = Builtin::new(Kind::LuckySeven);
    b.params().set_key("op2_level", 60.0);
    b.params().set_key("op1_level3", 99.0);
    b.params().set_key("op2_level3", 99.0);
    b.params().set_key("lfo_pitch", 40.0);
    b.params().set_key("lfo_amp", 50.0);
    b.params().set_key("op1_amp_mod", 2.0);
    b.params().set_key("lfo_speed", 45.0);
    b.params().set_key("pitch_mod_sens", 4.0);
    let p = b.params().clone();
    let (l, r) = render(&b, &[(0.0, 11.5, 57, 100)], 60.0, 12.0, move |t| {
        p.set_key("lfo_wave", (t / 2.0).floor().min(5.0) as f64);
    });
    write(dir, "lucky-seven-lfo-shapes.wav", &l, &r);
}
