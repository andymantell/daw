// SPDX-License-Identifier: GPL-3.0-or-later
//! Sounds made here, from code, so each instrument has something to
//! play before any sound files are chosen, and the app ships no
//! recorded content. Named `made:` and an id where a sound file's name
//! would go.

use std::f32::consts::TAU;
use std::sync::Mutex;

use crate::drum_voice::{Drum, Hit};
use crate::sample::Sample;

/// What a reference to a sound made here starts with.
pub const PREFIX: &str = "made:";

/// What kind of sound it is, for the menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Keys,
    Basses,
    Pads,
    Plucks,
    Drums,
    Loops,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::Keys,
        Group::Basses,
        Group::Pads,
        Group::Plucks,
        Group::Drums,
        Group::Loops,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Group::Keys => "Keys",
            Group::Basses => "Basses",
            Group::Pads => "Pads",
            Group::Plucks => "Plucks",
            Group::Drums => "Drums",
            Group::Loops => "Loops",
        }
    }
}

/// A sound made here: its id (never change one, projects keep it), its
/// name, its kind, the note it plays at if it has a pitch, and where it
/// loops (as parts of its length, start to end) if it is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Made {
    pub id: &'static str,
    pub name: &'static str,
    pub group: Group,
    pub root: Option<u8>,
    pub looped: Option<(f64, f64)>,
}

const fn made(id: &'static str, name: &'static str, group: Group, root: Option<u8>) -> Made {
    Made {
        id,
        name,
        group,
        root,
        looped: None,
    }
}

/// A held sound: its attack, then `LOOP` seconds that loop.
const fn held(id: &'static str, name: &'static str, group: Group, root: u8, attack: f64) -> Made {
    Made {
        id,
        name,
        group,
        root: Some(root),
        looped: Some((attack / (attack + LOOP), 1.0)),
    }
}

/// How long a held sound's loop is, in seconds: every partial in it is
/// tuned to a whole number of cycles in this time, so it loops without
/// a click (the pitch moves by a cent or two at most).
const LOOP: f64 = 2.0;

const C2: u8 = 36;
const C3: u8 = 48;
const C4: u8 = 60;
const C5: u8 = 72;

/// The sounds made here, by kind.
pub const SOUNDS: &[Made] = &[
    made("keys", "Keys", Group::Keys, Some(C4)),
    made("piano", "Soft piano", Group::Keys, Some(C4)),
    held("organ", "Organ", Group::Keys, C4, 0.02),
    made("mallets", "Mallets", Group::Keys, Some(C4)),
    made("bell", "Bell", Group::Keys, Some(C5)),
    made("round_bass", "Round bass", Group::Basses, Some(C2)),
    made("picked_bass", "Picked bass", Group::Basses, Some(C2)),
    made("saw_bass", "Saw bass", Group::Basses, Some(C2)),
    held("sub_bass", "Sub bass", Group::Basses, C2, 0.01),
    held("warm_pad", "Warm pad", Group::Pads, C4, 0.4),
    held("strings", "Strings", Group::Pads, C4, 0.3),
    held("glass_pad", "Glass pad", Group::Pads, C4, 0.5),
    made("pluck", "Plucked string", Group::Plucks, Some(C3)),
    made("nylon", "Nylon guitar", Group::Plucks, Some(C3)),
    made("harp", "Harp", Group::Plucks, Some(C4)),
    made("kick", "Kick", Group::Drums, None),
    made("deep_kick", "Deep kick", Group::Drums, None),
    made("tight_kick", "Tight kick", Group::Drums, None),
    made("snare", "Snare", Group::Drums, None),
    made("tight_snare", "Tight snare", Group::Drums, None),
    made("clap", "Clap", Group::Drums, None),
    made("rim", "Rim", Group::Drums, None),
    made("hat", "Hi-hat", Group::Drums, None),
    made("open_hat", "Open hi-hat", Group::Drums, None),
    made("shaker", "Shaker", Group::Drums, None),
    made("low_tom", "Low tom", Group::Drums, None),
    made("high_tom", "High tom", Group::Drums, None),
    made("crash", "Crash", Group::Drums, None),
    made("ride", "Ride", Group::Drums, None),
    made("cowbell", "Cowbell", Group::Drums, None),
    made("loop", "Drum loop, 120 bpm", Group::Loops, None),
    made("loop_90", "Laid-back loop, 90 bpm", Group::Loops, None),
    made("loop_shuffle", "Shuffle loop, 100 bpm", Group::Loops, None),
    made(
        "loop_four",
        "Four on the floor, 124 bpm",
        Group::Loops,
        None,
    ),
];

