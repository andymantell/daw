// SPDX-License-Identifier: GPL-3.0-or-later
//! Compressor, noise gate and limiter.
//!
//! The compressor follows the feed-forward, log-domain design with a soft
//! knee and a smooth peak detector described in D. Giannoulis, M. Massberg
//! and J. D. Reiss, "Digital Dynamic Range Compressor Design — A Tutorial
//! and Analysis", JAES 60(6), 2012. The limiter looks ahead: the gain it
//! needs is held at its lowest over the look-ahead and then averaged over
//! it, so the gain is already down when a peak arrives and never lets one
//! over the ceiling. It watches the true peak, between samples as well
//! as at them (4× oversampled, as ITU-R BS.1770-4 Annex 2 measures it),
//! so the ceiling holds once the audio is turned back into a wave too.
//! Both channels share one gain, so the stereo image stays put.

use daw_engine::meter::TRUE_PEAK_FIR;

use crate::{Dsp, Param, Unit, coef, db, gain};

pub const COMPRESSOR: &[Param] = &[
    Param::new("threshold", "Threshold", -60.0, 0.0, -18.0, Unit::Db),
    Param::new("ratio", "Ratio", 1.0, 20.0, 4.0, Unit::Ratio).log(),
    Param::new("attack", "Attack", 0.1, 100.0, 10.0, Unit::Ms).log(),
    Param::new("release", "Release", 10.0, 1000.0, 120.0, Unit::Ms).log(),
    Param::new("knee", "Knee", 0.0, 18.0, 6.0, Unit::Db),
    Param::new("makeup", "Make-up gain", -6.0, 24.0, 0.0, Unit::Db),
    Param::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub struct Compressor {
    rate: f64,
    threshold: f64,
    ratio: f64,
    knee: f64,
    attack: f64,
    release: f64,
    makeup: f64,
    wet: f32,
    /// The smoothed gain reduction, in dB (0 or above).
    level: f64,
}

impl Compressor {
    pub fn new(rate: f64) -> Compressor {
        Compressor {
            rate,
            threshold: 0.0,
            ratio: 1.0,
            knee: 0.0,
            attack: 0.0,
            release: 0.0,
            makeup: 1.0,
            wet: 1.0,
            level: 0.0,
        }
    }

    /// How far a level of `x` dB is turned down, in dB (0 or above).
    pub(crate) fn reduce(&self, x: f64) -> f64 {
        let (t, w, r) = (self.threshold, self.knee, self.ratio);
        let over = x - t;
        let y = if 2.0 * over < -w {
            x
        } else if 2.0 * over <= w && w > 0.0 {
            x + (1.0 / r - 1.0) * (over + w / 2.0).powi(2) / (2.0 * w)
        } else {
            t + over / r
        };
        x - y
    }
}

impl Dsp for Compressor {
    fn update(&mut self, v: &[f64]) {
        self.threshold = v[0];
        self.ratio = v[1];
        self.attack = coef(v[2] / 1000.0, self.rate);
        self.release = coef(v[3] / 1000.0, self.rate);
        self.knee = v[4];
        self.makeup = gain(v[5]);
        self.wet = (v[6] / 100.0) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.step(l.abs().max(r.abs()));
            *l *= g;
            *r *= g;
        }
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let g = self.step(key.0[i].abs().max(key.1[i].abs()));
            *l *= g;
            *r *= g;
        }
    }

    fn takes_key(&self) -> bool {
        true
    }

    fn reduction_db(&self) -> f32 {
        -self.level as f32
    }
}

impl Compressor {
    /// The gain for one frame whose level (own or sidechain) is `peak`.
    #[inline]
    pub(crate) fn step(&mut self, peak: f32) -> f32 {
        let x = db(peak as f64);
        let want = self.reduce(x);
        let a = if want > self.level {
            self.attack
        } else {
            self.release
        };
        self.level = a * self.level + (1.0 - a) * want;
        let g = (gain(-self.level) * self.makeup) as f32;
        1.0 - self.wet + self.wet * g
    }
}

