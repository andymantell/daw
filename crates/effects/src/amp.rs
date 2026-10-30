// SPDX-License-Identifier: GPL-3.0-or-later
//! A guitar amp: one to three gain stages that clip softly and a little
//! unevenly (as a valve stage biased off centre does), a bass, middle and
//! treble control, presence, a power stage that sags when pushed, and a
//! speaker cabinet after it.
//!
//! Our own design from textbook behaviour of valve guitar amps, as
//! described in the literature (for example D. T. Yeh, "Digital
//! Implementation of Musical Distortion Circuits by Analysis and
//! Simulation", Stanford, 2009; and U. Zölzer (ed.), "DAFX", chapter on
//! nonlinear processing). It is not a model of any one amp. Everything
//! between the input filters and the output runs at four times the
//! sample rate, so the harmonics the clipping makes don't fold back as
//! inharmonic whistles. It plays in mono, as an amp does.

use crate::biquad::{Biquad, Coefs};
use crate::cabinet::{self, CABINETS};
use crate::convolution::Convolution;
use crate::{CHUNK, Dsp, Param, Unit};

pub const VOICES: &[&str] = &["Clean", "Warm", "Crunch", "High gain"];

/// The cabinet list, with no cabinet first.
const CAB_CHOICES: &[&str] = &[
    "None",
    CABINETS[0],
    CABINETS[1],
    CABINETS[2],
    CABINETS[3],
    CABINETS[4],
];

pub const PARAMS: &[Param] = &[
    Param::new(
        "voice",
        "Voice",
        0.0,
        (VOICES.len() - 1) as f64,
        2.0,
        Unit::Choice,
    )
    .choices(VOICES),
    Param::new("gain", "Gain", 0.0, 10.0, 5.0, Unit::Plain),
    Param::new("bass", "Bass", 0.0, 10.0, 5.0, Unit::Plain),
    Param::new("middle", "Middle", 0.0, 10.0, 5.0, Unit::Plain),
    Param::new("treble", "Treble", 0.0, 10.0, 5.0, Unit::Plain),
    Param::new("presence", "Presence", 0.0, 10.0, 5.0, Unit::Plain),
    Param::new("volume", "Volume", -24.0, 12.0, 0.0, Unit::Db),
    Param::new(
        "cabinet",
        "Cabinet",
        0.0,
        (CAB_CHOICES.len() - 1) as f64,
        4.0,
        Unit::Choice,
    )
    .choices(CAB_CHOICES),
];

/// How much the input is oversampled.
const OVER: usize = 4;

/// How one voice is set up.
struct Voice {
    /// Gain stages that clip.
    stages: usize,
    /// The first stage's gain at the gain control's bottom and top, dB.
    drive: (f64, f64),
    /// Each later stage's gain, dB.
    later: f64,
    /// How far off centre each stage is biased: more is more uneven
    /// clipping, and more even harmonics.
    bias: f64,
    /// Bass cut before the gain, so heavy clipping stays tight, Hz.
    tight: f64,
    /// A treble lift before the gain, dB at 2.5 kHz and up.
    bright: f64,
    /// Where the tone controls work: bass shelf, middle, treble shelf.
    tone: (f64, f64, f64),
    /// The middle's dip with its control at half, dB.
    scoop: f64,
    /// How hard the power stage is driven, and how much it sags.
    power: f64,
    sag: f64,
    /// How much of the gain control's rise in level is taken back
    /// after the amp, 0 to 1, so it adds grit more than loudness.
    follow: f64,
    /// Brings the voice to about the level it came in at, dB.
    trim: f64,
}

