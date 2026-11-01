// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects for lo-fi character: a worn record (hiss, crackle, rumble and
//! a narrowed band), a worn tape (wow and flutter, saturation, dull
//! highs and hiss) and a bit crusher (fewer bits and a lower sample
//! rate).
//!
//! The record's crackle is short bursts of noise at random times (a
//! Poisson process), most of them faint and a few loud, each placed at
//! its own point between the speakers. The tape's wow and flutter read a
//! delay line at a time that moves, as a chorus does (J. Dattorro,
//! "Effect Design, Part 2: Delay-Line Modulation and Chorus", JAES
//! 45(10), 1997): a slow wave plus a slow random drift for the wow, a
//! faster one for the flutter. Its saturation is the app's own
//! saturation effect, one warm stage. The bit crusher rounds each sample
//! to a step of the bit depth and holds it for as long as a sample lasts
//! at the lower rate, as U. Zölzer (ed.), "DAFX", describes for
//! quantisation and sample-and-hold; it can smooth the steps with low
//! passes on either side of the hold, or leave the aliasing in.

use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use crate::biquad::{Biquad, Coefs};
use crate::saturation::Saturation;
use crate::{Dsp, Param, Unit, delay_read, flush, gain};

/// Noise from a linear congruential generator (the constants of
/// Numerical Recipes): from -1 to 1.
#[derive(Clone, Copy)]
struct Noise(u32);

impl Noise {
    #[inline]
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1 << 23) as f32 - 1.0
    }

    /// From 0 to 1.
    #[inline]
    fn unit(&mut self) -> f32 {
        0.5 * (self.next() + 1.0)
    }
}

/// White noise with every value between -1 and 1 equally likely has
/// this level (its RMS).
const NOISE_RMS: f32 = 0.577_350_3;

// ---------------------------------------------------------------- record

pub const VINYL: &[Param] = &[
    Param::new("hiss", "Hiss", -80.0, -20.0, -54.0, Unit::Db).off_at(-80.0),
    Param::new("crackle", "Crackle", 0.0, 100.0, 40.0, Unit::Percent),
    Param::new(
        "crackle-level",
        "Crackle level",
        -48.0,
        0.0,
        -18.0,
        Unit::Db,
    ),
    Param::new("rumble", "Rumble", -80.0, -20.0, -60.0, Unit::Db).off_at(-80.0),
    Param::new("wear", "Wear", 0.0, 100.0, 30.0, Unit::Percent),
];

/// Crackles a second at full crackle.
const MOST_CRACKLES: f64 = 200.0;
/// How long a crackle takes to fade, in seconds: a faint one, and how
/// much longer a loud one rings.
const CRACKLE_SECONDS: f64 = 0.000_2;
const POP_SECONDS: f64 = 0.001_5;
/// Where rumble sits, Hz.
const RUMBLE_HZ: f64 = 25.0;

pub struct Vinyl {
    rate: f64,
    noise: Noise,
    hiss: f32,
    /// The chance of a crackle starting in a frame.
    chance: f32,
    crackle: f32,
    rumble: f32,
    /// The crackle sounding now: its level on each side, and how fast it
    /// fades.
    env: [f32; 2],
    fade: f32,
    /// The colour of the crackle (it is bright), and the rumble.
    click: [Biquad; 2],
    low: Biquad,
    /// The band a worn record keeps.
    band: [[Biquad; 2]; 2],
}

impl Vinyl {
    pub fn new(rate: f64) -> Vinyl {
        let click = Biquad::new(Coefs::high_pass(1500.0, FRAC_1_SQRT_2, rate));
        Vinyl {
            rate,
            noise: Noise(0x5eed),
            hiss: 0.0,
            chance: 0.0,
            crackle: 0.0,
            rumble: 0.0,
            env: [0.0; 2],
            fade: 0.0,
            click: [click; 2],
            low: Biquad::new(Coefs::low_pass(RUMBLE_HZ, FRAC_1_SQRT_2, rate)),
            band: [[Biquad::default(); 2]; 2],
        }
    }
}

