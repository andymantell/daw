// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders the June's sounds to stereo WAV files for listening (plan
//! `docs/plans/classic-synths.md`, S4): each sound made in code playing
//! chords or a line, then the chorus modes side by side, the high-pass
//! switch stepping through its positions, the filter singing on its
//! own, and the LFO's delay fading a vibrato in.
//!
//! `cargo run -p daw-instruments --release --example june_listen -- <dir>`

use std::path::Path;

use daw_engine::midi::Event;
use daw_instruments::june::at;
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

/// A chord of `pitches` from `beat` for `beats`.
fn chord(beat: f32, beats: f32, pitches: &[u8], vel: u8) -> Vec<Note> {
    pitches.iter().map(|&p| (beat, beats, p, vel)).collect()
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    // Four slow chords in A minor for the pads.
    let mut pads: Vec<Note> = Vec::new();
    pads.extend(chord(0.0, 3.8, &[57, 60, 64, 69], 90));
    pads.extend(chord(4.0, 3.8, &[53, 57, 60, 65], 90));
    pads.extend(chord(8.0, 3.8, &[55, 59, 62, 67], 90));
    pads.extend(chord(12.0, 3.8, &[52, 56, 59, 64], 90));
    // Off-beat stabs over a bar, as a house track has them.
    let mut stabs: Vec<Note> = Vec::new();
    for bar in 0..2 {
        let b = bar as f32 * 4.0;
        for (beat, pitches) in [
            (0.5, [57u8, 60, 64, 67]),
            (1.5, [57, 60, 64, 67]),
            (2.5, [55, 59, 62, 67]),
            (3.5, [55, 59, 62, 67]),
        ] {
            stabs.extend(chord(b + beat, 0.3, &pitches, 110));
        }
    }
    // A bass line with a few overlaps.
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
    let mut brass: Vec<Note> = Vec::new();
    brass.extend(chord(0.0, 0.9, &[57, 61, 64], 110));
    brass.extend(chord(1.0, 0.9, &[55, 59, 62], 100));
    brass.extend(chord(2.0, 1.9, &[53, 57, 60, 64], 115));
    brass.extend(chord(4.0, 3.5, &[52, 56, 59, 64], 110));

    for s in Kind::June.sounds() {
        let b = Builtin::new(Kind::June);
        b.set_sound(s.name).unwrap();
        let (notes, bpm, seconds) = match s.name {
            "Strings pad" | "Warm pad" => (&pads[..], 80.0, 14.0),
            "House stab" => (&stabs[..], 124.0, 5.0),
            "Chorus bass" | "Round bass" => (&bass[..], 124.0, 6.0),
            "Brass" => (&brass[..], 110.0, 6.0),
            "Organ" => (&pads[..], 110.0, 10.0),
            _ => (&lead[..], 110.0, 7.0),
        };
        let (l, r) = render(&b, notes, bpm, seconds, |_| {});
        let file = format!("june-{}.wav", s.name.to_lowercase().replace(' ', "-"));
        write(dir, &file, &l, &r);
    }

    // The chorus modes side by side on one chord: off, I, II, both.
    let b = Builtin::new(Kind::June);
    b.set_sound("Strings pad").unwrap();
    let p = b.params().clone();
    p.set(at::ATTACK, 0.05);
    let mut notes: Vec<Note> = Vec::new();
    for k in 0..4 {
        notes.extend(chord(k as f32 * 4.0, 3.6, &[57, 60, 64, 69], 90));
    }
    let (l, r) = render(&b, &notes, 80.0, 12.5, |t| {
        p.set(at::CHORUS, ((t / 3.0) as u32).min(3) as f64);
    });
    write(dir, "june-chorus-off-one-two-both.wav", &l, &r);

    // The high-pass switch stepping through its four positions over a
    // saw and sub chord: bass lift, flat, then the two cuts.
    let b = Builtin::new(Kind::June);
    let p = b.params().clone();
    p.set(at::SUB, 0.7);
    p.set(at::CUTOFF, 4000.0);
    p.set(at::FILTER_ENV, 0.0);
    p.set(at::SUSTAIN, 1.0);
    p.set(at::ATTACK, 0.02);
    let (l, r) = render(&b, &chord(0.0, 11.5, &[45, 52, 57], 100), 60.0, 12.0, |t| {
        p.set(at::HIGHPASS, ((t / 3.0) as u32).min(3) as f64);
    });
    write(dir, "june-highpass-steps.wav", &l, &r);

    // The filter singing on its own: no oscillators, resonance full,
    // the keyboard playing the filter with full tracking.
    let b = Builtin::new(Kind::June);
    let p = b.params().clone();
    p.set(at::SAW, 0.0);
    p.set(at::SUB, 0.0);
    p.set(at::NOISE, 0.03);
    p.set(at::CUTOFF, 261.6);
    p.set(at::RESONANCE, 1.0);
    p.set(at::FILTER_ENV, 0.0);
    p.set(at::KEY_TRACK, 1.0);
    p.set(at::ATTACK, 0.01);
    p.set(at::SUSTAIN, 1.0);
    p.set(at::RELEASE, 0.3);
    p.set(at::CHORUS, 1.0);
    let (l, r) = render(&b, &lead, 110.0, 7.0, |_| {});
    write(dir, "june-self-oscillation.wav", &l, &r);

    // The LFO's delay: a held chord, the vibrato arriving after a
    // second and a half, with the pulse width swept by the same LFO.
    let b = Builtin::new(Kind::June);
    b.set_sound("Warm pad").unwrap();
    let p = b.params().clone();
    p.set(at::LFO_RATE, 5.5);
    p.set(at::LFO_DELAY, 1.5);
    p.set(at::LFO_PITCH, 0.3);
    p.set(at::PWM, 0.5);
    let (l, r) = render(
        &b,
        &chord(0.0, 6.0, &[57, 60, 64, 69], 90),
        60.0,
        8.0,
        |_| {},
    );
    write(dir, "june-lfo-delay.wav", &l, &r);

    // The envelope on the filter either way up: the same stab with the
    // envelope opening the filter, then closing it from above.
    let b = Builtin::new(Kind::June);
    b.set_sound("House stab").unwrap();
    let p = b.params().clone();
    let (l, r) = render(&b, &stabs, 124.0, 5.0, |t| {
        if t > 1.9 {
            p.set(at::CUTOFF, 4000.0);
            p.set(at::FILTER_ENV, -3.0);
        }
    });
    write(dir, "june-envelope-inverted.wav", &l, &r);
}