const VOICE: [Voice; 4] = [
    Voice {
        stages: 1,
        drive: (0.0, 18.0),
        later: 0.0,
        bias: 0.08,
        tight: 60.0,
        bright: 3.0,
        tone: (120.0, 600.0, 2800.0),
        scoop: -3.0,
        power: 0.8,
        sag: 0.3,
        follow: 0.5,
        trim: -3.5,
    },
    Voice {
        stages: 2,
        drive: (-6.0, 20.0),
        later: 12.0,
        bias: 0.15,
        tight: 70.0,
        bright: 1.5,
        tone: (110.0, 700.0, 2600.0),
        scoop: -2.0,
        power: 1.2,
        sag: 0.6,
        follow: 0.4,
        trim: -10.5,
    },
    Voice {
        stages: 2,
        drive: (0.0, 30.0),
        later: 16.0,
        bias: 0.2,
        tight: 90.0,
        bright: 2.0,
        tone: (100.0, 800.0, 2400.0),
        scoop: -1.0,
        power: 1.5,
        sag: 0.5,
        follow: 0.15,
        trim: -13.5,
    },
    Voice {
        stages: 3,
        drive: (6.0, 36.0),
        later: 18.0,
        bias: 0.25,
        tight: 140.0,
        bright: 0.0,
        tone: (90.0, 650.0, 2200.0),
        scoop: -4.0,
        power: 1.8,
        sag: 0.3,
        follow: 0.0,
        trim: -18.0,
    },
];

/// The level of a control's 0 to 10 as -1 to 1.
fn centred(v: f64) -> f64 {
    (v - 5.0) / 5.0
}

/// Soft clipping, uneven by `bias`, passing silence as silence.
#[inline]
fn stage(x: f64, bias: f64) -> f64 {
    (x + bias).tanh() - bias.tanh()
}

pub struct Amp {
    rate: f64,
    voice: usize,
    /// Before the gain, at the sample rate.
    tight: Biquad,
    bright: Biquad,
    drive: f64,
    /// The gain control's share of the level taken back, as a gain.
    follow: f64,
    /// Up and down the oversampled rate.
    up: [Biquad; 4],
    down: [Biquad; 4],
    /// Between stages: a cut of the bass (each stage's coupling) and the
    /// treble (each stage's own roll-off), per stage.
    between: [[Biquad; 2]; 3],
    tone: [Biquad; 3],
    presence: Biquad,
    /// How far the power stage is sagging now, 0 to 1.
    sag_env: f64,
    sag_up: f64,
    sag_down: f64,
    out_gain: f64,
    /// The cabinet, and how much of it is heard (moving to `cab_want`).
    cab: Convolution,
    cab_want: f32,
    cab_mix: f32,
    cab_step: f32,
    mono: [f32; CHUNK],
    wet: [f32; CHUNK],
    started: bool,
    settings: [f64; 8],
}

impl Amp {
    pub fn new(rate: f64) -> Amp {
        let over = rate * OVER as f64;
        // An eighth-order Butterworth low pass at the oversampled rate,
        // just under the sample rate's top: its four sections' Qs.
        let qs = [0.5098, 0.6013, 0.9000, 2.5629];
        let edge = (rate * 0.45).min(20_000.0);
        let lp = |q: f64| Biquad::new(Coefs::low_pass(edge, q, over));
        Amp {
            rate,
            voice: usize::MAX,
            tight: Biquad::default(),
            bright: Biquad::default(),
            drive: 1.0,
            follow: 1.0,
            up: qs.map(lp),
            down: qs.map(lp),
            between: Default::default(),
            tone: Default::default(),
            presence: Biquad::default(),
            sag_env: 0.0,
            sag_up: crate::coef(0.004, over),
            sag_down: crate::coef(0.15, over),
            out_gain: 1.0,
            cab: Convolution::with(rate, cabinet::all(rate), None),
            cab_want: 0.0,
            cab_mix: 0.0,
            // A change of cabinet on or off takes about 10 ms.
            cab_step: (1.0 / (0.01 * rate)) as f32,
            mono: [0.0; CHUNK],
            wet: [0.0; CHUNK],
            started: false,
            settings: [f64::NAN; 8],
        }
    }

