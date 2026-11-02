// SPDX-License-Identifier: GPL-3.0-or-later
//! Renders a fixed set of sounds through the vintage toolkit to WAV
//! files, for listening (the plan's step 0 asks for a resonant bass
//! sweep, a self-oscillating filter, an overdriven mixer and a slow
//! drifting pad), and prints each filter's measured response.
//!
//! `cargo run -p daw-instruments --release --example vintage_listen -- <dir>`

use std::f32::consts::TAU;
use std::path::Path;

use daw_instruments::dsp::hz;
use daw_instruments::vintage::{
    Analog, Cascade, Chorus, ChorusMode, Diode, Drift, Drive, Ladder, Osc, Poles, SallenKey, Shape,
    SkMode, Steps, Sub, SubShape, Svf, SvfOut, Wave,
};

const RATE: f32 = 48_000.0;

fn write(dir: &Path, name: &str, left: &[f32], right: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE as u32,
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

/// A filter of any kind, for the comparisons.
enum Filter {
    Ladder(Ladder),
    Diode(Diode),
    Cascade(Cascade),
    Svf(Svf),
    SallenKey(SallenKey),
    /// The old poly-synth's filter, clean and linear.
    Linear(daw_instruments::dsp::Svf),
}

impl Filter {
    fn all() -> Vec<(&'static str, Filter)> {
        vec![
            ("ladder", Filter::Ladder(Ladder::new())),
            ("diode", Filter::Diode(Diode::new())),
            ("cascade", Filter::Cascade(Cascade::new())),
            ("svf", Filter::Svf(Svf::new())),
            ("sallen-key", Filter::SallenKey(SallenKey::new())),
            (
                "linear-old",
                Filter::Linear(daw_instruments::dsp::Svf::default()),
            ),
        ]
    }

    fn set(&mut self, cutoff: f32, res: f32, drive: f32) {
        match self {
            Filter::Ladder(f) => f.set(cutoff, res, drive, 0.3, RATE),
            Filter::Diode(f) => f.set(cutoff, res, drive, 60.0, 0.3, RATE),
            Filter::Cascade(f) => f.set(Poles::Four, cutoff, res, drive, 0.3, RATE),
            Filter::Svf(f) => f.set(SvfOut::Morph(0.0), cutoff, res, drive, RATE),
            Filter::SallenKey(f) => f.set(SkMode::Low, cutoff, res, drive, RATE),
            Filter::Linear(f) => f.set(cutoff, res, RATE),
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        match self {
            Filter::Ladder(f) => f.run(x),
            Filter::Diode(f) => f.run(x),
            Filter::Cascade(f) => f.run(x),
            Filter::Svf(f) => f.run(x),
            Filter::SallenKey(f) => f.run(x),
            Filter::Linear(f) => f.run(x, daw_instruments::dsp::Mode::Low),
        }
    }
}

/// A bass line: note, length in eighths, accent.
const BASS: &[(f32, u32, bool)] = &[
    (33.0, 1, true),
    (33.0, 1, false),
    (45.0, 1, false),
    (33.0, 1, true),
    (36.0, 1, false),
    (33.0, 1, false),
    (31.0, 2, true),
    (33.0, 1, true),
    (33.0, 1, false),
    (40.0, 1, false),
    (33.0, 1, true),
    (43.0, 1, false),
    (45.0, 1, false),
    (33.0, 2, true),
];

/// The resonant bass sweep: a saw bass line with a quick filter decay
/// and resonance up, the sound that was "farty" on the linear filter.
fn bass_sweep(dir: &Path) {
    let eighth = (RATE * 60.0 / 124.0 / 2.0) as usize;
    for (name, mut filter) in Filter::all() {
        let mut osc = Osc::new(Wave::Saw);
        let mut sub = Sub::new(SubShape::OctaveDown);
        let mut amp = Analog::IDLE;
        let mut fenv = Analog::IDLE;
        let mut out = Vec::new();
        let drive = Drive::new(0.35);
        for &(note, len, accent) in BASS {
            osc.set_hz(hz(note), RATE);
            amp.trigger();
            fenv.trigger();
            let amp_s = Steps::new(Shape::adsr(0.003, 0.25, 0.3, 0.08), RATE);
            let f_s = Steps::new(
                Shape::adsr(0.002, if accent { 0.18 } else { 0.12 }, 0.0, 0.1),
                RATE,
            );
            let n = eighth * len as usize;
            for i in 0..n {
                if i == n - n / 8 {
                    amp.release();
                    fenv.release();
                }
                let a = amp.next(&amp_s);
                let f = fenv.next(&f_s);
                let cutoff = 90.0 * (f * (if accent { 4.5 } else { 3.5 })).exp2();
                if i % 16 == 0 {
                    filter.set(cutoff, if accent { 0.75 } else { 0.6 }, 1.0);
                }
                let x = drive.run(0.4 * osc.tick(None) + 0.25 * sub.tick(osc.wrapped));
                out.push(filter.run(x) * a * 0.9);
            }
        }
        write(dir, &format!("bass-sweep-{name}.wav"), &out, &out);
    }
}

/// The filter alone, singing: no oscillator, the cutoff played as a
/// melody with glide, on each filter that self-oscillates.
fn self_oscillation(dir: &Path) {
    let notes = [45.0, 52.0, 57.0, 60.0, 57.0, 52.0, 48.0, 45.0];
    for (name, mut filter) in Filter::all() {
        if matches!(filter, Filter::Linear(_)) {
            continue;
        }
        let mut out = Vec::new();
        let mut pitch = notes[0];
        for (k, &note) in notes.iter().enumerate() {
            let n = (RATE * 0.5) as usize;
            for i in 0..n {
                if i % 16 == 0 {
                    pitch += (note - pitch) * 0.02;
                    filter.set(hz(pitch), 1.0, 1.0);
                }
                // A tap to get it going, then nothing.
                let x = if k == 0 && i == 0 { 0.5 } else { 0.0 };
                out.push(filter.run(x));
            }
        }
        write(dir, &format!("self-oscillation-{name}.wav"), &out, &out);
    }
}

/// Three saws into an overdriven mixer into the ladder, held chord
/// notes, with the drive rising through the clip.
fn overdriven_mixer(dir: &Path) {
    let mut oscs: Vec<(Osc, Drift)> = (0..3)
        .map(|k| {
            let mut o = Osc::new(if k == 2 { Wave::Pulse } else { Wave::Saw });
            o.width = 0.3;
            (o, Drift::new(k as u32 + 7))
        })
        .collect();
    let mut filter = Ladder::new();
    let mut out = Vec::new();
    let total = (RATE * 8.0) as usize;
    let notes = [40.0, 40.0 + 0.1, 28.0];
    for i in 0..total {
        let t = i as f32 / total as f32;
        if i % 16 == 0 {
            let drive = Drive::new(t);
            for (k, (o, d)) in oscs.iter_mut().enumerate() {
                let cents = d.next(1.0, 16.0, RATE);
                o.set_hz(hz(notes[k] + cents / 100.0), RATE);
            }
            filter.set(900.0 + 400.0 * (t * TAU * 0.6).sin(), 0.35, 1.0, 0.5, RATE);
            let _ = drive;
        }
        let drive = Drive::new(t);
        let mix = oscs
            .iter_mut()
            .map(|(o, _)| o.tick(None) * 0.33)
            .sum::<f32>();
        out.push(filter.run(drive.run(mix)));
    }
    write(dir, "overdriven-mixer.wav", &out, &out);
}

/// A slow pad: four voices of two drifting oscillators each, pulse
/// width moving, cascade filter opening slowly, through the chorus.
fn slow_pad(dir: &Path) {
    let chord = [48.0, 55.0, 60.0, 64.0, 67.0, 71.0];
    struct V {
        a: Osc,
        b: Osc,
        drift: [Drift; 2],
        filter: Cascade,
        amp: Analog,
        note: f32,
    }
    let mut voices: Vec<V> = chord
        .iter()
        .enumerate()
        .map(|(k, &note)| {
            let mut a = Osc::new(Wave::Saw);
            let mut b = Osc::new(Wave::Pulse);
            a.reset(k as f32 * 0.17);
            b.reset(k as f32 * 0.43);
            V {
                a,
                b,
                drift: [Drift::new(k as u32 * 2 + 1), Drift::new(k as u32 * 2 + 2)],
                filter: Cascade::new(),
                amp: Analog::IDLE,
                note,
            }
        })
        .collect();
    let mut chorus = Chorus::new(RATE as u32);
    chorus.mode = ChorusMode::Both;
    let steps = Steps::new(Shape::adsr(1.2, 0.5, 0.8, 1.5), RATE);
    for v in &mut voices {
        v.amp.trigger();
    }
    let total = (RATE * 10.0) as usize;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for i in 0..total {
        let t = i as f32 / RATE;
        if t > 7.0 && voices[0].amp.held() {
            for v in &mut voices {
                v.amp.release();
            }
        }
        let mut mix = 0.0;
        for v in &mut voices {
            if i % 16 == 0 {
                let c0 = v.drift[0].next(1.0, 16.0, RATE);
                let c1 = v.drift[1].next(1.0, 16.0, RATE);
                v.a.set_hz(hz(v.note + c0 / 100.0), RATE);
                v.b.set_hz(hz(v.note - 12.0 + c1 / 100.0), RATE);
                v.b.width = 0.5 + 0.3 * (t * TAU * 0.23 + v.note).sin();
                let open = (t / 6.0).min(1.0);
                v.filter.set(
                    Poles::Four,
                    300.0 * (open * 3.0).exp2(),
                    0.45,
                    1.0,
                    0.5,
                    RATE,
                );
            }
            let a = v.amp.next(&steps);
            let x = 0.5 * v.a.tick(None) + 0.4 * v.b.tick(None);
            mix += v.filter.run(x) * a;
        }
        let (l, r) = chorus.run(mix * 0.25);
        left.push(l);
        right.push(r);
    }
    write(dir, "slow-pad-chorus.wav", &left, &right);
}

/// An acid line on the diode ladder: square wave, accents that pile the
/// sweep up, slides between overlapping notes.
fn acid_line(dir: &Path) {
    let sixteenth = (RATE * 60.0 / 130.0 / 4.0) as usize;
    // (note, accent, slide into the next)
    let line: &[(f32, bool, bool)] = &[
        (33.0, true, false),
        (33.0, false, false),
        (45.0, false, true),
        (33.0, false, false),
        (33.0, true, false),
        (36.0, false, true),
        (35.0, false, false),
        (33.0, false, false),
        (33.0, true, false),
        (40.0, false, false),
        (33.0, false, true),
        (45.0, true, false),
        (33.0, false, false),
        (31.0, false, false),
        (33.0, true, true),
        (45.0, false, false),
    ];
    let mut osc = Osc::new(Wave::Pulse);
    let mut filter = Diode::new();
    let mut amp = Analog::IDLE;
    let mut fenv = Analog::IDLE;
    let mut pitch = 33.0;
    let mut accent_pile = 0.0f32;
    let mut out = Vec::new();
    for pass in 0..2 {
        for (k, &(note, accent, slide)) in line.iter().enumerate() {
            let prev_slide = k > 0 && line[k - 1].2;
            if !prev_slide {
                amp.trigger();
                fenv.trigger();
                if !accent {
                    accent_pile *= 0.5;
                }
            }
            if accent {
                accent_pile = (accent_pile + 0.5).min(1.5);
            }
            let amp_s = Steps::new(Shape::adsr(0.003, 1.6, 0.0, 0.03), RATE);
            let f_s = Steps::new(
                Shape::adsr(0.002, if accent { 0.2 } else { 0.35 }, 0.0, 0.05),
                RATE,
            );
            for i in 0..sixteenth {
                if !slide && i == sixteenth * 3 / 4 {
                    amp.release();
                }
                if i % 16 == 0 {
                    pitch = if slide || prev_slide {
                        pitch + (note - pitch) * 0.08
                    } else {
                        note
                    };
                    osc.set_hz(hz(pitch), RATE);
                    let f = fenv.level;
                    let env = 0.45 + 0.25 * pass as f32;
                    let cutoff = 180.0 * (f * (env * 4.0 + accent_pile * 1.5)).exp2();
                    filter.set(cutoff, 0.72 + 0.1 * pass as f32, 1.6, 60.0, 0.2, RATE);
                }
                let a = amp.next(&amp_s) * if accent { 1.0 } else { 0.7 };
                fenv.next(&f_s);
                out.push(filter.run(osc.tick(None) * 0.45) * a);
            }
        }
    }
    write(dir, "acid-line-diode.wav", &out, &out);
}

/// Each filter's measured response, for the review notes.
fn responses() {
    let level = |f: &mut Filter, hz: f32| {
        let n = RATE as usize;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for i in 0..n {
            let p = TAU * hz * i as f32 / RATE;
            let y = f.run(0.05 * p.sin());
            if i >= n / 2 {
                re += (y * p.cos()) as f64;
                im += (y * p.sin()) as f64;
            }
        }
        20.0 * ((re * re + im * im).sqrt() * 2.0 / (n / 2) as f64 / 0.05).log10()
    };
    println!("\nResponse at 1 kHz cutoff, dB at (Hz): no resonance / resonance 0.7");
    let points = [50.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
    println!(
        "filter       {}",
        points.iter().map(|p| format!("{p:>7}")).collect::<String>()
    );
    for (name, mut f) in Filter::all() {
        for res in [0.0, 0.7] {
            let row: String = points
                .iter()
                .map(|&p| {
                    f.set(1000.0, res, 1.0);
                    format!("{:>7.1}", level(&mut f, p))
                })
                .collect();
            println!("{name:<12} {row}  (res {res})");
        }
    }
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "vintage-listen".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    bass_sweep(dir);
    self_oscillation(dir);
    overdriven_mixer(dir);
    slow_pad(dir);
    acid_line(dir);
    responses();
}
