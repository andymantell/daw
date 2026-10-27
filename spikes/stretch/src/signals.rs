// SPDX-License-Identifier: GPL-3.0-or-later
//! Test material and simple objective measures.

use crate::RATE;

const SR: f32 = RATE as f32;

/// A plucked string by the Karplus-Strong method (Karplus & Strong, 1983),
/// added into `out` at `start`.
pub fn pluck(out: &mut [f32], start: usize, freq: f32, gain: f32, seed: &mut u32) {
    let period = (SR / freq).round() as usize;
    let mut line: Vec<f32> = (0..period)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect();
    // Soften the pick a little, like a pick near the middle of the string.
    for i in 1..period {
        line[i] = 0.5 * (line[i] + line[i - 1]);
    }
    let len = (SR * 3.0) as usize;
    let mut idx = 0;
    for i in 0..len.min(out.len().saturating_sub(start)) {
        let a = line[idx];
        let b = line[(idx + 1) % period];
        out[start + i] += gain * a;
        line[idx] = 0.996 * 0.5 * (a + b);
        idx = (idx + 1) % period;
    }
}

fn midi(n: f32) -> f32 {
    440.0 * 2f32.powf((n - 69.0) / 12.0)
}

/// About eight seconds of guitar-like playing: a picked single-note line,
/// then strummed chords.
pub fn guitar_phrase() -> Vec<f32> {
    let mut out = vec![0.0; (SR * 9.0) as usize];
    let mut seed = 0x1234_5678;
    let line = [
        52.0, 55.0, 57.0, 59.0, 62.0, 59.0, 57.0, 55.0, 52.0, 55.0, 57.0, 60.0,
    ];
    for (i, &n) in line.iter().enumerate() {
        pluck(
            &mut out,
            (i as f32 * 0.25 * SR) as usize,
            midi(n),
            0.5,
            &mut seed,
        );
    }
    let chords: [&[f32]; 4] = [
        &[40.0, 47.0, 52.0, 56.0, 59.0, 64.0], // E
        &[45.0, 52.0, 57.0, 61.0, 64.0],       // A
        &[47.0, 54.0, 59.0, 63.0, 66.0],       // B
        &[40.0, 47.0, 52.0, 56.0, 59.0, 64.0], // E
    ];
    for (c, chord) in chords.iter().enumerate() {
        let at = 3.2 + c as f32 * 1.2;
        for (s, &n) in chord.iter().enumerate() {
            pluck(
                &mut out,
                ((at + s as f32 * 0.012) * SR) as usize,
                midi(n),
                0.25,
                &mut seed,
            );
        }
    }
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    out.iter_mut().for_each(|x| *x *= 0.8 / peak);
    out
}

/// A steady 220 Hz tone with ten harmonics, for pitch checks.
pub fn harmonic_tone(seconds: f32) -> Vec<f32> {
    (0..(SR * seconds) as usize)
        .map(|i| {
            let t = i as f32 / SR;
            (1..=10)
                .map(|h| (2.0 * std::f32::consts::PI * 220.0 * h as f32 * t).sin() / h as f32)
                .sum::<f32>()
                * 0.3
        })
        .collect()
}

/// One pluck after a second of silence, for attack checks.
pub fn single_pluck() -> Vec<f32> {
    let mut out = vec![0.0; (SR * 3.0) as usize];
    let mut seed = 42;
    pluck(&mut out, SR as usize, midi(57.0), 0.8, &mut seed);
    out
}

/// Fundamental by normalised autocorrelation, searched 50–1000 Hz, with
/// parabolic interpolation of the peak.
pub fn estimate_f0(x: &[f32]) -> f32 {
    let min_lag = (SR / 1000.0) as usize;
    let max_lag = (SR / 50.0) as usize;
    let n = x.len() - max_lag - 1;
    let r = |lag: usize| -> f32 {
        let (mut num, mut e0, mut e1) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            num += (x[i] * x[i + lag]) as f64;
            e0 += (x[i] * x[i]) as f64;
            e1 += (x[i + lag] * x[i + lag]) as f64;
        }
        (num / (e0 * e1).sqrt().max(1e-12)) as f32
    };
    let rs: Vec<f32> = (0..=max_lag + 1)
        .map(|l| if l < min_lag { 0.0 } else { r(l) })
        .collect();
    let best = rs[..=max_lag].iter().cloned().fold(0.0f32, f32::max);
    // The first peak that comes close to the best, to avoid octave errors.
    let lag = (min_lag + 1..max_lag)
        .find(|&l| rs[l] > 0.9 * best && rs[l] >= rs[l - 1] && rs[l] >= rs[l + 1])
        .unwrap_or(min_lag);
    let (a, b, c) = (rs[lag - 1], rs[lag], rs[lag + 1]);
    let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
    SR / (lag as f32 + if shift.is_finite() { shift } else { 0.0 })
}

pub fn cents(got: f32, want: f32) -> f32 {
    1200.0 * (got / want).log2()
}

/// Attack shape of the loudest event: milliseconds from 10% to 90% of
/// peak level, and from 1% to the peak (how far the attack is smeared
/// out ahead of itself). Uses a 1 ms RMS envelope.
pub fn attack(x: &[f32]) -> (f32, f32) {
    let w = (SR / 1000.0) as usize;
    let env: Vec<f32> = x
        .chunks(w)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let (pk, &peak) = env
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    let back = |frac: f32| (0..=pk).rev().find(|&i| env[i] < frac * peak).unwrap_or(0);
    let t10 = back(0.1);
    let t90 = back(0.9);
    let t01 = back(0.01);
    ((t90 as f32 - t10 as f32).max(0.0), (pk - t01) as f32)
}

pub fn write_wav(path: &std::path::Path, x: &[f32]) -> std::io::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(std::io::Error::other)?;
    for &s in x {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .map_err(std::io::Error::other)?;
    }
    w.finalize().map_err(std::io::Error::other)
}

/// First channel of a WAV file, as f32. Must be 48 kHz.
pub fn read_wav(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = r.spec();
    if spec.sample_rate != RATE {
        return Err(format!("{} Hz file; please use 48000 Hz", spec.sample_rate));
    }
    let ch = spec.channels as usize;
    let all: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 * scale)
                .collect()
        }
    };
    Ok(all.chunks(ch).map(|f| f[0]).collect())
}