    fn voice(&self) -> &'static Voice {
        &VOICE[self.voice.min(VOICE.len() - 1)]
    }

    /// One sample at the oversampled rate.
    #[inline]
    fn over(&mut self, x: f64) -> f64 {
        let v = self.voice();
        let mut x = x * self.drive;
        let later = crate::gain(v.later);
        for s in 0..v.stages {
            if s > 0 {
                x *= later;
            }
            x = stage(x, v.bias);
            let [low, high] = &mut self.between[s];
            x = high.run(low.run(x as f32)) as f64;
        }
        for t in &mut self.tone {
            x = t.run(x as f32) as f64;
        }
        x = self.presence.run(x as f32) as f64;
        // The power stage: the harder it works, the more its supply
        // droops and the less gain it has, for a moment.
        let level = x.abs();
        let c = if level > self.sag_env {
            self.sag_up
        } else {
            self.sag_down
        };
        self.sag_env = level + (self.sag_env - level) * c;
        let p = v.power / (1.0 + v.sag * self.sag_env);
        (p * x).tanh() / v.power
    }
}

impl Dsp for Amp {
    fn update(&mut self, v: &[f64]) {
        let rate = self.rate;
        let over = rate * OVER as f64;
        let voice = v[0] as usize;
        let changed = |i: usize, s: &[f64; 8]| s[i] != v[i];
        if voice != self.voice {
            self.voice = voice;
            let vc = self.voice();
            self.tight.c = Coefs::high_pass(vc.tight, 0.7, rate);
            self.bright.c = Coefs::high_shelf(2500.0, vc.bright, rate);
            for (s, [low, high]) in self.between.iter_mut().enumerate() {
                // Each stage's bass cut a little higher, as coupling
                // capacitors are chosen in amps that clip more.
                let s = s as f64;
                low.c = Coefs::high_pass(25.0 + 20.0 * s, 0.7, over);
                high.c = Coefs::low_pass(9000.0 - 1500.0 * s, 0.7, over);
            }
            self.settings = [f64::NAN; 8];
        }
        let vc = self.voice();
        let s = self.settings;
        if changed(1, &s) {
            let (lo, hi) = vc.drive;
            // Moves more slowly at the bottom, as a gain control does.
            let k = v[1] / 10.0;
            let db = lo + (hi - lo) * k.powf(0.7);
            self.drive = crate::gain(db);
            self.follow = crate::gain(-db * vc.follow);
        }
        if changed(2, &s) {
            self.tone[0].c = Coefs::low_shelf(vc.tone.0, centred(v[2]) * 12.0, over);
        }
        if changed(3, &s) {
            let db = vc.scoop + centred(v[3]) * 10.0;
            self.tone[1].c = Coefs::peak(vc.tone.1, 0.8, db, over);
        }
        if changed(4, &s) {
            self.tone[2].c = Coefs::high_shelf(vc.tone.2, centred(v[4]) * 12.0, over);
        }
        if changed(5, &s) {
            self.presence.c = Coefs::peak(4500.0, 0.7, centred(v[5]) * 8.0, over);
        }
        if changed(1, &s) || changed(6, &s) {
            self.out_gain = crate::gain(v[6] + vc.trim) * self.follow;
        }
        let cab = v[7] as usize;
        if cab > 0 {
            self.cab.update(&[(cab - 1) as f64, 0.0, 100.0]);
        }
        self.cab_want = if cab > 0 { 1.0 } else { 0.0 };
        if !self.started {
            self.cab_mix = self.cab_want;
            self.started = true;
        }
        self.settings.copy_from_slice(&v[..8]);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len()).min(CHUNK);
        for i in 0..n {
            let x = self.bright.run(self.tight.run((left[i] + right[i]) * 0.5));
            let mut y = 0.0;
            for k in 0..OVER {
                // Each input sample, then silence, made smooth by the
                // low pass: four times the samples, at the same level.
                let u = if k == 0 { x * OVER as f32 } else { 0.0 };
                let u = self.up.iter_mut().fold(u, |u, f| f.run(u));
                let o = self.over(u as f64) as f32;
                let d = self.down.iter_mut().fold(o, |d, f| f.run(d));
                if k == 0 {
                    y = d;
                }
            }
            self.mono[i] = crate::flush((y as f64 * self.out_gain) as f32);
        }
        // The cabinet hears the amp in mono, and gives both sides.
        let (m, w) = (&self.mono[..n], &mut self.wet[..n]);
        w.copy_from_slice(m);
        let mut spare = [0f32; CHUNK];
        let spare = &mut spare[..n];
        spare.copy_from_slice(m);
        if self.cab_want > 0.0 || self.cab_mix > 0.0 {
            self.cab.process(w, spare);
        }
        for i in 0..n {
            if self.cab_mix != self.cab_want {
                let d = (self.cab_want - self.cab_mix).clamp(-self.cab_step, self.cab_step);
                self.cab_mix += d;
            }
            let out = self.mono[i] * (1.0 - self.cab_mix) + self.wet[i] * self.cab_mix;
            left[i] = out;
            right[i] = out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;
    use std::f64::consts::PI;

    /// A guitar-like note: a decaying tone with harmonics, at about
    /// -18 dB.
    fn note(rate: f64, seconds: f64) -> Vec<f32> {
        note_with(rate, seconds, 6)
    }

    fn note_with(rate: f64, seconds: f64, harmonics: usize) -> Vec<f32> {
        (0..(rate * seconds) as usize)
            .map(|n| {
                let t = n as f64 / rate;
                let s: f64 = (1..harmonics)
                    .map(|h| (2.0 * PI * 196.0 * h as f64 * t).sin() / h as f64)
                    .sum();
                (s * 0.13 * (-t * 1.5).exp()) as f32
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn play(e: &Effect, input: &[f32], rate: u32) -> Vec<f32> {
        let mut p = e.processor(rate);
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(a, b);
        }
        assert_eq!(l, r, "the amp plays in mono");
        l
    }

    /// Every voice, at every gain, stays within a few dB of where it
    /// started (the trims), so turning the gain up adds grit more than
    /// level.
    #[test]
    fn voices_stay_near_the_level_they_came_in_at() {
        let input = note(48_000.0, 1.0);
        let rin = crate::db(rms(&input));
        for (v, name) in VOICES.iter().enumerate() {
            for gain in [0.0, 5.0, 10.0] {
                let e = Effect::new(Kind::Amp);
                e.set(0, v as f64);
                e.set(1, gain);
                let out = play(&e, &input, 48_000);
                let rout = crate::db(rms(&out));
                assert!(
                    (rout - rin).abs() < 6.0,
                    "{name} at {gain}: {rout:.1} dB from {rin:.1} dB"
                );
            }
        }
    }

    /// The share of a tone's energy that is not the tone: none clean
    /// and quiet, a lot with the gain up.
    fn distortion(e: &Effect, rate: f64) -> f64 {
        let hz = 220.0;
        let n = rate as usize;
        let input: Vec<f32> = (0..n)
            .map(|i| ((2.0 * PI * hz * i as f64 / rate).sin() * 0.1) as f32)
            .collect();
        let out = play(e, &input, rate as u32);
        let tail = &out[n / 2..];
        let (mut re, mut im) = (0.0, 0.0);
        for (i, x) in tail.iter().enumerate() {
            let w = 2.0 * PI * hz * (i + n / 2) as f64 / rate;
            re += *x as f64 * w.cos();
            im += *x as f64 * w.sin();
        }
        let len = tail.len() as f64;
        let fund = 2.0 * (re * re + im * im) / (len * len);
        let all: f64 = tail.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / len;
        ((all - fund) / all).max(0.0)
    }

    #[test]
    #[ignore]
    fn print_levels() {
        let input = note(48_000.0, 1.0);
        let rin = crate::db(rms(&input));
        for (v, name) in VOICES.iter().enumerate() {
            for gain in [0.0, 5.0, 10.0] {
                let e = Effect::new(Kind::Amp);
                e.set(0, v as f64);
                e.set(1, gain);
                let out = play(&e, &input, 48_000);
                e.set(7, 0.0);
                let raw = play(&e, &input, 48_000);
                println!(
                    "{name:<10} gain {gain:>4}: {:+.1} dB (no cab {:+.1}), grit {:.3}",
                    crate::db(rms(&out)) - rin,
                    crate::db(rms(&raw)) - rin,
                    distortion(&e, 48_000.0)
                );
            }
        }
    }

    #[test]
    fn gain_adds_grit() {
        let rate = 48_000.0;
        let clean = Effect::new(Kind::Amp);
        clean.set(0, 0.0);
        clean.set(1, 0.0);
        clean.set(7, 0.0);
        let dirty = Effect::new(Kind::Amp);
        dirty.set(0, 3.0);
        dirty.set(1, 10.0);
        dirty.set(7, 0.0);
        let (c, d) = (distortion(&clean, rate), distortion(&dirty, rate));
        assert!(c < 0.05, "clean: {c}");
        // A square wave's harmonics are a fifth of it; the filters
        // between stages and the tone controls take some off.
        assert!(d > 0.08, "high gain: {d}");
    }

    /// Turned all the way up, the clipping's harmonics stay mostly where
    /// they belong instead of folding back below the note.
    #[test]
    fn oversampling_keeps_harmonics_from_folding_back() {
        let rate = 44_100.0;
        let e = Effect::new(Kind::Amp);
        e.set(0, 3.0);
        e.set(1, 10.0);
        e.set(4, 10.0);
        e.set(7, 0.0);
        let hz = 3100.0;
        let n = rate as usize;
        let input: Vec<f32> = (0..n)
            .map(|i| ((2.0 * PI * hz * i as f64 / rate).sin() * 0.2) as f32)
            .collect();
        let out = play(&e, &input, rate as u32);
        let size = 16_384;
        let mut tail: Vec<f32> = (out[n - size..].iter().enumerate())
            .map(|(i, x)| x * (PI * i as f64 / size as f64).sin().powi(2) as f32)
            .collect();
        let mut fft = realfft::RealFftPlanner::<f32>::new();
        let plan = fft.plan_fft_forward(size);
        let mut spec = plan.make_output_vec();
        plan.process(&mut tail, &mut spec).unwrap();
        let bin = |hz: f64| (hz / rate * size as f64) as usize;
        let power =
            |a: usize, b: usize| spec[a..b].iter().map(|c| c.norm_sqr() as f64).sum::<f64>();
        // Only harmonics folded back can be below the note.
        let folded = power(bin(60.0), bin(hz - 200.0));
        let all = power(bin(60.0), spec.len());
        let db = 10.0 * (folded / all).log10();
        assert!(db < -40.0, "{db:.1} dB");
    }

    /// The share of `x`'s energy above `hz`.
    fn share_above(x: &[f32], hz: f64, rate: f64) -> f64 {
        let mut planner = realfft::RealFftPlanner::<f32>::new();
        let plan = planner.plan_fft_forward(x.len());
        let mut buf = x.to_vec();
        let mut spec = plan.make_output_vec();
        plan.process(&mut buf, &mut spec).unwrap();
        let at = (hz / rate * x.len() as f64) as usize;
        let power = |s: &[realfft::num_complex::Complex32]| {
            s.iter().map(|c| c.norm_sqr() as f64).sum::<f64>()
        };
        power(&spec[at..]) / power(&spec)
    }

    #[test]
    fn the_cabinet_takes_the_fizz_off() {
        let rate = 48_000.0;
        let fizz = |cab: f64| {
            let e = Effect::new(Kind::Amp);
            e.set(0, 3.0);
            e.set(1, 10.0);
            e.set(7, cab);
            let out = play(&e, &note(rate, 0.5), rate as u32);
            share_above(&out[4800..], 6000.0, rate)
        };
        let (none, cab) = (fizz(0.0), fizz(4.0));
        // At least 13 dB less of it.
        assert!(cab < none * 0.05, "{cab} against {none}");
    }

    #[test]
    fn tone_controls_move_the_sound() {
        let rate = 48_000.0;
        let input = note_with(rate, 0.5, 30);
        let bright = |treble: f64| {
            let e = Effect::new(Kind::Amp);
            e.set(0, 0.0);
            e.set(1, 2.0);
            e.set(4, treble);
            e.set(7, 0.0);
            let out = play(&e, &input, rate as u32);
            let diff: Vec<f32> = out.windows(2).map(|w| w[1] - w[0]).collect();
            rms(&diff) / rms(&out)
        };
        assert!(bright(10.0) > bright(0.0) * 1.5);
    }
}