/// `x` (0 to 1) of the way from `a` to `b`, evenly in ratio.
fn towards(a: f64, b: f64, x: f64) -> f64 {
    a * (b / a).powf(x)
}

impl Dsp for Vinyl {
    fn update(&mut self, v: &[f64]) {
        let off = |i: usize| VINYL[i].off_at == Some(v[i]);
        // White noise's level, brought to the setting.
        self.hiss = if off(0) {
            0.0
        } else {
            gain(v[0]) as f32 / NOISE_RMS
        };
        let density = (v[1] / 100.0).powi(2) * MOST_CRACKLES;
        self.chance = (density / self.rate) as f32;
        self.crackle = gain(v[2]) as f32;
        // Noise through the low pass keeps about 1.1 times the corner
        // of its band: brought back to the setting's level.
        let kept = (2.0 * 1.11 * RUMBLE_HZ / self.rate).sqrt() as f32;
        self.rumble = if off(3) {
            0.0
        } else {
            gain(v[3]) as f32 / (NOISE_RMS * kept)
        };
        let wear = v[4] / 100.0;
        let (lp, hp) = if wear <= 0.0 {
            (Coefs::THROUGH, Coefs::THROUGH)
        } else {
            (
                Coefs::low_pass(towards(20_000.0, 5_000.0, wear), FRAC_1_SQRT_2, self.rate),
                Coefs::high_pass(towards(10.0, 120.0, wear), FRAC_1_SQRT_2, self.rate),
            )
        };
        for b in &mut self.band {
            b[0].c = lp;
            b[1].c = hp;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let fade_short = (-1.0 / (CRACKLE_SECONDS * self.rate)).exp() as f32;
        let fade_long = (-1.0 / (POP_SECONDS * self.rate)).exp() as f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.chance > 0.0 && self.noise.unit() < self.chance {
                // Most crackles faint, a few loud; a loud one rings
                // longer.
                let size = self.noise.unit().powi(3);
                let level = self.crackle * size;
                let pan = 0.5 + 0.35 * self.noise.next();
                self.env = [level * (1.0 - pan).sqrt(), level * pan.sqrt()];
                self.fade = fade_short + (fade_long - fade_short) * size;
            }
            let burst = self.noise.next();
            let rumble = self.rumble * self.low.run(self.noise.next());
            let mut out = [*l, *r];
            for (side, o) in out.iter_mut().enumerate() {
                let crackle = self.click[side].run(burst * self.env[side]);
                self.env[side] = flush(self.env[side] * self.fade);
                let hiss = self.hiss * self.noise.next();
                let b = &mut self.band[side];
                let x = {
                    let y = b[0].run(*o + hiss + crackle);
                    b[1].run(y)
                };
                *o = x + rumble;
            }
            *l = out[0];
            *r = out[1];
        }
    }
}

// ------------------------------------------------------------------ tape

pub const TAPE: &[Param] = &[
    Param::new("wow", "Wow", 0.0, 100.0, 25.0, Unit::Percent),
    Param::new("wow-rate", "Wow speed", 0.1, 4.0, 0.6, Unit::Hz).log(),
    Param::new("flutter", "Flutter", 0.0, 100.0, 20.0, Unit::Percent),
    Param::new("flutter-rate", "Flutter speed", 2.0, 20.0, 7.0, Unit::Hz).log(),
    Param::new("saturation", "Saturation", 0.0, 24.0, 6.0, Unit::Db).off_at(0.0),
    Param::new("highs", "Highs", 2000.0, 20000.0, 12000.0, Unit::Hz)
        .log()
        .off_at(20000.0),
    Param::new("hiss", "Hiss", -80.0, -30.0, -80.0, Unit::Db).off_at(-80.0),
];