pub const GATE: &[Param] = &[
    Param::new("threshold", "Threshold", -80.0, 0.0, -50.0, Unit::Db),
    Param::new("depth", "Depth", 0.0, 80.0, 60.0, Unit::Db),
    Param::new("attack", "Attack", 0.1, 50.0, 1.0, Unit::Ms).log(),
    Param::new("hold", "Hold", 0.0, 500.0, 50.0, Unit::Ms),
    Param::new("release", "Release", 5.0, 2000.0, 150.0, Unit::Ms).log(),
];

/// How far below the threshold the level must fall before the gate
/// closes, so it doesn't chatter on a level near it.
const GATE_HYSTERESIS_DB: f64 = 6.0;

pub struct Gate {
    rate: f64,
    open_at: f64,
    close_at: f64,
    depth: f64,
    attack: f64,
    release: f64,
    hold: u32,
    /// The input's level, falling slowly between peaks.
    env: f64,
    env_fall: f64,
    open: bool,
    /// Frames left before closing.
    holding: u32,
    /// The gain now, in dB (0 or below).
    gain_db: f64,
}

impl Gate {
    pub fn new(rate: f64) -> Gate {
        Gate {
            rate,
            open_at: 0.0,
            close_at: 0.0,
            depth: 0.0,
            attack: 0.0,
            release: 0.0,
            hold: 0,
            env: 0.0,
            env_fall: coef(0.01, rate),
            open: false,
            holding: 0,
            gain_db: 0.0,
        }
    }
}

impl Dsp for Gate {
    fn update(&mut self, v: &[f64]) {
        self.open_at = gain(v[0]);
        self.close_at = gain(v[0] - GATE_HYSTERESIS_DB);
        self.depth = v[1];
        self.attack = coef(v[2] / 1000.0, self.rate);
        self.hold = (v[3] / 1000.0 * self.rate) as u32;
        self.release = coef(v[4] / 1000.0, self.rate);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.step(l.abs().max(r.abs()));
            *l *= g;
            *r *= g;
        }
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let g = self.step(key.0[i].abs().max(key.1[i].abs()));
            *l *= g;
            *r *= g;
        }
    }

    fn takes_key(&self) -> bool {
        true
    }

    fn reduction_db(&self) -> f32 {
        self.gain_db as f32
    }
}

impl Gate {
    /// The gain for one frame whose level (own or sidechain) is `peak`.
    #[inline]
    fn step(&mut self, peak: f32) -> f32 {
        let a = peak as f64;
        self.env = if a > self.env {
            a
        } else {
            self.env * self.env_fall
        };
        if self.env >= self.open_at {
            self.open = true;
            self.holding = self.hold;
        } else if self.open && self.env < self.close_at {
            if self.holding == 0 {
                self.open = false;
            } else {
                self.holding -= 1;
            }
        }
        let (target, c) = if self.open {
            (0.0, self.attack)
        } else {
            (-self.depth, self.release)
        };
        self.gain_db = target + (self.gain_db - target) * c;
        gain(self.gain_db) as f32
    }
}

pub const LIMITER: &[Param] = &[
    Param::new("ceiling", "Ceiling", -12.0, 0.0, -0.3, Unit::Db),
    Param::new("drive", "Drive", 0.0, 24.0, 0.0, Unit::Db),
    Param::new("release", "Release", 1.0, 1000.0, 60.0, Unit::Ms).log(),
];

/// How far ahead the limiter looks, in seconds.
const LOOKAHEAD_SECONDS: f64 = 0.0015;
/// How many frames behind the newest the true-peak detector looks at:
/// its interpolating filter finds the peaks between the frames this far
/// and one less back.
const DETECT: usize = 6;
/// How far under the ceiling the gain aims, in dB. The gain changes
/// across the dozen frames the detector's filter spans, which can lift
/// the peak between them by a hundredth of a dB or so.
const MARGIN_DB: f64 = 0.05;

