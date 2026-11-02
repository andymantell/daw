// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders Patch Twenty, the semi-modular monosynth, to WAV files for
//! listening: a screaming ring-mod lead, a bass line, a drone moved by
//! the mod generator and sample-and-hold, both filters self-oscillating
//! as a melody, and a plucked "guitar" played through its filters with
//! the external trigger.
//!
//! `cargo run -p daw-instruments --release --example patch_twenty_listen -- <dir>`

use std::path::Path;
use std::sync::Arc;

use daw_engine::midi::{Event, Instrument};
use daw_instruments::params::Params;
use daw_instruments::patch_twenty::{PatchTwenty, at, cord_at, params};

const RATE: u32 = 48_000;
const BPM: f32 = 120.0;

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
    let gain = 0.8 / peak;
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for (l, r) in left.iter().zip(right) {
        w.write_sample((l * gain * 32767.0) as i16).unwrap();
        w.write_sample((r * gain * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    println!("wrote {name} (peak before scaling {peak:.3})");
}

/// A synth with some settings changed from their defaults.
fn synth(set: &[(usize, f64)]) -> (PatchTwenty, Arc<Params>) {
    let p = Arc::new(Params::new(params()));
    for &(i, v) in set {
        p.set(i, v);
    }
    (PatchTwenty::new(RATE, p.clone()), p)
}

/// A line of (note, length in beats, velocity), 0 for a rest, played
/// at the tempo with the notes slightly overlapping, into `seconds` of
/// audio, with the track's own audio `input` (if any) in the buffers.
fn play(
    s: &mut PatchTwenty,
    line: &[(u8, f32, u8)],
    seconds: f32,
    input: Option<&[f32]>,
) -> (Vec<f32>, Vec<f32>) {
    let n = (RATE as f32 * seconds) as usize;
    let mut left = vec![0.0; n];
    let mut right = vec![0.0; n];
    if let Some(x) = input {
        let m = n.min(x.len());
        left[..m].copy_from_slice(&x[..m]);
        right[..m].copy_from_slice(&x[..m]);
    }
    let beat = RATE as f32 * 60.0 / BPM;
    let mut events = Vec::new();
    let mut t = 0.0f32;
    for &(note, beats, vel) in line {
        if note > 0 {
            let on = (t * beat) as u32;
            let off = ((t + beats * 0.95) * beat) as u32;
            events.push(Event::note_on(on, 0, note, vel, 0));
            events.push(Event::note_off(off, 0, note, 0));
        }
        t += beats;
    }
    events.sort_by_key(|e| e.frame);
    let block = 256;
    let mut at = 0;
    while at < n {
        let end = (at + block).min(n);
        let these: Vec<Event> = (events.iter())
            .filter(|e| (e.frame as usize) >= at && (e.frame as usize) < end)
            .map(|e| Event {
                frame: e.frame - at as u32,
                ..*e
            })
            .collect();
        let (l, r) = (&mut left[at..end], &mut right[at..end]);
        s.process(&these, l, r);
        at = end;
    }
    (left, right)
}

/// A screaming lead: pulse against a ring modulator, both filters
/// peaking, envelope 2 pushing the low-pass, the keyboard tracking
/// both, the mod generator wobbling the pulse width.
fn screaming_lead(dir: &Path) {
    let (mut s, _) = synth(&[
        (at::OSC1, 2.0),
        (at::OSC2, 3.0),
        (at::OSC2_LEVEL, 0.7),
        (at::HP_CUTOFF, 150.0),
        (at::HP_PEAK, 0.7),
        (at::LP_CUTOFF, 500.0),
        (at::LP_PEAK, 0.95),
        (at::LP_EG2, 3.0),
        (at::DRIVE, 0.8),
        (at::EG2_DECAY, 0.5),
        (at::EG2_SUSTAIN, 0.4),
        (at::GLIDE, 0.05),
        (at::RETRIGGER, 0.0),
        (cord_at(0, 0), 7.0),
        (cord_at(0, 1), 7.0),
        (cord_at(0, 2), 0.7),
        (cord_at(1, 0), 1.0),
        (cord_at(1, 1), 4.0),
        (cord_at(1, 2), 0.6),
        (at::VINTAGE, 0.5),
    ]);
    let line = [
        (62, 1.0, 110),
        (65, 0.5, 100),
        (69, 0.5, 120),
        (74, 2.0, 127),
        (72, 0.5, 100),
        (69, 0.5, 110),
        (65, 1.0, 100),
        (62, 2.0, 120),
        (0, 1.0, 0),
        (57, 0.5, 110),
        (60, 0.5, 100),
        (62, 3.0, 127),
    ];
    let twice: Vec<_> = line.iter().chain(&line).copied().collect();
    let (l, r) = play(&mut s, &twice, 13.0, None);
    write(dir, "patch-twenty-screaming-lead.wav", &l, &r);
}

/// A bass: saw an octave down with a square under it, the low-pass
/// snapping shut, a little drive and glide.
fn bass(dir: &Path) {
    let (mut s, _) = synth(&[
        (at::OSC1, 1.0),
        (at::OSC1_OCTAVE, 1.0),
        (at::OSC2, 1.0),
        (at::OSC2_OCTAVE, 0.0),
        (at::OSC2_LEVEL, 0.5),
        (at::LP_CUTOFF, 250.0),
        (at::LP_PEAK, 0.5),
        (at::LP_EG2, 2.5),
        (at::EG2_DECAY, 0.25),
        (at::EG2_SUSTAIN, 0.1),
        (at::DRIVE, 0.5),
        (at::GLIDE, 0.06),
        (at::RETRIGGER, 0.0),
        (at::VINTAGE, 0.4),
    ]);
    let line = [
        (36, 0.75, 120),
        (36, 0.25, 80),
        (36, 0.5, 100),
        (43, 0.5, 120),
        (41, 0.5, 100),
        (36, 0.5, 110),
        (36, 0.5, 90),
        (48, 0.5, 127),
        (36, 0.75, 120),
        (36, 0.25, 80),
        (39, 0.5, 100),
        (41, 0.5, 120),
        (43, 0.5, 100),
        (41, 0.5, 110),
        (36, 1.0, 127),
    ];
    let twice: Vec<_> = line.iter().chain(&line).copied().collect();
    let (l, r) = play(&mut s, &twice, 9.0, None);
    write(dir, "patch-twenty-bass.wav", &l, &r);
}

/// A drone: the amplifier held open, no key, the mod generator's
/// triangle sweeping the low-pass and its sample-and-hold stepping the
/// high-pass, the oscillators a fifth apart.
fn drone(dir: &Path) {
    let (mut s, _) = synth(&[
        (at::OSC1, 1.0),
        (at::OSC1_OCTAVE, 1.0),
        (at::OSC2, 0.0),
        (at::OSC2_PITCH, 7.0),
        (at::OSC2_LEVEL, 0.6),
        (at::HP_CUTOFF, 120.0),
        (at::HP_PEAK, 0.6),
        (at::LP_CUTOFF, 400.0),
        (at::LP_PEAK, 0.8),
        (at::LP_EG2, 0.0),
        (at::LP_MG, 1.5),
        (at::MG_RATE, 0.25),
        (at::MG_SHAPE, 0.3),
        (at::DRIVE, 0.6),
        (at::DRONE, 1.0),
        (cord_at(0, 0), 5.0),
        (cord_at(0, 1), 5.0),
        (cord_at(0, 2), 0.8),
        (cord_at(1, 0), 1.0),
        (cord_at(1, 1), 9.0),
        (cord_at(1, 2), 0.5),
        (at::VINTAGE, 0.6),
    ]);
    // One note to set the pitch; the drone outlives it.
    let (l, r) = play(&mut s, &[(36, 0.5, 100)], 12.0, None);
    write(dir, "patch-twenty-drone.wav", &l, &r);
}

/// Both filters singing on their own: no oscillators, peaks full, the
/// keyboard driving both cutoffs so the whistle follows a melody.
fn self_oscillation(dir: &Path) {
    let (mut s, _) = synth(&[
        (at::OSC1_LEVEL, 0.0),
        (at::OSC2_LEVEL, 0.0),
        (at::HP_CUTOFF, 220.0),
        (at::HP_PEAK, 1.0),
        (at::LP_CUTOFF, 440.0),
        (at::LP_PEAK, 1.0),
        (at::LP_EG2, 0.0),
        (at::EG2_ATTACK, 0.01),
        (at::EG2_SUSTAIN, 1.0),
        (at::EG2_RELEASE, 0.2),
        (at::GLIDE, 0.12),
        (at::RETRIGGER, 0.0),
        (cord_at(0, 0), 7.0),
        (cord_at(0, 1), 7.0),
        (cord_at(0, 2), 1.0),
        // A touch of noise keeps them ringing.
        (at::OSC1, 3.0),
        (at::OSC1_LEVEL, 0.02),
    ]);
    let line = [
        (60, 1.0, 100),
        (67, 1.0, 100),
        (72, 1.0, 100),
        (70, 0.5, 100),
        (67, 0.5, 100),
        (63, 2.0, 100),
        (60, 2.0, 100),
    ];
    let twice: Vec<_> = line.iter().chain(&line).copied().collect();
    let (l, r) = play(&mut s, &twice, 9.0, None);
    write(dir, "patch-twenty-self-oscillation.wav", &l, &r);
}

/// A plucked string, roughly: a burst of noise through a short delay
/// line that feeds back a little dull, which is what a guitar's string
/// does, so the filters have something guitar-like to chew on.
fn pluck(hz: f32, seconds: f32, level: f32) -> Vec<f32> {
    let n = (RATE as f32 * seconds) as usize;
    let len = (RATE as f32 / hz) as usize;
    let mut ring: Vec<f32> = Vec::with_capacity(len);
    let mut x: u32 = 0x1357_9bdf;
    for _ in 0..len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        ring.push((x as f32 / u32::MAX as f32) * 2.0 - 1.0);
    }
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    for _ in 0..n {
        let j = (i + 1) % len;
        let y = 0.5 * (ring[i] + ring[j]) * 0.996;
        out.push(level * ring[i]);
        ring[i] = y;
        i = j;
    }
    out
}

/// A guitar-like line played through the filters: no notes on the
/// synth; the external input triggers the envelopes, which sweep the
/// low-pass, with the amplifier following them.
fn guitar_through_the_filters(dir: &Path) {
    let n = (RATE as f32 * 8.0) as usize;
    let mut input = vec![0.0f32; n];
    let beat = (RATE as f32 * 60.0 / BPM) as usize;
    let chords: [&[f32]; 4] = [
        &[82.41, 123.47, 164.81, 207.65],
        &[110.0, 164.81, 220.0, 277.18],
        &[82.41, 123.47, 164.81, 207.65],
        &[98.0, 146.83, 196.0, 246.94],
    ];
    for (bar, notes) in chords.iter().enumerate() {
        for strum in [0.0, 1.5, 2.0, 3.0] {
            let start = ((bar as f32 * 4.0 + strum) * beat as f32) as usize;
            for (k, &hz) in notes.iter().enumerate() {
                let p = pluck(hz, 1.2, 0.18);
                let offset = start + k * 400;
                for (i, v) in p.iter().enumerate() {
                    if offset + i < n {
                        input[offset + i] += v;
                    }
                }
            }
        }
    }
    let (mut s, _) = synth(&[
        (at::OSC1_LEVEL, 0.0),
        (at::OSC2_LEVEL, 0.0),
        (at::EXT_IN, 1.0),
        (at::EXT_GAIN, 6.0),
        (at::EXT_TRIGGER, 1.0),
        (at::EXT_THRESHOLD, -24.0),
        (at::HP_CUTOFF, 90.0),
        (at::HP_PEAK, 0.3),
        (at::LP_CUTOFF, 300.0),
        (at::LP_PEAK, 0.75),
        (at::LP_EG2, 3.0),
        (at::EG2_ATTACK, 0.005),
        (at::EG2_DECAY, 0.35),
        (at::EG2_SUSTAIN, 0.2),
        (at::EG2_RELEASE, 0.3),
        (at::DRIVE, 0.6),
        (at::LEVEL, -4.0),
    ]);
    let (l, r) = play(&mut s, &[], 8.0, Some(&input));
    write(dir, "patch-twenty-guitar-through-the-filters.wav", &l, &r);
    write(dir, "patch-twenty-guitar-dry.wav", &input, &input);
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    screaming_lead(dir);
    bass(dir);
    drone(dir);
    self_oscillation(dir);
    guitar_through_the_filters(dir);
}