/// The most the wow and the flutter move the delay either way, ms: at
/// 0.6 Hz, full wow bends the pitch by about 1.5%; at 7 Hz, full flutter
/// by about 0.4%.
const WOW_MS: f64 = 4.0;
const FLUTTER_MS: f64 = 0.1;
/// The delay the wobble swings around, ms, so it never reaches zero.
const TAPE_DELAY_MS: f64 = WOW_MS + FLUTTER_MS + 0.5;
/// How much of the wow is a slow random drift rather than a steady wave.
const DRIFT: f64 = 0.35;

pub struct Tape {
    rate: f64,
    lines: [Vec<f32>; 2],
    at: usize,
    /// The delay the wobble swings around, frames.
    centre: f64,
    wow: f64,
    wow_hz: f64,
    flutter: f64,
    flutter_hz: f64,
    wow_phase: f64,
    flutter_phase: f64,
    /// The drift: noise smoothed twice, kept between -1 and 1.
    noise: Noise,
    drift: [f64; 2],
    saturate: bool,
    saturation: Saturation,
    highs: [[Biquad; 2]; 2],
    hiss: f32,
}

impl Tape {
    pub fn new(rate: f64) -> Tape {
        // A whole number of frames, so with no wobble it is a plain
        // delay of its latency.
        let centre = (TAPE_DELAY_MS / 1000.0 * rate).round();
        let len = ((TAPE_DELAY_MS + WOW_MS + FLUTTER_MS) / 1000.0 * rate) as usize + 4;
        Tape {
            rate,
            lines: [vec![0.0; len], vec![0.0; len]],
            at: 0,
            centre,
            wow: 0.0,
            wow_hz: 1.0,
            flutter: 0.0,
            flutter_hz: 1.0,
            wow_phase: 0.0,
            flutter_phase: 0.25,
            noise: Noise(0x7a9e),
            drift: [0.0; 2],
            saturate: false,
            saturation: Saturation::new(rate),
            highs: [[Biquad::default(); 2]; 2],
            hiss: 0.0,
        }
    }
}

impl Dsp for Tape {
    fn update(&mut self, v: &[f64]) {
        let ms = self.rate / 1000.0;
        self.wow = v[0] / 100.0 * WOW_MS * ms;
        self.wow_hz = v[1];
        self.flutter = v[2] / 100.0 * FLUTTER_MS * ms;
        self.flutter_hz = v[3];
        self.saturate = TAPE[4].off_at != Some(v[4]);
        // One warm stage, its tone left open: the highs are below.
        self.saturation.update(&[v[4], 0.0, 1.0, 20_000.0, 0.0]);
        let c = if TAPE[5].off_at == Some(v[5]) {
            [Coefs::THROUGH; 2]
        } else {
            // Fourth order (two Butterworth sections), as worn tape
            // loses its top quickly past the corner.
            [0.5412, 1.3066].map(|q| Coefs::low_pass(v[5], q, self.rate))
        };
        for side in &mut self.highs {
            side[0].c = c[0];
            side[1].c = c[1];
        }
        self.hiss = if TAPE[6].off_at == Some(v[6]) {
            0.0
        } else {
            gain(v[6]) as f32 / NOISE_RMS
        };
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = self.lines[0].len();
        // The drift's smoothing: a corner near the wow's speed.
        let k = 1.0 - (-TAU * self.wow_hz / self.rate).exp();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.lines[0][self.at] = *l;
            self.lines[1][self.at] = *r;
            let n = self.noise.next() as f64 * 6.0;
            self.drift[0] += (n - self.drift[0]) * k;
            self.drift[1] += (self.drift[0] - self.drift[1]) * k;
            let drift = self.drift[1].clamp(-1.0, 1.0);
            let wow = (1.0 - DRIFT) * (TAU * self.wow_phase).sin() + DRIFT * drift;
            let flutter = (TAU * self.flutter_phase).sin();
            let d = self.centre + self.wow * wow + self.flutter * flutter;
            *l = delay_read(&self.lines[0], self.at, d);
            *r = delay_read(&self.lines[1], self.at, d);
            self.at = (self.at + 1) % len;
            self.wow_phase = (self.wow_phase + self.wow_hz / self.rate).fract();
            self.flutter_phase = (self.flutter_phase + self.flutter_hz / self.rate).fract();
        }
        if self.saturate {
            self.saturation.process(left, right);
        }
        for (side, buf) in [left, right].into_iter().enumerate() {
            let h = &mut self.highs[side];
            for x in buf.iter_mut() {
                let hiss = self.hiss * self.noise.next();
                *x = {
                    let y = h[0].run(*x + hiss);
                    h[1].run(y)
                };
            }
        }
    }

    fn latency(&self) -> u32 {
        self.centre.round() as u32
    }
}

