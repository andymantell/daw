// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders the Oracle's sounds to WAV files for listening: each sound
//! made in code playing chords or a line, the two filter boards on the
//! same chord, the sync lead swept by its envelope, and the wheel
//! bringing the LFO in.
//!
//! `cargo run -p daw-instruments --release --example oracle_listen -- <dir>`

use std::path::Path;

use daw_engine::midi::{Event, Kind as Midi};
use daw_instruments::oracle::at;
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
/// each block of 512 frames, given the time in seconds and a place to
/// add events.
fn render(
    b: &Builtin,
    notes: &[Note],
    bpm: f32,
    seconds: f32,
    mut tweak: impl FnMut(f32, &mut Vec<Event>),
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
        let mut block = Vec::new();
        tweak(at as f32 / RATE as f32, &mut block);
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

/// A chord's notes, all starting and ending together.
fn chord(start: f32, len: f32, pitches: &[u8], vel: u8) -> Vec<Note> {
    pitches.iter().map(|&p| (start, len, p, vel)).collect()
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    // Chords: Am, F, C, G, each a bar, with a fifth note added late.
    let mut chords: Vec<Note> = Vec::new();
    chords.extend(chord(0.0, 3.8, &[57, 60, 64, 69], 100));
    chords.extend(chord(4.0, 3.8, &[53, 57, 60, 65], 95));
    chords.extend(chord(8.0, 3.8, &[55, 60, 64, 67], 100));
    chords.extend(chord(12.0, 3.8, &[55, 59, 62, 67], 90));
    chords.push((14.0, 1.8, 74, 110));
    // Brass stabs: short chords on the off-beats.
    let mut stabs: Vec<Note> = Vec::new();
    for (bar, root) in [(0.0, 57u8), (4.0, 53), (8.0, 55), (12.0, 52)] {
        stabs.extend(chord(bar, 0.6, &[root, root + 4, root + 7, root + 12], 110));
        stabs.extend(chord(bar + 1.5, 0.4, &[root, root + 4, root + 7], 90));
        stabs.extend(chord(
            bar + 2.5,
            1.2,
            &[root, root + 3, root + 7, root + 10],
            100,
        ));
    }
    // A bass line in A minor.
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
    // A lead line, with overlaps for the glide to work on.
    let lead: Vec<Note> = vec![
        (0.0, 0.9, 69, 100),
        (1.0, 0.9, 72, 100),
        (1.8, 1.2, 76, 110),
        (3.0, 0.9, 74, 100),
        (4.0, 1.9, 69, 110),
        (5.8, 2.2, 81, 120),
        (8.0, 2.0, 76, 100),
    ];
    // Bells and keys: an arpeggio.
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

    for s in Kind::Oracle.sounds() {
        let b = Builtin::new(Kind::Oracle);
        b.set_sound(s.name).unwrap();
        let (notes, bpm, seconds) = match s.name {
            "Poly brass" => (&stabs[..], 120.0, 9.0),
            "Warm pad" | "Strings" => (&chords[..], 100.0, 12.0),
            "Sync lead" | "Unison lead" => (&lead[..], 110.0, 7.0),
            "Bell" | "Hollow keys" => (&keys[..], 110.0, 7.0),
            _ => (&bass[..], 124.0, 6.0),
        };
        let (l, r) = render(&b, notes, bpm, seconds, |_, _| {});
        let file = format!("oracle-{}.wav", s.name.to_lowercase().replace(' ', "-"));
        write(dir, &file, &l, &r);
    }

    // The two boards on the same brass chords: the 4-pole cascade, then
    // the 2-pole state-variable low-pass, each swept by hand.
    for (board, name) in [
        (0.0, "oracle-board-a-cascade.wav"),
        (1.0, "oracle-board-b-svf.wav"),
    ] {
        let b = Builtin::new(Kind::Oracle);
        b.set_sound("Poly brass").unwrap();
        let p = b.params().clone();
        p.set(at::BOARD, board);
        p.set(at::ENV_AMOUNT, 0.0);
        p.set(at::RESONANCE, 0.55);
        p.set(at::F_SUSTAIN, 1.0);
        let (l, r) = render(&b, &chords, 100.0, 10.0, |t, _| {
            let sweep = 0.5 - 0.5 * (t * 0.9).cos();
            p.set(at::CUTOFF, (200.0 * (sweep * 5.0).exp2()) as f64);
        });
        write(dir, name, &l, &r);
    }

    // The state-variable board's four outputs on a held chord, a bar
    // each: low, notch, high, band.
    let b = Builtin::new(Kind::Oracle);
    b.set_sound("Strings").unwrap();
    let p = b.params().clone();
    p.set(at::RESONANCE, 0.5);
    p.set(at::CUTOFF, 1200.0);
    let held = chord(0.0, 15.5, &[57, 60, 64, 69], 100);
    let (l, r) = render(&b, &held, 100.0, 10.0, |t, _| {
        p.set(at::MODE, (t / 2.4).floor().min(3.0) as f64);
    });
    write(dir, "oracle-board-b-outputs.wav", &l, &r);

    // Sync: the lead with oscillator A's pitch swept up and down by
    // hand against B, the classic sync sweep.
    let b = Builtin::new(Kind::Oracle);
    b.set_sound("Sync lead").unwrap();
    let p = b.params().clone();
    p.set(at::PM_ENV, 0.0);
    let (l, r) = render(&b, &[(0.0, 7.5, 45, 110)], 60.0, 8.0, |t, _| {
        // A sits a fifth to two octaves over B.
        let sweep = 0.5 - 0.5 * (t * 1.1).cos();
        p.set(at::B_COARSE, (-7.0 - 17.0 * sweep) as f64);
    });
    write(dir, "oracle-sync-sweep.wav", &l, &r);

    // Poly-mod: oscillator B into the filter at audio rate, its amount
    // rising, on the bass.
    let b = Builtin::new(Kind::Oracle);
    b.set_sound("Poly bass").unwrap();
    let p = b.params().clone();
    p.set(at::PM_FILTER, 1.0);
    p.set(at::B_LEVEL, 0.0);
    p.set(at::B_COARSE, 7.0);
    let (l, r) = render(&b, &bass, 124.0, 6.0, |t, _| {
        p.set(at::PM_OSC_B, (t / 5.5).clamp(0.0, 0.6) as f64);
    });
    write(dir, "oracle-polymod-filter.wav", &l, &r);

    // The wheel pushed up over a held chord: the LFO into the pitch of
    // both oscillators, then into the filter as well.
    let b = Builtin::new(Kind::Oracle);
    b.set_sound("Warm pad").unwrap();
    let p = b.params().clone();
    p.set(at::LFO_RATE, 5.5);
    p.set(at::LFO_INITIAL, 0.0);
    p.set(at::LFO_FREQ_A, 1.0);
    p.set(at::LFO_FREQ_B, 1.0);
    p.set(at::LFO_WIDTH_A, 0.0);
    let (l, r) = render(&b, &held, 100.0, 10.0, |t, events| {
        let wheel = ((t - 2.0) / 5.0).clamp(0.0, 0.5);
        events.push(Event {
            frame: 0,
            channel: 0,
            kind: Midi::Control {
                number: 1,
                value: (wheel * 127.0) as u8,
            },
        });
        p.set(at::LFO_FILTER, if t > 6.0 { 1.0 } else { 0.0 });
    });
    write(dir, "oracle-wheel-lfo.wav", &l, &r);

    // Vintage: the same chord with no drift, then with the vintage
    // amount full, to hear what it does.
    for (amount, name) in [
        (0.0, "oracle-vintage-off.wav"),
        (1.0, "oracle-vintage-full.wav"),
    ] {
        let b = Builtin::new(Kind::Oracle);
        b.set_sound("Strings").unwrap();
        let p = b.params().clone();
        p.set(at::VINTAGE, amount);
        p.set(at::LFO_INITIAL, 0.0);
        let (l, r) = render(&b, &held, 100.0, 8.0, |_, _| {});
        write(dir, name, &l, &r);
    }
}
