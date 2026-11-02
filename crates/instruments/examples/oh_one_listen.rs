// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders a few sounds on the Oh-One monosynth to WAV files for
//! listening (plan `docs/plans/classic-synths.md`, S3): a house bass
//! with slides, an acid squelch, a pulse-width lead, a deep sub bass,
//! the filter singing on its own, and the random LFO on the filter.
//! Also prints the time one voice takes per sample.
//!
//! `cargo run -p daw-instruments --release --example oh_one_listen -- <dir>`

use std::path::Path;
use std::sync::Arc;

use daw_engine::midi::{Event, Instrument};
use daw_instruments::oh_one::{OhOne, at, params};
use daw_instruments::params::Params;

const RATE: u32 = 48_000;
const BPM: f32 = 124.0;

fn write(dir: &Path, name: &str, x: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let gain = 0.8 / peak;
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for v in x {
        let s = (v * gain * 32767.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
    println!("wrote {name} (peak before scaling {peak:.3})");
}

/// A note: at `beat`, for `beats`, at `pitch` and `velocity`.
struct Note(f32, f32, u8, u8);

/// Plays `notes` on a synth with `settings` (by index) over `beats`
/// beats and a tail, a block at a time, as the engine would.
fn play(settings: &[(usize, f64)], notes: &[Note], beats: f32) -> Vec<f32> {
    let p = Arc::new(Params::new(params()));
    for &(i, v) in settings {
        p.set(i, v);
    }
    let mut s = OhOne::new(RATE, p);
    let frames = |b: f32| (b * 60.0 / BPM * RATE as f32) as u32;
    let mut events: Vec<(u32, Event)> = Vec::new();
    for (k, n) in notes.iter().enumerate() {
        let id = k as u32;
        events.push((frames(n.0), Event::note_on(0, 0, n.2, n.3, id)));
        events.push((frames(n.0 + n.1), Event::note_off(0, 0, n.2, id)));
    }
    // Offs before ons at the same frame, so a repeated note retriggers.
    events.sort_by_key(|(f, e)| {
        (
            *f,
            !matches!(e.kind, daw_engine::midi::Kind::NoteOff { .. }),
        )
    });
    let total = frames(beats + 2.0) as usize;
    let mut out = vec![0.0f32; total];
    let mut right = vec![0.0f32; 512];
    let mut next = 0;
    let mut at = 0;
    while at < total {
        let n = 512.min(total - at);
        let mut block = Vec::new();
        while next < events.len() && (events[next].0 as usize) < at + n {
            let (f, mut e) = events[next];
            e.frame = f - at as u32;
            block.push(e);
            next += 1;
        }
        right[..n].fill(0.0);
        s.process(&block, &mut out[at..at + n], &mut right[..n]);
        at += n;
    }
    out
}

fn house_bass(dir: &Path) {
    let settings = [
        (at::SAW, 1.0),
        (at::PULSE, 0.4),
        (at::PWM_SOURCE, 1.0),
        (at::PWM, 0.3),
        (at::SUB, 0.8),
        (at::SUB_SHAPE, 1.0),
        (at::CUTOFF, 300.0),
        (at::RESONANCE, 0.55),
        (at::FILTER_ENV, 2.5),
        (at::DECAY, 0.35),
        (at::AMP, 1.0),
        (at::PORTAMENTO, 2.0),
        (at::GLIDE, 0.08),
        (at::DRIVE, 0.4),
        (at::VINTAGE, 0.4),
        (at::LFO_RATE, 0.3),
    ];
    let mut notes = Vec::new();
    for bar in 0..4 {
        let b = bar as f32 * 4.0;
        let root = if bar == 3 { 34 } else { 36 };
        notes.push(Note(b, 0.4, root, 110));
        notes.push(Note(b + 0.5, 0.4, root, 90));
        notes.push(Note(b + 1.0, 0.4, root, 110));
        notes.push(Note(b + 1.5, 0.9, root + 7, 100));
        notes.push(Note(b + 2.0, 0.4, root, 110));
        notes.push(Note(b + 2.5, 0.4, root, 90));
        notes.push(Note(b + 3.0, 0.7, root + 12, 110));
        notes.push(Note(b + 3.5, 0.4, root + 10, 100));
    }
    write(dir, "oh-one-house-bass.wav", &play(&settings, &notes, 16.0));
}

fn acid_squelch(dir: &Path) {
    let settings = [
        (at::SAW, 1.0),
        (at::SUB, 0.3),
        (at::CUTOFF, 200.0),
        (at::RESONANCE, 0.85),
        (at::FILTER_ENV, 3.5),
        (at::DECAY, 0.2),
        (at::SUSTAIN, 0.0),
        (at::RELEASE, 0.1),
        (at::AMP, 1.0),
        (at::PORTAMENTO, 2.0),
        (at::GLIDE, 0.12),
        (at::LEGATO, 1.0),
        (at::DRIVE, 0.6),
        (at::VINTAGE, 0.5),
    ];
    let line = [
        (0.0, 0.4, 33, 127),
        (0.5, 0.4, 33, 80),
        (1.0, 0.4, 45, 127),
        (1.5, 0.4, 33, 80),
        (2.0, 0.7, 36, 127),
        (2.5, 0.4, 38, 100),
        (3.0, 0.4, 33, 127),
        (3.5, 0.4, 31, 80),
    ];
    let mut notes = Vec::new();
    for bar in 0..4 {
        for &(b, len, pitch, vel) in &line {
            // The second half brighter, the accents harder.
            let vel = if bar >= 2 && vel == 127 {
                127
            } else {
                vel - 10
            };
            notes.push(Note(bar as f32 * 4.0 + b, len, pitch, vel as u8));
        }
    }
    write(
        dir,
        "oh-one-acid-squelch.wav",
        &play(&settings, &notes, 16.0),
    );
}

fn pwm_lead(dir: &Path) {
    let settings = [
        (at::SAW, 0.0),
        (at::PULSE, 1.0),
        (at::PWM_SOURCE, 1.0),
        (at::PWM, 0.8),
        (at::SUB, 0.0),
        (at::RANGE, 2.0),
        (at::CUTOFF, 2500.0),
        (at::RESONANCE, 0.3),
        (at::FILTER_ENV, 1.0),
        (at::ATTACK, 0.01),
        (at::DECAY, 0.4),
        (at::SUSTAIN, 0.6),
        (at::RELEASE, 0.3),
        (at::LFO_RATE, 0.7),
        (at::LFO_PITCH, 0.0),
        (at::PORTAMENTO, 1.0),
        (at::GLIDE, 0.06),
        (at::VINTAGE, 0.6),
        (at::LEVEL, -8.0),
    ];
    let melody = [
        (0.0, 1.0, 57),
        (1.0, 1.0, 60),
        (2.0, 0.5, 64),
        (2.5, 1.5, 62),
        (4.0, 1.0, 60),
        (5.0, 1.0, 57),
        (6.0, 2.0, 55),
        (8.0, 1.0, 57),
        (9.0, 1.0, 60),
        (10.0, 0.5, 64),
        (10.5, 1.5, 67),
        (12.0, 4.0, 64),
    ];
    let notes: Vec<Note> = melody
        .iter()
        .map(|&(b, len, p)| Note(b, len, p, 100))
        .collect();
    write(dir, "oh-one-pwm-lead.wav", &play(&settings, &notes, 16.0));
}

fn sub_bass(dir: &Path) {
    let settings = [
        (at::SAW, 0.6),
        (at::SUB, 1.0),
        (at::SUB_SHAPE, 2.0),
        (at::CUTOFF, 120.0),
        (at::RESONANCE, 0.15),
        (at::FILTER_ENV, 1.5),
        (at::ATTACK, 0.004),
        (at::DECAY, 0.5),
        (at::SUSTAIN, 0.3),
        (at::RELEASE, 0.15),
        (at::KEY_TRACK, 0.3),
        (at::DRIVE, 0.3),
        (at::VINTAGE, 0.3),
    ];
    let mut notes = Vec::new();
    for bar in 0..4 {
        let b = bar as f32 * 4.0;
        let root = [36, 36, 41, 43][bar];
        notes.push(Note(b, 1.4, root, 110));
        notes.push(Note(b + 1.5, 0.4, root, 90));
        notes.push(Note(b + 2.0, 1.4, root, 110));
        notes.push(Note(b + 3.5, 0.4, root + 12, 90));
    }
    write(dir, "oh-one-sub-bass.wav", &play(&settings, &notes, 16.0));
}

fn self_oscillation(dir: &Path) {
    let settings = [
        (at::SAW, 0.0),
        (at::SUB, 0.0),
        (at::NOISE, 0.03),
        (at::CUTOFF, 261.6),
        (at::RESONANCE, 1.0),
        (at::FILTER_ENV, 0.0),
        (at::KEY_TRACK, 1.0),
        (at::ATTACK, 0.01),
        (at::SUSTAIN, 1.0),
        (at::RELEASE, 0.3),
        (at::PORTAMENTO, 1.0),
        (at::GLIDE, 0.15),
        (at::VINTAGE, 0.3),
    ];
    let melody = [57, 64, 69, 72, 69, 64, 60, 57];
    let notes: Vec<Note> = melody
        .iter()
        .enumerate()
        .map(|(i, &p)| Note(i as f32, 0.95, p, 100))
        .collect();
    write(
        dir,
        "oh-one-self-oscillation.wav",
        &play(&settings, &notes, 8.0),
    );
}

fn random_filter(dir: &Path) {
    let settings = [
        (at::SAW, 1.0),
        (at::SUB, 0.5),
        (at::CUTOFF, 400.0),
        (at::RESONANCE, 0.7),
        (at::FILTER_ENV, 0.5),
        (at::FILTER_LFO, 2.5),
        (at::LFO_WAVE, 2.0),
        (at::LFO_RATE, 8.27),
        (at::SUSTAIN, 1.0),
        (at::RELEASE, 0.2),
        (at::DRIVE, 0.3),
        (at::VINTAGE, 0.3),
    ];
    let notes = [Note(0.0, 3.8, 36, 110), Note(4.0, 3.8, 41, 110)];
    write(
        dir,
        "oh-one-random-filter.wav",
        &play(&settings, &notes, 8.0),
    );
}

fn cpu() {
    let p = Arc::new(Params::new(params()));
    p.set(at::SUSTAIN, 1.0);
    p.set(at::NOISE, 0.2);
    p.set(at::PWM_SOURCE, 1.0);
    let mut s = OhOne::new(RATE, p);
    let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
    let seconds = 4.0;
    let start = std::time::Instant::now();
    s.process(&[Event::note_on(0, 0, 40, 100, 0)], &mut l, &mut r);
    let blocks = (RATE as f64 * seconds / 512.0) as usize;
    for _ in 0..blocks {
        s.process(&[], &mut l, &mut r);
    }
    let used = start.elapsed().as_secs_f64();
    println!(
        "one voice: {:.0} ns a sample, {:.2}% of one core at {RATE} Hz",
        used / (blocks as f64 * 512.0) * 1e9,
        used / seconds * 100.0
    );
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "oh-one-listen".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    house_bass(dir);
    acid_squelch(dir);
    pwm_lead(dir);
    sub_bass(dir);
    self_oscillation(dir);
    random_filter(dir);
    cpu();
}