// ----------------------------------------------------------- bit crusher

pub const CRUSHER: &[Param] = &[
    Param::new("bits", "Bits", 1.0, 24.0, 8.0, Unit::Plain).off_at(24.0),
    Param::new("rate", "Sample rate", 400.0, 48000.0, 12000.0, Unit::Hz)
        .log()
        .off_at(48000.0),
    Param::new("drive", "Drive", -12.0, 24.0, 0.0, Unit::Db),
    Param::new("smooth", "Smooth steps", 0.0, 1.0, 0.0, Unit::Switch),
    Param::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub struct Crusher {
    rate: f64,
    /// The size of a step, or none for no rounding.
    step: Option<f32>,
    /// How far through a held sample each frame moves, or none to hold
    /// nothing.
    hold: Option<f64>,
    phase: f64,
    held: [f32; 2],
    drive: f32,
    smooth: bool,
    /// Low passes either side of the hold, for each side.
    before: [[Biquad; 4]; 2],
    after: [[Biquad; 4]; 2],
    wet: f32,
}

impl Crusher {
    pub fn new(rate: f64) -> Crusher {
        Crusher {
            rate,
            step: None,
            hold: None,
            phase: 0.0,
            held: [0.0; 2],
            drive: 1.0,
            smooth: false,
            before: [[Biquad::default(); 4]; 2],
            after: [[Biquad::default(); 4]; 2],
            wet: 1.0,
        }
    }
}

impl Dsp for Crusher {
    fn update(&mut self, v: &[f64]) {
        // From -1 to 1 in 2^bits steps.
        self.step = (CRUSHER[0].off_at != Some(v[0])).then(|| (2.0 / v[0].exp2()) as f32);
        self.hold = (CRUSHER[1].off_at != Some(v[1]) && v[1] < self.rate).then(|| v[1] / self.rate);
        self.drive = gain(v[2]) as f32;
        self.smooth = v[3] >= 0.5;
        if self.smooth {
            // Eighth order (four Butterworth sections), a little under
            // the top of the lower rate's band.
            let edge = v[1].min(self.rate) * 0.42;
            let c = [0.5098, 0.6013, 0.9000, 2.5629].map(|q| Coefs::low_pass(edge, q, self.rate));
            for f in self.before.iter_mut().chain(self.after.iter_mut()) {
                for (f, c) in f.iter_mut().zip(c) {
                    f.c = c;
                }
            }
        }
        self.wet = (v[4] / 100.0) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (g, back) = (self.drive, 1.0 / self.drive);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dry = [*l, *r];
            let mut x = dry;
            if self.smooth && self.hold.is_some() {
                for (side, v) in x.iter_mut().enumerate() {
                    *v = self.before[side].iter_mut().fold(*v, |v, f| f.run(v));
                }
            }
            let take = match self.hold {
                Some(h) => {
                    self.phase += h;
                    let t = self.phase >= 1.0;
                    if t {
                        self.phase -= 1.0;
                    }
                    t
                }
                None => true,
            };
            if take {
                for (side, v) in x.iter().enumerate() {
                    let u = (v * g).clamp(-1.0, 1.0);
                    let u = match self.step {
                        Some(s) => ((u / s).round() * s).clamp(-1.0, 1.0),
                        None => u,
                    };
                    self.held[side] = u * back;
                }
            }
            let mut y = self.held;
            if self.smooth && self.hold.is_some() {
                for (side, v) in y.iter_mut().enumerate() {
                    *v = self.after[side].iter_mut().fold(*v, |v, f| f.run(v));
                }
            }
            let dw = 1.0 - self.wet;
            *l = dry[0] * dw + y[0] * self.wet;
            *r = dry[1] * dw + y[1] * self.wet;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;
    use std::f64::consts::TAU;

    const RATE: u32 = 48_000;

    fn play(e: &Effect, l: &mut [f32], r: &mut [f32]) {
        let mut p = e.processor(RATE);
        for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(l, r);
        }
    }

    fn sine(hz: f64, level: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (level * (TAU * hz * i as f64 / RATE as f64).sin()) as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn db(x: f64) -> f64 {
        20.0 * x.max(1e-10).log10()
    }

    /// Sets each setting by key.
    fn with(kind: Kind, values: &[(&str, f64)]) -> Effect {
        let e = Effect::new(kind);
        for (k, v) in values {
            let i = kind.params().iter().position(|p| p.key == *k).unwrap();
            e.set(i, *v);
        }
        e
    }

    /// With everything off, the record passes the sound unchanged.
    #[test]
    fn a_new_record_is_the_sound_unchanged() {
        let e = with(
            Kind::Vinyl,
            &[
                ("hiss", -80.0),
                ("crackle", 0.0),
                ("rumble", -80.0),
                ("wear", 0.0),
            ],
        );
        let src = sine(440.0, 0.5, 4800);
        let (mut l, mut r) = (src.clone(), src.clone());
        play(&e, &mut l, &mut r);
        assert_eq!(l, src);
        assert_eq!(r, src);
    }

    /// On silence the record hisses at its hiss level, and crackles more
    /// as the crackle goes up, a little differently on each side.
    #[test]
    fn a_record_hisses_and_crackles() {
        let e = with(
            Kind::Vinyl,
            &[
                ("hiss", -40.0),
                ("crackle", 0.0),
                ("rumble", -80.0),
                ("wear", 0.0),
            ],
        );
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        play(&e, &mut l, &mut r);
        let level = db(rms(&l));
        assert!((level + 40.0).abs() < 1.0, "{level}");
        assert_ne!(l, r);

        let clicks = |crackle: f64| {
            let e = with(
                Kind::Vinyl,
                &[
                    ("hiss", -80.0),
                    ("crackle", crackle),
                    ("crackle-level", 0.0),
                    ("rumble", -80.0),
                ],
            );
            let (mut l, mut r) = (vec![0.0; 96_000], vec![0.0; 96_000]);
            play(&e, &mut l, &mut r);
            assert!(crackle == 0.0 || l != r);
            // Bursts heard: frames that rise over -50 dB after a quiet
            // stretch.
            let mut n = 0;
            let mut quiet = 100;
            for x in &l {
                if x.abs() > 0.003 {
                    if quiet >= 100 {
                        n += 1;
                    }
                    quiet = 0;
                } else {
                    quiet += 1;
                }
            }
            n
        };
        let (few, many) = (clicks(20.0), clicks(80.0));
        assert!(few > 2 && many > 4 * few, "{few} {many}");
        assert_eq!(clicks(0.0), 0);
    }

    /// Rumble is low: a low pass at 100 Hz keeps nearly all of it.
    #[test]
    fn rumble_is_low_and_at_its_level() {
        let e = with(
            Kind::Vinyl,
            &[
                ("hiss", -80.0),
                ("crackle", 0.0),
                ("rumble", -30.0),
                ("wear", 0.0),
            ],
        );
        let (mut l, mut r) = (vec![0.0; 96_000], vec![0.0; 96_000]);
        play(&e, &mut l, &mut r);
        let level = db(rms(&l[24_000..]));
        assert!((level + 30.0).abs() < 3.0, "{level}");
        let mut f = crate::biquad::Biquad::new(crate::biquad::Coefs::low_pass(
            100.0,
            std::f64::consts::FRAC_1_SQRT_2,
            RATE as f64,
        ));
        let low: Vec<f32> = l.iter().map(|x| f.run(*x)).collect();
        assert!(db(rms(&low[24_000..])) > level - 1.0);
    }

    /// Wear dulls the highs and thins the lows.
    #[test]
    fn a_worn_record_loses_its_highs_and_lows() {
        for hz in [40.0, 12_000.0] {
            let e = with(
                Kind::Vinyl,
                &[
                    ("hiss", -80.0),
                    ("crackle", 0.0),
                    ("rumble", -80.0),
                    ("wear", 100.0),
                ],
            );
            let src = sine(hz, 0.5, 48_000);
            let (mut l, mut r) = (src.clone(), src.clone());
            play(&e, &mut l, &mut r);
            let lost = db(rms(&l[24_000..]) / rms(&src[24_000..]));
            assert!(lost < -6.0, "{hz} Hz: {lost}");
        }
    }

    /// The frames between upward zero crossings of a tone, after the
    /// first half second.
    fn periods(x: &[f32]) -> Vec<f64> {
        let mut out = Vec::new();
        let mut last: Option<f64> = None;
        for i in 24_001..x.len() {
            let (a, b) = (x[i - 1] as f64, x[i] as f64);
            if a < 0.0 && b >= 0.0 {
                let t = (i - 1) as f64 + a / (a - b);
                if let Some(p) = last {
                    out.push(t - p);
                }
                last = Some(t);
            }
        }
        out
    }

    /// Without wow, flutter, saturation, dull highs or hiss, the tape is
    /// a steady delay of the latency it reports; with wow the pitch
    /// wanders by about the amount it says.
    #[test]
    fn tape_wobbles_the_pitch() {
        let e = with(
            Kind::Tape,
            &[
                ("wow", 0.0),
                ("flutter", 0.0),
                ("saturation", 0.0),
                ("highs", 20_000.0),
            ],
        );
        let p = e.processor(RATE);
        let lag = p.latency() as usize;
        assert_eq!(lag, 221);
        let src = sine(1000.0, 0.5, 9600);
        let (mut l, mut r) = (src.clone(), src.clone());
        play(&e, &mut l, &mut r);
        for i in lag..9600 {
            assert!(
                (l[i] - src[i - lag]).abs() < 1e-6,
                "{i}: {} {}",
                l[i],
                src[i - lag]
            );
        }

        let swing = |wow: f64, flutter: f64| {
            let e = with(
                Kind::Tape,
                &[
                    ("wow", wow),
                    ("wow-rate", 0.6),
                    ("flutter", flutter),
                    ("saturation", 0.0),
                    ("highs", 20_000.0),
                ],
            );
            let src = sine(1000.0, 0.5, 5 * RATE as usize);
            let (mut l, mut r) = (src.clone(), src);
            play(&e, &mut l, &mut r);
            assert_eq!(l, r, "both sides move together");
            let p = periods(&l);
            let (lo, hi) = p
                .iter()
                .fold((f64::MAX, 0f64), |(a, b), x| (a.min(*x), b.max(*x)));
            // How far the pitch bends either way, as a share.
            (hi - lo) / 2.0 / 48.0
        };
        let still = swing(0.0, 0.0);
        let wow = swing(100.0, 0.0);
        let flutter = swing(0.0, 100.0);
        assert!(still < 0.001, "{still}");
        assert!(wow > 0.008 && wow < 0.025, "{wow}");
        assert!(flutter > 0.002 && flutter < 0.008, "{flutter}");
    }

    /// Dull highs take the top off, and saturation adds harmonics.
    #[test]
    fn tape_dulls_and_warms() {
        let e = with(
            Kind::Tape,
            &[
                ("wow", 0.0),
                ("flutter", 0.0),
                ("saturation", 0.0),
                ("highs", 3000.0),
            ],
        );
        let src = sine(10_000.0, 0.25, 24_000);
        let (mut l, mut r) = (src.clone(), src.clone());
        play(&e, &mut l, &mut r);
        assert!(db(rms(&l[4800..]) / rms(&src[4800..])) < -20.0);

        let hot = with(
            Kind::Tape,
            &[
                ("wow", 0.0),
                ("flutter", 0.0),
                ("saturation", 24.0),
                ("highs", 20_000.0),
            ],
        );
        let src = sine(500.0, 0.5, 24_000);
        let (mut l, mut r) = (src.clone(), src.clone());
        play(&hot, &mut l, &mut r);
        // The third harmonic, against the tone.
        let x = &l[4800..];
        let tone = |hz: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, v) in x.iter().enumerate() {
                let w = TAU * hz * n as f64 / RATE as f64;
                re += *v as f64 * w.cos();
                im += *v as f64 * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(db(tone(1000.0).max(tone(1500.0)) / tone(500.0)) > -40.0);
    }

    /// One bit leaves three levels; a low rate holds each value for as
    /// long as one of its samples lasts; off, it changes nothing.
    #[test]
    fn the_crusher_rounds_and_holds() {
        let e = with(Kind::BitCrusher, &[("bits", 1.0), ("rate", 48_000.0)]);
        let src = sine(110.0, 0.9, 4800);
        let (mut l, mut r) = (src.clone(), src.clone());
        play(&e, &mut l, &mut r);
        let mut levels: Vec<i32> = l.iter().map(|x| (x * 1000.0).round() as i32).collect();
        levels.sort();
        levels.dedup();
        assert_eq!(levels, [-1000, 0, 1000]);

        let e = with(Kind::BitCrusher, &[("bits", 24.0), ("rate", 1000.0)]);
        let src = sine(110.0, 0.9, 4800);
        let (mut l, mut r) = (src.clone(), src.clone());
        play(&e, &mut l, &mut r);
        let mut runs = Vec::new();
        let mut run = 1;
        for i in 1..l.len() {
            if l[i] == l[i - 1] {
                run += 1;
            } else {
                runs.push(run);
                run = 1;
            }
        }
        let runs = &runs[1..];
        assert!(runs.iter().all(|&n| (47..=49).contains(&n)), "{runs:?}");
        let mean = runs.iter().sum::<usize>() as f64 / runs.len() as f64;
        assert!((mean - 48.0).abs() < 0.1, "{mean}");

        let e = with(Kind::BitCrusher, &[("bits", 24.0), ("rate", 48_000.0)]);
        let (mut l, mut r) = (src.clone(), src.clone());
        play(&e, &mut l, &mut r);
        assert_eq!(l, src);
    }

    /// Smoothing the steps takes away most of what the hold folds above
    /// the lower rate's top.
    #[test]
    fn smoothing_takes_the_aliasing_away() {
        let fold = |smooth: f64| {
            let e = with(
                Kind::BitCrusher,
                &[("bits", 24.0), ("rate", 8000.0), ("smooth", smooth)],
            );
            let src = sine(3000.0, 0.5, 24_000);
            let (mut l, mut r) = (src.clone(), src);
            play(&e, &mut l, &mut r);
            // An image of the tone at 8 kHz - 3 kHz = 5 kHz.
            let x = &l[4800..];
            let (mut re, mut im) = (0.0, 0.0);
            for (n, v) in x.iter().enumerate() {
                let w = TAU * 5000.0 * n as f64 / RATE as f64;
                re += *v as f64 * w.cos();
                im += *v as f64 * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(db(fold(1.0) / fold(0.0)) < -20.0);
    }
}