/// The rate sounds are made at; instruments convert them as they do
/// any sound.
pub const RATE: u32 = 48_000;

/// The reference for sound `id`.
pub fn reference(id: &str) -> String {
    format!("{PREFIX}{id}")
}

/// The sound made here a reference names.
pub fn find(reference: &str) -> Option<&'static Made> {
    let id = reference.strip_prefix(PREFIX)?;
    SOUNDS.iter().find(|s| s.id == id)
}

/// The name of the sound a reference names, if it is one made here.
pub fn name(reference: &str) -> Option<&'static str> {
    find(reference).map(|s| s.name)
}

/// The settings a sampler's zone takes to play a sound made here as it
/// was made: its root note, and its loop if it is held. By the part of
/// the key after `zoneN.`.
pub fn zone_settings(reference: &str) -> Vec<(&'static str, f64)> {
    let Some(m) = find(reference) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    if let Some(r) = m.root {
        v.push(("root", r as f64));
    }
    match m.looped {
        Some((a, b)) => v.extend([("loop", 1.0), ("loop_start", a), ("loop_end", b)]),
        None => v.push(("loop", 0.0)),
    }
    v
}

/// The sound a `made:` reference names. Each is made once and kept.
pub fn sound(reference: &str) -> Option<Sample> {
    static MADE: Mutex<Vec<(&'static str, Sample)>> = Mutex::new(Vec::new());
    let m = find(reference)?;
    if let Some((_, s)) = (MADE.lock().ok()?.iter()).find(|(id, _)| *id == m.id) {
        return Some(s.clone());
    }
    let s = make(m.id)?;
    if let Ok(mut list) = MADE.lock() {
        list.push((m.id, s.clone()));
    }
    Some(s)
}

fn make(id: &str) -> Option<Sample> {
    let r = RATE as f32;
    let mut s = match id {
        "keys" => keys(r),
        "piano" => piano(r),
        "organ" => organ(),
        "mallets" => mallets(r),
        "bell" => bell(r),
        "round_bass" => round_bass(r),
        "picked_bass" => picked_bass(r),
        "saw_bass" => saw_bass(r),
        "sub_bass" => sub_bass(),
        "warm_pad" => warm_pad(),
        "strings" => strings(),
        "glass_pad" => glass_pad(),
        "pluck" => pluck(r),
        "nylon" => nylon(r),
        "harp" => harp(r),
        "kick" => drum(Drum::Kick, 0.6, 0.6, r),
        "deep_kick" => drum_tuned(Drum::Kick, -3.0, 1.2, 0.2, r),
        "tight_kick" => drum_tuned(Drum::Kick, 2.0, 0.25, 0.9, r),
        "snare" => drum(Drum::Snare, 0.3, 0.5, r),
        "tight_snare" => drum_tuned(Drum::Snare, 3.0, 0.18, 0.8, r),
        "clap" => drum(Drum::Clap, 0.35, 0.5, r),
        "rim" => drum(Drum::Rim, 0.06, 0.5, r),
        "hat" => drum(Drum::ClosedHat, 0.12, 0.5, r),
        "open_hat" => drum(Drum::OpenHat, 0.6, 0.5, r),
        "shaker" => shaker(r),
        "low_tom" => drum_tuned(Drum::Tom, -7.0, 0.7, 0.3, r),
        "high_tom" => drum_tuned(Drum::Tom, 6.0, 0.5, 0.3, r),
        "crash" => drum(Drum::Cymbal, 2.0, 0.6, r),
        "ride" => drum_tuned(Drum::Cymbal, 5.0, 3.0, 0.15, r),
        "cowbell" => drum(Drum::Cowbell, 0.35, 0.5, r),
        "loop" => drum_loop(r),
        "loop_90" => laid_back_loop(r),
        "loop_shuffle" => shuffle_loop(r),
        "loop_four" => four_loop(r),
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

/// A sine partial: its frequency (Hz), level, phase (turns) and how
/// fast it dies away (seconds to fall to 1/e; infinite for one held).
#[derive(Clone, Copy)]
struct Partial {
    f: f64,
    amp: f64,
    phase: f64,
    tau: f64,
}

fn partial(f: f64, amp: f64, tau: f64) -> Partial {
    Partial {
        f,
        amp,
        phase: 0.0,
        tau,
    }
}

/// `n` samples of the partials added up, each turned by a rotating
/// phasor rather than a sine a sample (exact enough over seconds, in
/// 64-bit), and those above the top of the band left out.
fn additive(n: usize, partials: &[Partial]) -> Vec<f32> {
    let r = RATE as f64;
    let mut out = vec![0.0f64; n];
    for p in partials.iter().filter(|p| p.f > 0.0 && p.f < 0.45 * r) {
        let w = std::f64::consts::TAU * p.f / r;
        let d = if p.tau.is_finite() {
            (-1.0 / (p.tau * r)).exp()
        } else {
            1.0
        };
        let (dc, ds) = (w.cos() * d, w.sin() * d);
        let ph = std::f64::consts::TAU * p.phase;
        let (mut x, mut y) = (p.amp * ph.cos(), p.amp * ph.sin());
        for o in out.iter_mut() {
            *o += y;
            (x, y) = (x * dc - y * ds, x * ds + y * dc);
        }
    }
    out.into_iter().map(|v| v as f32).collect()
}

/// Shapes the start of a sound: a rise over `secs`, by a raised cosine.
fn fade_in(x: &mut [f32], secs: f32) {
    let n = (secs * RATE as f32) as usize;
    for (i, v) in x.iter_mut().take(n).enumerate() {
        *v *= 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / n as f32).cos();
    }
}

/// A frequency moved to a whole number of cycles in the loop.
fn in_loop(f: f64) -> f64 {
    (f * LOOP).round().max(1.0) / LOOP
}

/// The partials of a saw wave at `f`, through a gentle low-pass at
/// `cutoff` (about 12 dB an octave), up to 8 kHz.
fn saw(f: f64, cutoff: f64, level: f64, noise: &mut crate::dsp::Noise) -> Vec<Partial> {
    let f = in_loop(f);
    (1..)
        .map(|k| k as f64)
        .take_while(|k| k * f < 8000.0)
        .map(|k| {
            let x = k * f / cutoff;
            Partial {
                f: k * f,
                amp: level / k / (1.0 + x.powi(4)).sqrt(),
                phase: noise.tick() as f64 * 0.5 + 0.5,
                tau: f64::INFINITY,
            }
        })
        .collect()
}

/// A held sound: `attack` seconds rising to the partials (held ones,
/// each tuned into the loop), then a loop of [`LOOP`] seconds of them,
/// in stereo.
fn held_sound(attack: f64, left: &[Partial], right: &[Partial]) -> Sample {
    let n = ((attack + LOOP) * RATE as f64).round() as usize;
    let tune = |p: &[Partial]| -> Vec<Partial> {
        (p.iter())
            .map(|p| Partial {
                f: in_loop(p.f),
                tau: f64::INFINITY,
                ..*p
            })
            .collect()
    };
    let mut l = additive(n, &tune(left));
    let mut r = additive(n, &tune(right));
    fade_in(&mut l, attack as f32);
    fade_in(&mut r, attack as f32);
    Sample {
        left: l,
        right: r,
        rate: RATE,
    }
}

/// An acoustic piano note at C4, softly played: each partial a little
/// sharp of a harmonic, as a stiff string's are (Fletcher, 1964), two
/// strings a little apart so they beat, a fast fall then a slow one,
/// and the hammer's thud.
fn piano(r: f32) -> Sample {
    let f = 261.63f64;
    let b = 0.0004;
    let mut v = Vec::new();
    for k in 1..=18 {
        let k = k as f64;
        let fk = k * f * (1.0 + b * k * k).sqrt();
        let a = 1.0 / k.powf(1.1) / (1.0 + (k / 7.0).powi(2));
        let (fast, slow) = (0.5 / k.sqrt(), 4.0 / k.powf(0.6));
        for (df, share) in [(0.0, 0.55), (0.35, 0.45)] {
            v.push(partial(fk + df, a * share * 0.6, fast));
            v.push(partial(fk + df, a * share * 0.4, slow));
        }
    }
    let n = (r * 4.0) as usize;
    let mut x = additive(n, &v);
    thud(&mut x, 0.008, 0.15, 900.0, 11);
    fade_in(&mut x, 0.001);
    Sample::mono(x, RATE)
}

/// Adds a short low noise burst at the start: a hammer, a mallet or a
/// key's click.
fn thud(x: &mut [f32], secs: f32, level: f32, cutoff: f32, seed: u32) {
    let r = RATE as f32;
    let mut noise = crate::dsp::Noise::new(seed);
    let mut lp = crate::dsp::Svf::default();
    lp.set(cutoff, 0.1, r);
    for (i, v) in x.iter_mut().take((secs * r * 6.0) as usize).enumerate() {
        let t = i as f32 / r;
        *v += lp.run(noise.tick(), crate::dsp::Mode::Low) * level * (-t / secs).exp();
    }
}

/// A drawbar organ at C4: the footages under the note, an octave below
/// to three above, held, with the key click at the start.
fn organ() -> Sample {
    let f = 261.63;
    let bars = [
        (0.5, 0.6),
        (1.0, 1.0),
        (2.0, 0.6),
        (3.0, 0.4),
        (4.0, 0.35),
        (8.0, 0.15),
    ];
    let v: Vec<Partial> = (bars.iter())
        .map(|&(h, a)| partial(in_loop(f / 2.0) * 2.0 * h, a, f64::INFINITY))
        .collect();
    let mut s = held_sound(0.02, &v, &v);
    thud(&mut s.left, 0.003, 0.25, 4000.0, 5);
    thud(&mut s.right, 0.003, 0.25, 4000.0, 5);
    s
}

/// A marimba-like bar at C4: a tuned bar's first three modes, at 1, 3.9
/// and 9.2 times its pitch (Fletcher and Rossing, The Physics of Musical
/// Instruments, 1991), the higher dying fast, and the mallet's knock.
fn mallets(r: f32) -> Sample {
    let f = 261.63;
    let v = [
        partial(f, 1.0, 0.6),
        partial(f * 3.9, 0.35, 0.09),
        partial(f * 9.2, 0.12, 0.025),
    ];
    let mut x = additive((r * 2.0) as usize, &v);
    thud(&mut x, 0.002, 0.3, 2500.0, 13);
    fade_in(&mut x, 0.0005);
    Sample::mono(x, RATE)
}

/// A bell at C5, from the bell partials in Risset's catalogue of
/// computer-made sounds (1969): unevenly spaced, the low ones ringing
/// longest, pairs a hertz apart so they beat.
fn bell(r: f32) -> Sample {
    let f = 523.25;
    let ratios = [
        (0.56, 0.6, 2.5),
        (0.92, 0.7, 2.0),
        (1.0, 1.0, 3.0),
        (1.19, 0.6, 1.5),
        (1.71, 0.45, 1.2),
        (2.0, 0.5, 1.0),
        (2.74, 0.35, 0.8),
        (3.0, 0.3, 0.6),
        (3.76, 0.2, 0.5),
        (4.07, 0.15, 0.4),
    ];
    let mut v = Vec::new();
    for (h, a, tau) in ratios {
        v.push(partial(f * h, a * 0.6, tau));
        v.push(partial(f * h + 1.0, a * 0.4, tau));
    }
    let mut x = additive((r * 4.0) as usize, &v);
    fade_in(&mut x, 0.001);
    Sample::mono(x, RATE)
}

/// A round electric bass note at C2, played with the fingers: a strong
/// fundamental, a few harmonics dying faster.
fn round_bass(r: f32) -> Sample {
    let f = 65.41;
    let v = [
        partial(f, 1.0, 1.6),
        partial(f * 2.0, 0.5, 0.9),
        partial(f * 3.0, 0.22, 0.5),
        partial(f * 4.0, 0.12, 0.3),
        partial(f * 5.0, 0.06, 0.2),
        partial(f * 6.0, 0.03, 0.15),
    ];
    let mut x = additive((r * 3.0) as usize, &v);
    fade_in(&mut x, 0.004);
    Sample::mono(x, RATE)
}

/// A bass note at C2 played with a pick, by the Karplus–Strong method
/// as [`pluck`] is, rung longer, with the pick's click.
fn picked_bass(r: f32) -> Sample {
    let f = 65.41;
    let period = (r / f).round() as usize;
    let mut noise = crate::dsp::Noise::new(17);
    let mut line: Vec<f32> = (0..period).map(|_| noise.tick() * 0.5).collect();
    let n = (r * 3.0) as usize;
    let mut out = Vec::with_capacity(n);
    let mut at = 0;
    for _ in 0..n {
        let next = (at + 1) % period;
        let y = 0.4995 * (line[at] + line[next]);
        out.push(line[at]);
        line[at] = y;
        at = next;
    }
    // The first trips round are noise; a low-pass takes the hiss off.
    let mut lp = crate::dsp::Svf::default();
    lp.set(2500.0, 0.0, r);
    let mut x: Vec<f32> = (out.into_iter())
        .map(|v| lp.run(v, crate::dsp::Mode::Low))
        .collect();
    thud(&mut x, 0.002, 0.2, 3000.0, 19);
    fade_in(&mut x, 0.001);
    Sample::mono(x, RATE)
}

/// A synth bass at C2: a saw and a sine an octave under it, through a
/// resonant low-pass that closes fast after the note starts.
fn saw_bass(r: f32) -> Sample {
    use crate::dsp::{Mode, Svf, Wave, osc};
    let f = 65.41f32;
    let n = (r * 2.0) as usize;
    let dt = f / r;
    let mut lp = Svf::default();
    let (mut ph, mut sub) = (0.0f32, 0.0f32);
    let x = (0..n)
        .map(|i| {
            let t = i as f32 / r;
            if i % 16 == 0 {
                lp.set(140.0 + 2600.0 * (-t / 0.12).exp(), 0.35, r);
            }
            let v = osc(Wave::Saw, ph, dt, 0.5) * 0.7 + (sub * TAU).sin() * 0.5;
            ph = (ph + dt).fract();
            sub = (sub + dt * 0.5).fract();
            lp.run(v, Mode::Low) * (-t / 1.2).exp() * (t / 0.002).min(1.0)
        })
        .collect();
    Sample::mono(x, RATE)
}

/// A sub bass at C2, held: a sine with a little of the octave and the
/// twelfth so it is heard on small speakers.
fn sub_bass() -> Sample {
    let f = 65.41;
    let v = [
        partial(f, 1.0, f64::INFINITY),
        partial(in_loop(f) * 2.0, 0.12, f64::INFINITY),
        partial(in_loop(f) * 3.0, 0.04, f64::INFINITY),
    ];
    held_sound(0.01, &v, &v)
}

/// The partials of a few saws a few cents apart.
fn voices(f: f64, cents: &[f64], cutoff: f64, seed: u32) -> Vec<Partial> {
    let mut noise = crate::dsp::Noise::new(seed);
    let level = 1.0 / cents.len() as f64;
    (cents.iter())
        .flat_map(|c| saw(f * (c / 1200.0).exp2(), cutoff, level, &mut noise))
        .collect()
}

/// A warm pad at C4, held: saws a few cents apart on each side, through
/// a low-pass well down, with a soft octave below.
fn warm_pad() -> Sample {
    let f = 261.63;
    let under = |v: &mut Vec<Partial>| v.push(partial(f / 2.0, 0.25, f64::INFINITY));
    let mut l = voices(f, &[-9.0, 2.0], 900.0, 31);
    let mut r = voices(f, &[-3.0, 8.0], 900.0, 37);
    under(&mut l);
    under(&mut r);
    held_sound(0.4, &l, &r)
}

/// A string section at C4, held: more saws, spread wider, brighter.
fn strings() -> Sample {
    let f = 261.63;
    let l = voices(f, &[-14.0, -4.0, 6.0], 2800.0, 41);
    let r = voices(f, &[-9.0, 1.0, 12.0], 2800.0, 43);
    held_sound(0.3, &l, &r)
}

/// A glassy pad at C4, held: a few sine partials, the octaves and the
/// twelfth, two sets a few cents apart.
fn glass_pad() -> Sample {
    let f = 261.63;
    let set = |cents: f64, phase: f64| -> Vec<Partial> {
        let g = f * (cents / 1200.0).exp2();
        [
            (1.0, 1.0),
            (2.0, 0.5),
            (3.0, 0.18),
            (4.0, 0.3),
            (6.0, 0.12),
            (8.0, 0.08),
        ]
        .iter()
        .map(|&(h, a)| Partial {
            f: in_loop(g) * h,
            amp: a * 0.5,
            phase: phase * h,
            tau: f64::INFINITY,
        })
        .collect()
    };
    let l: Vec<Partial> = set(-4.0, 0.0).into_iter().chain(set(3.0, 0.3)).collect();
    let r: Vec<Partial> = set(-2.0, 0.6).into_iter().chain(set(5.0, 0.1)).collect();
    held_sound(0.5, &l, &r)
}

/// A string plucked a fifth of the way along: the harmonics a pluck
/// there gives (every fifth one missing), each dying faster than the
/// one below, a stiff string's stretch, and the finger's touch.
fn plucked(f: f64, at: f64, ring: f64, bright: f64, seconds: f32, seed: u32) -> Sample {
    let v: Vec<Partial> = (1..=40)
        .map(|k| k as f64)
        .map(|k| {
            let a = (std::f64::consts::PI * k * at).sin().abs() / k.powf(bright);
            let fk = k * f * (1.0 + 0.00005 * k * k).sqrt();
            partial(fk, a, ring / (1.0 + 0.15 * k))
        })
        .collect();
    let mut x = additive((RATE as f32 * seconds) as usize, &v);
    thud(&mut x, 0.0015, 0.05, 3000.0, seed);
    fade_in(&mut x, 0.0008);
    Sample::mono(x, RATE)
}

/// A nylon-strung guitar note at C3.
fn nylon(_r: f32) -> Sample {
    plucked(130.81, 0.2, 2.5, 1.0, 3.0, 23)
}

/// A harp note at C4: plucked nearer the middle, softer, ringing on.
fn harp(_r: f32) -> Sample {
    plucked(261.63, 1.0 / 7.0, 3.5, 1.5, 3.0, 29)
}

fn drum_tuned(d: Drum, tune: f32, decay: f32, tone: f32, r: f32) -> Sample {
    let mut h = Hit::new(d, r, tune, decay, tone, 3);
    let mut x = Vec::new();
    while !h.done && x.len() < (r * 4.0) as usize {
        x.push(h.tick() * 0.7);
    }
    Sample::mono(x, RATE)
}

/// A shaker: two quick swells of high noise, the beads hitting the
/// front of the shell and then the back.
fn shaker(r: f32) -> Sample {
    use crate::dsp::{Mode, Noise, Svf};
    let n = (r * 0.25) as usize;
    let mut noise = Noise::new(47);
    let mut bp = Svf::default();
    bp.set(6500.0, 0.3, r);
    let grain = |t: f32, at: f32, level: f32| {
        let t = t - at;
        if t < 0.0 {
            0.0
        } else {
            level * (t / 0.012).min(1.0) * (-t / 0.045).exp()
        }
    };
    let x = (0..n)
        .map(|i| {
            let t = i as f32 / r;
            let e = grain(t, 0.0, 1.0) + grain(t, 0.07, 0.6);
            bp.run(noise.tick(), Mode::Band) * e
        })
        .collect();
    Sample::mono(x, RATE)
}

/// A hit in a pattern: its step, the drum, its tuning, decay and tone,
/// and how loud.
type Step = (usize, Drum, f32, f32, f32, f32);

/// `steps` steps of `secs` each, with these hits on them; a step can be
/// moved later by `late` of a step (a swing).
fn pattern(r: f32, steps: usize, secs: f32, hits: &[Step], late: impl Fn(usize) -> f32) -> Sample {
    let n = (steps as f32 * secs * r).round() as usize;
    let mut x = vec![0.0f32; n];
    for (seed, &(step, d, tune, decay, tone, level)) in hits.iter().enumerate() {
        let at = ((step as f32 + late(step)) * secs * r) as usize;
        let mut h = Hit::new(d, r, tune, decay, tone, 50 + seed as u32);
        let mut i = at;
        while !h.done && i < n {
            x[i] += h.tick() * level;
            i += 1;
        }
    }
    Sample::mono(x, RATE)
}

/// One bar at 90 bpm in sixteenths: a kick that pushes ahead, the snare
/// on two and four with quiet ghost notes, hi-hats on the eighths.
fn laid_back_loop(r: f32) -> Sample {
    let mut hits: Vec<Step> = Vec::new();
    for k in (0..16).step_by(2) {
        let level = if k % 4 == 0 { 0.3 } else { 0.18 };
        hits.push((k, Drum::ClosedHat, 0.0, 0.07, 0.4, level));
    }
    for k in [0, 7, 10] {
        hits.push((k, Drum::Kick, -1.0, 0.55, 0.4, 0.85));
    }
    for k in [4, 12] {
        hits.push((k, Drum::Snare, 0.0, 0.28, 0.4, 0.6));
    }
    for k in [6, 15] {
        hits.push((k, Drum::Snare, 0.0, 0.12, 0.4, 0.12));
    }
    pattern(r, 16, 60.0 / 90.0 / 4.0, &hits, |_| 0.0)
}

/// One bar at 100 bpm with a shuffle: hi-hats on the swung eighths
/// (the second of each pair two-thirds of the way through the beat),
/// kick on one and three, snare on two and four, ghost notes before
/// them.
fn shuffle_loop(r: f32) -> Sample {
    let mut hits: Vec<Step> = Vec::new();
    for k in 0..8 {
        let level = if k % 2 == 0 { 0.3 } else { 0.2 };
        hits.push((k, Drum::ClosedHat, 0.0, 0.08, 0.5, level));
    }
    for k in [0, 4, 5] {
        hits.push((k, Drum::Kick, 0.0, 0.5, 0.5, 0.8));
    }
    for k in [2, 6] {
        hits.push((k, Drum::Snare, 0.0, 0.25, 0.5, 0.6));
    }
    for k in [3, 7] {
        hits.push((k, Drum::Snare, 0.0, 0.1, 0.5, 0.12));
    }
    // The off-beat eighths a third of an eighth late.
    pattern(r, 8, 60.0 / 100.0 / 2.0, &hits, |k| {
        if k % 2 == 1 { 1.0 / 3.0 } else { 0.0 }
    })
}

/// One bar at 124 bpm: a kick on every beat, a clap on two and four, an
/// open hi-hat between the beats and closed ones on the sixteenths in
/// between.
fn four_loop(r: f32) -> Sample {
    let mut hits: Vec<Step> = Vec::new();
    for k in [0, 4, 8, 12] {
        hits.push((k, Drum::Kick, 0.0, 0.45, 0.6, 0.85));
    }
    for k in [4, 12] {
        hits.push((k, Drum::Clap, 0.0, 0.3, 0.5, 0.5));
    }
    for k in [2, 6, 10, 14] {
        hits.push((k, Drum::OpenHat, 0.0, 0.2, 0.5, 0.25));
    }
    for k in (1..16).step_by(2) {
        hits.push((k, Drum::ClosedHat, 0.0, 0.04, 0.6, 0.12));
    }
    pattern(r, 16, 60.0 / 124.0 / 4.0, &hits, |_| 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_made_and_named() {
        for m in SOUNDS {
            let r = reference(m.id);
            let s = sound(&r).unwrap();
            assert!(s.len() > 1000, "{}", m.id);
            let p = s.peak();
            assert!(p > 0.05 && p <= 1.0, "{}: {p}", m.id);
            assert_eq!(super::name(&r), Some(m.name));
            assert!(s.left.iter().chain(&s.right).all(|v| v.is_finite()));
        }
        assert!(sound("made:nothing").is_none());
        assert!(sound("kick.wav").is_none());
        assert_eq!(sound("made:loop").unwrap().seconds(), 2.0);
        // Every kind has some, and no two share an id or a name.
        for g in Group::ALL {
            assert!(SOUNDS.iter().filter(|m| m.group == g).count() >= 3, "{g:?}");
        }
        for (i, a) in SOUNDS.iter().enumerate() {
            for b in &SOUNDS[i + 1..] {
                assert!(a.id != b.id && a.name != b.name, "{}", a.id);
            }
        }
    }

    /// The pitch of a sound with one, by counting where it crosses zero
    /// going up, over a stretch after its start.
    fn pitch(s: &Sample, from: f64, secs: f64) -> f64 {
        let a = (from * RATE as f64) as usize;
        let x = &s.left[a..a + (secs * RATE as f64) as usize];
        let ups = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f64 / secs
    }

    #[test]
    fn pitched_sounds_play_at_their_root_note() {
        for (id, from) in [("sub_bass", 0.5), ("round_bass", 0.2), ("organ", 0.5)] {
            let m = find(&reference(id)).unwrap();
            let want = 440.0 * ((m.root.unwrap() as f64 - 69.0) / 12.0).exp2();
            let got = pitch(&sound(&reference(id)).unwrap(), from, 1.0);
            assert!((got / want - 1.0).abs() < 0.02, "{id}: {got} not {want}");
        }
    }

    #[test]
    fn a_held_sound_loops_without_a_jump() {
        for m in SOUNDS.iter().filter(|m| m.looped.is_some()) {
            let s = sound(&reference(m.id)).unwrap();
            let (a, b) = m.looped.unwrap();
            let start = (a * s.len() as f64).round() as usize;
            let end = ((b * s.len() as f64).round() as usize).min(s.len());
            assert_eq!(end - start, (LOOP * RATE as f64) as usize, "{}", m.id);
            for x in [&s.left, &s.right] {
                // From the last sample round to the loop's first is a
                // step no bigger than one sample to the next anywhere.
                let biggest = (x[start..end].windows(2))
                    .map(|w| (w[1] - w[0]).abs())
                    .fold(0.0f32, f32::max);
                let jump = (x[start] - x[end - 1]).abs();
                assert!(jump <= biggest * 1.01, "{}: {jump} > {biggest}", m.id);
            }
            let z = zone_settings(&reference(m.id));
            assert!(z.contains(&("loop", 1.0)), "{}", m.id);
        }
        assert_eq!(
            zone_settings(&reference("keys")),
            [("root", 60.0), ("loop", 0.0)]
        );
        assert!(zone_settings("song.wav").is_empty());
    }

    #[test]
    fn the_loops_are_a_bar_at_their_tempo() {
        for (id, bpm) in [
            ("loop_90", 90.0),
            ("loop_shuffle", 100.0),
            ("loop_four", 124.0),
        ] {
            let s = sound(&reference(id)).unwrap();
            assert!((s.seconds() - 4.0 * 60.0 / bpm).abs() < 0.001, "{id}");
            assert_eq!(crate::slicer::guess_beats(s.seconds()), 4.0, "{id}");
        }
    }
}
