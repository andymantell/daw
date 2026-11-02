// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders the Model Mono's sounds to WAV files for listening: each
//! sound made in code playing a short line, a bass line with the mixer
//! overdrive rising, and a lead with glide and vibrato.
//!
//! `cargo run -p daw-instruments --release --example model_mono_listen -- <dir>`

use std::path::Path;

use daw_engine::midi::{Event, Kind as Midi};
use daw_instruments::model_mono::at;
use daw_instruments::{Builtin, Kind};

const RATE: u32 = 48_000;

fn write(dir: &Path, name: &str, x: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let gain = 0.8 / peak;
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for v in x {
        w.write_sample((v * gain * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    println!("wrote {name} (peak before scaling {peak:.3})");
}

/// A note: (beat it starts, beats it lasts, pitch, velocity).
type Note = (f32, f32, u8, u8);

/// Renders `notes` at `bpm` on the instrument `b`, with `tweak` run on
/// each block of 512 frames, given the time in seconds.
fn render(b: &Builtin, notes: &[Note], bpm: f32, seconds: f32, tweak: impl Fn(f32)) -> Vec<f32> {
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
    l
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    // A bass line in A minor, with overlaps for the glide to work on.
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
    let keys: Vec<Note> = vec![
        (0.0, 0.45, 60, 80),
        (0.5, 0.45, 64, 90),
        (1.0, 0.45, 67, 100),
        (1.5, 0.45, 72, 110),
        (2.0, 1.9, 67, 90),
        (4.0, 0.45, 59, 80),
        (4.5, 0.45, 62, 90),
        (5.0, 0.45, 67, 100),
        (5.5, 2.4, 71, 110),
    ];

    let wind: Vec<Note> = vec![(0.0, 6.0, 48, 100)];
    for s in Kind::ModelMono.sounds() {
        let b = Builtin::new(Kind::ModelMono);
        b.set_sound(s.name).unwrap();
        let (notes, bpm, seconds) = match s.name {
            "Screaming lead" | "Vibrato lead" | "Brass" => (&lead[..], 110.0, 7.0),
            "Soft keys" => (&keys[..], 110.0, 6.0),
            "Wind" => (&wind[..], 60.0, 9.0),
            _ => (&bass[..], 124.0, 6.0),
        };
        let x = render(&b, notes, bpm, seconds, |_| {});
        let file = format!("model-mono-{}.wav", s.name.to_lowercase().replace(' ', "-"));
        write(dir, &file, &x);
    }

    // The mixer overdrive rising from clean to hard over the fat bass.
    let b = Builtin::new(Kind::ModelMono);
    b.set_sound("Fat bass").unwrap();
    let p = b.params().clone();
    let x = render(&b, &bass, 124.0, 6.0, |t| {
        p.set(at::OVERDRIVE, (t / 5.5).clamp(0.0, 1.0) as f64);
    });
    write(dir, "model-mono-overdrive-rising.wav", &x);

    // The ladder's emphasis rising on a held note with the cutoff swept
    // by hand: the resonance thickening into a whistle.
    let b = Builtin::new(Kind::ModelMono);
    b.set_sound("Growl bass").unwrap();
    let p = b.params().clone();
    p.set(at::CONTOUR, 0.0);
    p.set(at::SUSTAIN, 1.0);
    let x = render(&b, &[(0.0, 7.5, 33, 110)], 60.0, 8.0, |t| {
        p.set(at::EMPHASIS, (t / 7.0).clamp(0.0, 0.95) as f64);
        let sweep = 0.5 - 0.5 * (t * 1.3).cos();
        p.set(at::CUTOFF, (150.0 * (sweep * 4.0).exp2()) as f64);
    });
    write(dir, "model-mono-emphasis-sweep.wav", &x);

    // Mod wheel in: oscillator 3 at audio rate into the filter and
    // pitch, the wheel pushed up over the note.
    let b = Builtin::new(Kind::ModelMono);
    b.set_sound("Screaming lead").unwrap();
    let p = b.params().clone();
    p.set(at::OSC3_LEVEL, 0.0);
    p.set(at::OSC3_RANGE, 1.0);
    p.set(at::OSC_MOD, 1.0);
    p.set(at::FILTER_MOD, 1.0);
    let mut i = b.make(RATE);
    let n = RATE as usize * 6;
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    i.process(
        &[Event::note_on(0, 0, 57, 110, 0)],
        &mut l[..512],
        &mut r[..512],
    );
    let mut at = 512;
    while at < n {
        let end = (at + 512).min(n);
        let t = at as f32 / RATE as f32;
        let wheel = ((t - 1.0) / 4.0).clamp(0.0, 1.0);
        let e = Event {
            frame: 0,
            channel: 0,
            kind: Midi::Control {
                number: 1,
                value: (wheel * 127.0) as u8,
            },
        };
        i.process(&[e], &mut l[at..end], &mut r[at..end]);
        at = end;
    }
    write(dir, "model-mono-osc3-modulation.wav", &l);
}