pub struct Limiter {
    rate: f64,
    ceiling: f32,
    /// [`MARGIN_DB`] as a gain.
    margin: f32,
    drive: f32,
    release: f64,
    /// The look-ahead in frames.
    len: usize,
    /// The input, delayed by `len - 1 + DETECT` frames.
    delay: [Vec<f32>; 2],
    dat: usize,
    /// The newest 12 frames, for the true-peak detector.
    history: [[f32; 12]; 2],
    hat: usize,
    /// The gain each recent frame needs, one more than the look-ahead,
    /// so a peak's gain covers every frame that averages it in.
    needed: Vec<f64>,
    nat: usize,
    /// That held at its lowest and recovering at the release rate.
    held: Vec<f64>,
    /// The sum of `held`, for its average.
    sum: f64,
    at: usize,
    /// `held` as last worked out, recovering.
    last: f64,
    /// The gain applied now.
    now: f64,
}

impl Limiter {
    pub fn new(rate: f64) -> Limiter {
        let len = (LOOKAHEAD_SECONDS * rate).round() as usize + 1;
        let delay = len + DETECT;
        Limiter {
            rate,
            ceiling: 1.0,
            margin: gain(-MARGIN_DB) as f32,
            drive: 1.0,
            release: 0.0,
            len,
            delay: [vec![0.0; delay], vec![0.0; delay]],
            dat: 0,
            history: [[0.0; 12]; 2],
            hat: 0,
            needed: vec![1.0; len + 1],
            nat: 0,
            held: vec![1.0; len],
            sum: len as f64,
            at: 0,
            last: 1.0,
            now: 1.0,
        }
    }

    /// Takes the newest frame and gives the true peak (BS.1770 Annex 2,
    /// 4× oversampled) between the frames `DETECT` and `DETECT - 1`
    /// back, and at them.
    #[inline]
    fn detect(&mut self, xl: f32, xr: f32) -> f32 {
        self.hat = (self.hat + 11) % 12;
        let mut m = 0f32;
        for (c, x) in [xl, xr].into_iter().enumerate() {
            let h = &mut self.history[c];
            h[self.hat] = x;
            let at = |k: usize| h[(self.hat + k) % 12];
            m = m.max(at(DETECT).abs()).max(at(DETECT - 1).abs());
            for phase in &TRUE_PEAK_FIR {
                let mut y = 0.0;
                for (k, coef) in phase.iter().enumerate() {
                    y += coef * at(k);
                }
                m = m.max(y.abs());
            }
        }
        m
    }
}

