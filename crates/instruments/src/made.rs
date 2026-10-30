// SPDX-License-Identifier: GPL-3.0-or-later
//! Sounds made here, from code, so each instrument has something to
//! play before any sound files are chosen, and the app ships no
//! recorded content. Named `made:` and an id where a sound file's name
//! would go.

use std::f32::consts::TAU;

use crate::drum_voice::{Drum, Hit};
use crate::sample::Sample;

/// What a reference to a sound made here starts with.
pub const PREFIX: &str = "made:";

/// The sounds made here: (id, name).
pub const SOUNDS: &[(&str, &str)] = &[
    ("keys", "Keys"),
    ("pluck", "Plucked string"),
    ("kick", "Kick"),
    ("snare", "Snare"),
    ("hat", "Hi-hat"),
    ("loop", "Drum loop, 120 bpm"),
];

/// The rate sounds are made at; instruments convert them as they do
/// any sound.
pub const RATE: u32 = 48_000;

/// The reference for sound `id`.
pub fn reference(id: &str) -> String {
    format!("{PREFIX}{id}")
}

/// The name of the sound a reference names, if it is one made here.
pub fn name(reference: &str) -> Option<&'static str> {
    let id = reference.strip_prefix(PREFIX)?;
    SOUNDS.iter().find(|s| s.0 == id).map(|s| s.1)
}

/// The sound a `made:` reference names.
pub fn sound(reference: &str) -> Option<Sample> {
    let id = reference.strip_prefix(PREFIX)?;
    let r = RATE as f32;
    let mut s = match id {
        "keys" => keys(r),
        "pluck" => pluck(r),
        "kick" => drum(Drum::Kick, 0.6, 0.6, r),
        "snare" => drum(Drum::Snare, 0.3, 0.5, r),
        "hat" => drum(Drum::ClosedHat, 0.12, 0.5, r),
        "loop" => drum_loop(r),
        _ => return None,
    };
    // Every one peaks at the same level.
    let g = 0.8 / s.peak().max(1e-6);
    for x in s.left.iter_mut().chain(s.right.iter_mut()) {
        *x *= g;
    }
    Some(s)
}

/// A bell-like electric piano note at C4: a few partials, the higher
/// ones dying faster, and a short knock at the start.
fn keys(r: f32) -> Sample {
    let f = 261.63;
    let n = (r * 3.0) as usize;
    let partials = [
        (1.0, 1.0, 1.2),
        (2.0, 0.5, 0.8),
        (3.0, 0.2, 0.4),
        (4.0, 0.12, 0.25),
        (14.0, 0.06, 0.03),
    ];
    let x = (0..n)
        .map(|i| {
            let t = i as f32 / r;
            let s: f32 = partials
                .iter()
                .map(|&(h, a, tau)| a * (TAU * f * h * t).sin() * (-t / tau).exp())
                .sum();
            s * 0.35 * (t / 0.002).min(1.0)
        })
        .collect();
    Sample::mono(x, RATE)
}

/// A plucked string at C3, by the Karplus–Strong method (1983): a burst
/// of noise going round a delay line one period long, softened each time.
fn pluck(r: f32) -> Sample {
    let f = 130.81;
    let period = (r / f) as usize;
    let mut noise = crate::dsp::Noise::new(7);
    let mut line: Vec<f32> = (0..period).map(|_| noise.tick() * 0.5).collect();
    let n = (r * 2.5) as usize;
    let mut out = Vec::with_capacity(n);
    let mut at = 0;
    for _ in 0..n {
        let next = (at + 1) % period;
        let y = 0.498 * (line[at] + line[next]);
        out.push(line[at]);
        line[at] = y;
        at = next;
    }
    Sample::mono(out, RATE)
}

fn drum(d: Drum, decay: f32, tone: f32, r: f32) -> Sample {
    let mut h = Hit::new(d, r, 0.0, decay, tone, 3);
    let mut x = Vec::new();
    while !h.done && x.len() < (r * 4.0) as usize {
        x.push(h.tick() * 0.7);
    }
    Sample::mono(x, RATE)
}

/// One bar at 120 bpm: kick, snare on two and four, hi-hats on the
/// eighths.
fn drum_loop(r: f32) -> Sample {
    let eighth = (r * 0.25) as usize;
    let n = eighth * 8;
    let mut x = vec![0.0f32; n];
    let mut put = |at: usize, d: Drum, decay: f32, level: f32, seed: u32| {
        let mut h = Hit::new(d, r, 0.0, decay, 0.5, seed);
        let mut i = at;
        while !h.done && i < n {
            x[i] += h.tick() * level;
            i += 1;
        }
    };
    for k in 0..8 {
        put(
            k * eighth,
            Drum::ClosedHat,
            0.07,
            if k % 2 == 0 { 0.35 } else { 0.22 },
            k as u32,
        );
    }
    for k in [0, 3, 4] {
        put(k * eighth, Drum::Kick, 0.5, 0.8, 10 + k as u32);
    }
    for k in [2, 6] {
        put(k * eighth, Drum::Snare, 0.25, 0.6, 20 + k as u32);
    }
    Sample::mono(x, RATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_made_and_named() {
        for (id, name) in SOUNDS {
            let r = reference(id);
            let s = sound(&r).unwrap();
            assert!(s.len() > 1000, "{id}");
            let p = s.peak();
            assert!(p > 0.05 && p <= 1.0, "{id}: {p}");
            assert_eq!(super::name(&r), Some(*name));
        }
        assert!(sound("made:nothing").is_none());
        assert!(sound("kick.wav").is_none());
        assert_eq!(sound("made:loop").unwrap().seconds(), 2.0);
    }
}
