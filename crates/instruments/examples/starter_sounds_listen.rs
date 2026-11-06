// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders every starter sound for listening: each built-in instrument's
//! own sounds (a drum machine's kits) playing a phrase that suits it,
//! and every sound made here played by the sampler, the drum sampler or
//! as it is (the loops). Prints how loud each is before the file is
//! scaled, so a sound far louder or quieter than the rest stands out.
//!
//! `cargo run -p daw-instruments --release --example starter_sounds_listen -- <dir>`

use std::path::Path;

use daw_engine::midi::Event;
use daw_instruments::made::{self, Group};
use daw_instruments::{Builtin, Kind, Loaded};

const RATE: u32 = 48_000;

/// A note: (beat it starts, beats it lasts, pitch, velocity).
type Note = (f32, f32, u8, u8);

fn write(dir: &Path, name: &str, l: &[f32], r: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let peak = l.iter().chain(r).fold(0.0f32, |m, v| m.max(v.abs()));
    let rms = (l.iter().chain(r).map(|v| v * v).sum::<f32>() / (2 * l.len()).max(1) as f32).sqrt();
    let gain = 0.8 / peak.max(1e-6);
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for (a, b) in l.iter().zip(r) {
        w.write_sample((a * gain * 32767.0) as i16).unwrap();
        w.write_sample((b * gain * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    println!(
        "{name:60} peak {:6.1} dB  rms {:6.1} dB",
        20.0 * peak.max(1e-9).log10(),
        20.0 * rms.max(1e-9).log10()
    );
}

fn render(b: &Builtin, notes: &[Note], bpm: f32) -> (Vec<f32>, Vec<f32>) {
    let mut i = b.make(RATE);
    let end = notes.iter().map(|n| n.0 + n.1).fold(0.0, f32::max);
    let frame = |beat: f32| (beat * 60.0 / bpm * RATE as f32) as usize;
    let n = frame(end) + 2 * RATE as usize;
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut events: Vec<(usize, Event)> = Vec::new();
    for (k, &(start, len, pitch, vel)) in notes.iter().enumerate() {
        events.push((frame(start), Event::note_on(0, 0, pitch, vel, k as u32)));
        events.push((frame(start + len), Event::note_off(0, 0, pitch, k as u32)));
    }
    events.sort_by_key(|e| e.0);
    let (mut at, mut next) = (0, 0);
    while at < n {
        let stop = (at + 512).min(n);
        let mut block = Vec::new();
        while next < events.len() && events[next].0 < stop {
            let (f, mut e) = events[next];
            e.frame = (f.max(at) - at) as u32;
            block.push(e);
            next += 1;
        }
        i.process(&block, &mut l[at..stop], &mut r[at..stop]);
        at = stop;
    }
    (l, r)
}

/// A phrase that suits a sound, by what its name says it is.
fn phrase(name: &str) -> Vec<Note> {
    let n = name.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|w| n.contains(w));
    if has(&["kit"]) {
        // Two bars: kick, snare, hats, a tom fill and a crash.
        let mut v = Vec::new();
        for bar in 0..2 {
            let b = bar as f32 * 4.0;
            for k in 0..8 {
                v.push((
                    b + k as f32 * 0.5,
                    0.25,
                    42,
                    if k % 2 == 0 { 100 } else { 70 },
                ));
            }
            for k in [0.0, 1.5, 2.5] {
                v.push((b + k, 0.25, 36, 120));
            }
            for k in [1.0, 3.0] {
                v.push((b + k, 0.25, 38, 110));
            }
        }
        for (k, t) in [48, 47, 45, 43].iter().enumerate() {
            v.push((8.0 + k as f32 * 0.5, 0.25, *t, 110));
        }
        v.push((10.0, 0.25, 36, 120));
        v.push((10.0, 0.25, 49, 110));
        v.push((10.5, 0.25, 46, 100));
        v
    } else if has(&["bass", "line", "rubber", "bleep", "acid", "zap", "noise"]) {
        // A bass line round C2.
        [36, 36, 48, 39, 41, 36, 43, 34]
            .iter()
            .enumerate()
            .map(|(k, p)| (k as f32 * 0.5, 0.4, *p, if k % 3 == 0 { 120 } else { 90 }))
            .collect()
    } else if has(&["pad", "strings", "choir", "organ", "drone", "wind", "sweep"]) {
        // Two held chords.
        let mut v: Vec<Note> = [48, 55, 60, 64]
            .iter()
            .map(|p| (0.0, 4.0, *p, 90))
            .collect();
        v.extend([45, 52, 57, 60].iter().map(|p| (4.0, 4.0, *p, 90)));
        v
    } else if has(&["guitar"]) {
        Vec::new()
    } else {
        // A short tune round C4, then a chord.
        let mut v: Vec<Note> = [60, 64, 67, 72, 67, 64]
            .iter()
            .enumerate()
            .map(|(k, p)| (k as f32 * 0.5, 0.45, *p, 100))
            .collect();
        v.extend([60, 64, 67].iter().map(|p| (3.0, 2.0, *p, 100)));
        v
    }
}

fn file_name(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "starter-sounds".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    for k in Kind::ALL {
        for s in k.sounds() {
            let notes = phrase(s.name);
            if notes.is_empty() {
                println!(
                    "{} {}: plays a guitar through it, not rendered",
                    k.name(),
                    s.name
                );
                continue;
            }
            let b = Builtin::new(k);
            b.set_sound(s.name).unwrap();
            let (l, r) = render(&b, &notes, 110.0);
            let name = format!("{}--{}.wav", file_name(k.name()), file_name(s.name));
            write(dir, &name, &l, &r);
        }
    }
    for m in made::SOUNDS {
        let reference = made::reference(m.id);
        let sound = made::sound(&reference).unwrap();
        let name = format!("made--{}.wav", file_name(m.name));
        match m.group {
            Group::Loops => write(dir, &name, &sound.left, &sound.right),
            Group::Drums => {
                let mut b = Builtin::new(Kind::DrumSampler);
                let loaded = Loaded::new(reference, Some(std::sync::Arc::new(sound)));
                b.set_sample("sample", Some(loaded));
                let notes: Vec<Note> = (0..4).map(|k| (k as f32, 0.5, 60, 110 - 15 * k)).collect();
                let (l, r) = render(&b, &notes, 110.0);
                write(dir, &name, &l, &r);
            }
            _ => {
                let mut b = Builtin::new(Kind::Sampler);
                let loaded = Loaded::new(reference, Some(std::sync::Arc::new(sound)));
                b.set_zone_sound(0, loaded);
                let notes = match m.group {
                    Group::Basses => phrase("bass"),
                    Group::Pads => phrase("pad"),
                    _ => phrase("keys"),
                };
                let (l, r) = render(&b, &notes, 110.0);
                write(dir, &name, &l, &r);
            }
        }
    }
}