impl Dsp for Limiter {
    fn update(&mut self, v: &[f64]) {
        self.ceiling = gain(v[0]) as f32;
        self.drive = gain(v[1]) as f32;
        self.release = coef(v[2] / 1000.0, self.rate);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = self.len;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (xl, xr) = (*l * self.drive, *r * self.drive);
            // The peak a frame `DETECT` back is aimed at: its audio is
            // delayed that much more than the gain.
            let peak = self.detect(xl, xr);
            let aim = self.ceiling * self.margin;
            let need = if peak > aim { (aim / peak) as f64 } else { 1.0 };
            // The oldest frame leaves the delay as this one comes in.
            let d = self.delay[0].len();
            let next = (self.dat + 1) % d;
            let (ol, or) = (self.delay[0][next], self.delay[1][next]);
            self.delay[0][self.dat] = xl;
            self.delay[1][self.dat] = xr;
            self.dat = next;
            self.needed[self.nat] = need;
            self.nat = (self.nat + 1) % self.needed.len();
            let lowest = self.needed.iter().copied().fold(1.0, f64::min);
            let recovering = 1.0 - (1.0 - self.last) * self.release;
            let held = lowest.min(recovering);
            self.last = held;
            let i = self.at;
            self.sum += held - self.held[i];
            self.held[i] = held;
            self.at = (i + 1) % n;
            if self.at == 0 {
                // Rounding errors don't build up.
                self.sum = self.held.iter().sum();
            }
            self.now = self.sum / n as f64;
            let g = self.now as f32;
            *l = (ol * g).clamp(-self.ceiling, self.ceiling);
            *r = (or * g).clamp(-self.ceiling, self.ceiling);
        }
    }

    fn latency(&self) -> u32 {
        (self.len - 1 + DETECT) as u32
    }

    fn reduction_db(&self) -> f32 {
        db(self.now) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;

    #[test]
    fn compressor_curve_matches_its_ratio() {
        let mut c = Compressor::new(48_000.0);
        c.update(&[-20.0, 4.0, 10.0, 100.0, 0.0, 0.0, 100.0]);
        assert_eq!(c.reduce(-30.0), 0.0);
        // 20 dB over at 4:1 comes out 5 dB over.
        assert!((c.reduce(0.0) - 15.0).abs() < 1e-9);
        c.update(&[-20.0, 4.0, 10.0, 100.0, 10.0, 0.0, 100.0]);
        // In the knee it turns down a little, and the curve is continuous
        // at its edges.
        assert!(c.reduce(-20.0) > 0.0 && c.reduce(-20.0) < 5.0);
        assert!(c.reduce(-25.0).abs() < 1e-9);
        assert!((c.reduce(-15.0) - 3.75).abs() < 1e-9);
    }

    fn sine(n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0).sin() * amp)
            .collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn compressor_turns_a_loud_tone_down() {
        let e = Effect::new(Kind::Compressor);
        e.set(0, -20.0);
        e.set(1, 4.0);
        e.set(4, 0.0);
        let mut p = e.processor(48_000);
        let mut l = sine(48_000, 1.0);
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        // 0 dB peaks, 20 dB over at 4:1: about 15 dB down once settled.
        // The detector follows peaks with a release, so a sine is turned
        // down a little less.
        let out = 20.0 * peak(&l[40_000..]).log10();
        assert!(out < -10.0 && out > -16.0, "{out}");
        assert!(e.reduction_db() < -10.0);
    }

    #[test]
    fn a_sidechain_drives_the_compressor_and_gate_instead_of_their_own_audio() {
        // A quiet tone, with a loud one on the sidechain: the compressor
        // turns the quiet tone down as if it were the loud one.
        let e = Effect::new(Kind::Compressor);
        e.set(0, -20.0);
        e.set(1, 4.0);
        e.set(4, 0.0);
        let mut p = e.processor(48_000);
        let mut l = sine(48_000, 0.1);
        let mut r = l.clone();
        let loud = sine(48_000, 1.0);
        p.process_keyed(&mut l, &mut r, (&loud, &loud));
        let out = 20.0 * (peak(&l[40_000..]) / 0.1).log10();
        assert!(out < -10.0 && out > -16.0, "{out}");

        // A gate on silence opens when the sidechain plays.
        let e = Effect::new(Kind::Gate);
        e.set(0, -40.0);
        let mut p = e.processor(48_000);
        let mut l = vec![0.01; 24_000];
        let mut r = l.clone();
        let key = sine(24_000, 0.5);
        p.process_keyed(&mut l, &mut r, (&key, &key));
        assert!(peak(&l[12_000..]) > 0.0099, "open on the key");
        let mut l = vec![0.01; 24_000];
        let mut r = l.clone();
        let silence = vec![0.0; 24_000];
        p.process_keyed(&mut l, &mut r, (&silence, &silence));
        assert!(peak(&l[22_000..]) < 0.0001, "closed on a silent key");

        // An effect without a key ignores it.
        let e = Effect::new(Kind::Eq);
        let mut p = e.processor(48_000);
        let mut l = sine(1000, 0.5);
        let mut r = l.clone();
        p.process_keyed(&mut l, &mut r, (&silence[..1000], &silence[..1000]));
        assert!((peak(&l) - 0.5).abs() < 0.01);
    }

    #[test]
    fn gate_silences_quiet_noise_and_lets_playing_through() {
        let e = Effect::new(Kind::Gate);
        e.set(0, -40.0);
        let mut p = e.processor(48_000);
        // Hiss at -60 dB.
        let mut l: Vec<f32> = (0..48_000)
            .map(|i| if i % 2 == 0 { 0.001 } else { -0.001 })
            .collect();
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        assert!(peak(&l[24_000..]) < 0.001 * 0.01);
        // A note at -6 dB opens it.
        let mut l = sine(24_000, 0.5);
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        assert!((peak(&l[4_800..]) - 0.5).abs() < 0.01);
    }

    #[test]
    fn limiter_never_goes_over_its_ceiling_and_reports_its_delay() {
        let e = Effect::new(Kind::Limiter);
        e.set(0, -6.0);
        e.set(1, 12.0);
        let mut p = e.processor(48_000);
        assert_eq!(p.latency(), 78);
        let ceiling = 10f32.powf(-6.0 / 20.0);
        // Sudden loud clicks over a tone.
        let mut l = sine(48_000, 0.5);
        for i in (1000..48_000).step_by(4321) {
            l[i] = 1.0;
        }
        let mut r = l.clone();
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(cl, cr);
        }
        assert!(peak(&l) <= ceiling * 1.0001, "{}", peak(&l));
        // Below the ceiling it passes unchanged, a latency later.
        let e = Effect::new(Kind::Limiter);
        let mut p = e.processor(48_000);
        let src = sine(4_800, 0.25);
        let mut l = src.clone();
        let mut r = src.clone();
        p.process(&mut l, &mut r);
        for i in 78..4_800 {
            assert!((l[i] - src[i - 78]).abs() < 1e-6);
        }
    }

    /// The true peak of what comes out, as the meters measure it, stays
    /// under the ceiling: a tone at a quarter of the sample rate, whose
    /// samples miss its tops by 3 dB, then loud bursts of it and noise.
    #[test]
    fn limiter_holds_the_true_peak_under_its_ceiling() {
        use daw_engine::meter::TruePeak;
        for (ceiling_db, drive, release) in [
            (-1.5, 0.0, 60.0),
            (-1.5, 12.0, 5.0),
            (-0.3, 6.0, 1.0),
            (-6.0, 24.0, 1.0),
            (-1.0, 18.0, 300.0),
        ] {
            let e = Effect::new(Kind::Limiter);
            e.set(0, ceiling_db);
            e.set(1, drive);
            e.set(2, release);
            let mut p = e.processor(48_000);
            let mut seed = 7u32;
            let n = 96_000;
            let mut l: Vec<f32> = (0..n)
                .map(|i| {
                    let t = i as f32;
                    // Quarter rate, 45° off: every sample at 0.707.
                    let tone = (std::f32::consts::FRAC_PI_2 * t + 0.785).sin();
                    let level = if (i / 3000) % 2 == 0 { 0.9 } else { 0.3 };
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let noise = (seed >> 8) as f32 / (1 << 23) as f32 - 1.0;
                    let click = if i % 4_801 < 3 { 0.8 } else { 0.0 };
                    level * tone + 0.3 * noise + click
                })
                .collect();
            let mut r: Vec<f32> = l.iter().map(|x| -0.8 * x).collect();
            for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                p.process(cl, cr);
            }
            let mut tp = TruePeak::default();
            let frames: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
            tp.push(&frames);
            let (tl, tr) = tp.max();
            let got = 20.0 * tl.max(tr).log10();
            assert!(
                got <= ceiling_db as f32,
                "{ceiling_db} dB ceiling: {got} dBTP"
            );
            // And it isn't simply turned right down.
            assert!(
                got > ceiling_db as f32 - 1.0,
                "{ceiling_db} dB ceiling: {got} dBTP"
            );
        }
    }
}
