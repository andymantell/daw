// SPDX-License-Identifier: GPL-3.0-or-later
//! Effect containers (roadmap 3.5): how each kind splits a track's audio
//! into chains of effects and joins them again. The engine runs the
//! chains' effects in between and lines them up in time.
//!
//! - Parallel chains: each chain gets a copy; they are added up with the
//!   untouched (dry) audio at their own levels.
//! - Frequency bands: two to four bands, split by fourth-order
//!   Linkwitz-Riley crossovers (two second-order Butterworth filters in
//!   a row, a published design), which add back up to the same level at
//!   every frequency. Each band below a crossover goes through an
//!   all-pass at that frequency so the bands stay in phase where they
//!   meet.
//! - Mid and side: the sum (L+R)/2 and difference (L−R)/2, each run as
//!   mono, then decoded back to left and right.
//! - Left and right: each side run on its own, as mono.

use daw_engine::graph::Lanes;

use crate::biquad::{Biquad, Coefs};
use crate::{Dsp, Kind, Param, Unit};

/// A level that goes down to off.
const fn level(key: &'static str, name: &'static str) -> Param {
    Param::new(key, name, -60.0, 12.0, 0.0, Unit::Db).off_at(-60.0)
}

pub(crate) const PARALLEL: &[Param] = &[
    Param::new("chains", "Chains", 0.0, 3.0, 1.0, Unit::Choice)
        .choices(&["1 chain", "2 chains", "3 chains", "4 chains"]),
    Param::new("dry", "Dry level", -60.0, 12.0, -60.0, Unit::Db).off_at(-60.0),
    level("level1", "Chain 1 level"),
    level("level2", "Chain 2 level"),
    level("level3", "Chain 3 level"),
    level("level4", "Chain 4 level"),
];

pub(crate) const BANDS: &[Param] = &[
    Param::new("bands", "Bands", 0.0, 2.0, 1.0, Unit::Choice)
        .choices(&["2 bands", "3 bands", "4 bands"]),
    Param::new("cross1", "Crossover 1", 20.0, 20_000.0, 200.0, Unit::Hz).log(),
    Param::new("cross2", "Crossover 2", 20.0, 20_000.0, 2_000.0, Unit::Hz).log(),
    Param::new("cross3", "Crossover 3", 20.0, 20_000.0, 6_000.0, Unit::Hz).log(),
    level("level1", "Band 1 level"),
    level("level2", "Band 2 level"),
    level("level3", "Band 3 level"),
    level("level4", "Band 4 level"),
];

pub(crate) const MID_SIDE: &[Param] =
    &[level("level1", "Mid level"), level("level2", "Side level")];

pub(crate) const LEFT_RIGHT: &[Param] = &[
    level("level1", "Left level"),
    level("level2", "Right level"),
];

/// How many chains a container of `kind` has with these settings (in
/// the order of its params), or 0 for an effect that isn't one.
pub(crate) fn chains(kind: Kind, values: &[f64]) -> usize {
    let at = |i: usize| values.get(i).copied().unwrap_or(0.0).round().max(0.0) as usize;
    match kind {
        Kind::Parallel => (at(0) + 1).min(4),
        Kind::Bands => (at(0) + 2).min(4),
        Kind::MidSide | Kind::LeftRight => 2,
        _ => 0,
    }
}

/// Which of a container's settings mean something with `count` chains.
pub(crate) fn shown(kind: Kind, key: &str, count: usize) -> bool {
    let n = |prefix: &str| {
        key.strip_prefix(prefix)
            .and_then(|d| d.parse::<usize>().ok())
    };
    match kind {
        Kind::Parallel | Kind::Bands => {
            if let Some(i) = n("level") {
                i <= count
            } else if let Some(i) = n("cross") {
                i < count
            } else {
                true
            }
        }
        _ => true,
    }
}

/// What a container's chain is called.
pub(crate) fn chain_name(kind: Kind, branch: usize, count: usize) -> String {
    let named = |names: &[&str]| names.get(branch).map(|s| s.to_string());
    match kind {
        Kind::Bands => match count {
            2 => named(&["Low band", "High band"]),
            3 => named(&["Low band", "Middle band", "High band"]),
            _ => named(&[
                "Low band",
                "Low middle band",
                "High middle band",
                "High band",
            ]),
        },
        Kind::MidSide => named(&["Mid", "Side"]),
        Kind::LeftRight => named(&["Left", "Right"]),
        _ => None,
    }
    .unwrap_or_else(|| format!("Chain {}", branch + 1))
}

/// The audio side of a container.
pub(crate) struct Container {
    kind: Kind,
    rate: f64,
    /// Each chain's level as a gain, then the dry level, as set, and as
    /// reached sample by sample.
    target: [f32; 5],
    now: [f32; 5],
    /// How far `now` moves towards `target` each sample.
    glide: f32,
    /// The crossovers in use, low to high, in Hz.
    cross: [f64; 3],
    bands: [Bands; 2],
    /// No settings taken yet: the first are reached at once.
    fresh: bool,
}

/// One channel's band filters.
#[derive(Clone, Copy, Default)]
struct Bands {
    /// At each crossover: the two low-pass and two high-pass halves.
    low: [[Biquad; 2]; 3],
    high: [[Biquad; 2]; 3],
    /// For band `b`, an all-pass at each crossover above its own.
    phase: [[Biquad; 3]; 3],
}

const DRY: usize = 4;

impl Container {
    pub(crate) fn new(kind: Kind, rate: f64) -> Container {
        Container {
            kind,
            rate,
            target: [1.0; 5],
            now: [1.0; 5],
            glide: 1.0 - (-1.0 / (0.01 * rate)).exp() as f32,
            cross: [0.0; 3],
            bands: [Bands::default(); 2],
            fresh: true,
        }
    }

    /// Makes the band filters for new crossovers, keeping their state.
    fn set_cross(&mut self, mut cross: [f64; 3]) {
        cross.sort_by(f64::total_cmp);
        if cross == self.cross {
            return;
        }
        self.cross = cross;
        let q = std::f64::consts::FRAC_1_SQRT_2;
        for b in &mut self.bands {
            for (k, &hz) in cross.iter().enumerate() {
                let lp = Coefs::low_pass(hz, q, self.rate);
                let hp = Coefs::high_pass(hz, q, self.rate);
                let ap = Coefs::all_pass(hz, q, self.rate);
                for f in &mut b.low[k] {
                    f.c = lp;
                }
                for f in &mut b.high[k] {
                    f.c = hp;
                }
                for band in &mut b.phase {
                    band[k].c = ap;
                }
            }
        }
    }

    /// The level of the chain or dry audio at `i`, a sample on.
    #[inline]
    fn step(&mut self, i: usize) -> f32 {
        self.now[i] += (self.target[i] - self.now[i]) * self.glide;
        self.now[i]
    }

    pub(crate) fn split(&mut self, left: &[f32], right: &[f32], lanes: &mut Lanes, at: usize) {
        let count = lanes.count();
        let n = left.len();
        match self.kind {
            Kind::Bands => {
                // The first `count - 1` crossovers, low to high.
                let used = count.saturating_sub(1).min(3);
                for (ch, input) in [left, right].into_iter().enumerate() {
                    let f = &mut self.bands[ch];
                    for (i, &x) in input.iter().enumerate() {
                        let mut out = [0f32; 4];
                        let mut rest = x;
                        for (k, band) in out.iter_mut().enumerate().take(used) {
                            let lo = f.low[k][0].run(rest);
                            let mut lo = f.low[k][1].run(lo);
                            let hi = f.high[k][0].run(rest);
                            rest = f.high[k][1].run(hi);
                            // In phase with the bands above it.
                            for j in k + 1..used {
                                lo = f.phase[k][j].run(lo);
                            }
                            *band = lo;
                        }
                        out[used] = rest;
                        for (b, &y) in out.iter().enumerate().take(count) {
                            let (l, r) = lanes.get_mut(b);
                            let lane = if ch == 0 { l } else { r };
                            lane[at + i] = y;
                        }
                    }
                }
            }
            Kind::MidSide => {
                for i in 0..n {
                    let (m, s) = ((left[i] + right[i]) * 0.5, (left[i] - right[i]) * 0.5);
                    for (b, v) in [m, s].into_iter().enumerate().take(count) {
                        let (l, r) = lanes.get_mut(b);
                        l[at + i] = v;
                        r[at + i] = v;
                    }
                }
            }
            Kind::LeftRight => {
                for i in 0..n {
                    for (b, v) in [left[i], right[i]].into_iter().enumerate().take(count) {
                        let (l, r) = lanes.get_mut(b);
                        l[at + i] = v;
                        r[at + i] = v;
                    }
                }
            }
            _ => {
                for b in 0..count {
                    let (l, r) = lanes.get_mut(b);
                    l[at..at + n].copy_from_slice(left);
                    r[at..at + n].copy_from_slice(right);
                }
            }
        }
    }

    pub(crate) fn join(
        &mut self,
        dry: (&[f32], &[f32]),
        lanes: &Lanes,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let count = lanes.count().min(4);
        let n = left.len();
        // A mono chain's sound: both sides of its lane, averaged.
        let mono = |b: usize, i: usize| {
            let (l, r) = lanes.get(b);
            (l[i] + r[i]) * 0.5
        };
        match self.kind {
            Kind::MidSide if count == 2 => {
                for i in 0..n {
                    let m = mono(0, i) * self.step(0);
                    let s = mono(1, i) * self.step(1);
                    left[i] = m + s;
                    right[i] = m - s;
                }
            }
            Kind::LeftRight if count == 2 => {
                for i in 0..n {
                    left[i] = mono(0, i) * self.step(0);
                    right[i] = mono(1, i) * self.step(1);
                }
            }
            _ => {
                let with_dry = self.kind == Kind::Parallel;
                for i in 0..n {
                    let (mut l, mut r) = (0.0, 0.0);
                    if with_dry {
                        let g = self.step(DRY);
                        l = dry.0[i] * g;
                        r = dry.1[i] * g;
                    }
                    for b in 0..count {
                        let g = self.step(b);
                        let (bl, br) = lanes.get(b);
                        l += bl[i] * g;
                        r += br[i] * g;
                    }
                    left[i] = l;
                    right[i] = r;
                }
            }
        }
    }
}

fn gain(db: f64, off: bool) -> f32 {
    if off { 0.0 } else { crate::gain(db) as f32 }
}

impl Dsp for Container {
    fn update(&mut self, v: &[f64]) {
        let level = |db: f64| gain(db, db <= -60.0);
        match self.kind {
            Kind::Parallel => {
                self.target[DRY] = level(v[1]);
                for b in 0..4 {
                    self.target[b] = level(v[2 + b]);
                }
            }
            Kind::Bands => {
                let used = chains(Kind::Bands, v) - 1;
                let mut cross = [v[1], v[2], v[3]];
                // Crossovers not in use sit above the rest, out of the way.
                for c in cross.iter_mut().skip(used) {
                    *c = 20_000.0;
                }
                self.set_cross(cross);
                for b in 0..4 {
                    self.target[b] = level(v[4 + b]);
                }
            }
            _ => {
                self.target[0] = level(v[0]);
                self.target[1] = level(v[1]);
            }
        }
        if std::mem::take(&mut self.fresh) {
            self.now = self.target;
        }
    }

    /// Unused: the engine splits and joins it instead.
    fn process(&mut self, _: &mut [f32], _: &mut [f32]) {}

    fn container(&mut self) -> Option<&mut Container> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Effect;
    use daw_engine::graph::{Insert, Lanes};
    use std::f64::consts::PI;

    const RATE: u32 = 48_000;
    const BLOCK: usize = 256;

    /// Runs `effect`'s split and join over `input` with nothing in the
    /// chains, as the engine would with empty ones.
    fn through(effect: &Effect, input: [&[f32]; 2]) -> [Vec<f32>; 2] {
        let mut p = effect.processor(RATE);
        let count = effect.chains();
        let mut lanes = Lanes::new(count, BLOCK);
        let n = input[0].len();
        let mut out = [vec![0.0; n], vec![0.0; n]];
        for at in (0..n).step_by(BLOCK) {
            let end = (at + BLOCK).min(n);
            assert_eq!(end - at, BLOCK, "whole blocks only");
            let s = p.as_split().expect("a container splits");
            let (l, r) = (&input[0][at..end], &input[1][at..end]);
            s.split(l, r, &mut lanes);
            let (ol, or) = out.split_at_mut(1);
            s.join((l, r), &lanes, &mut ol[0][at..end], &mut or[0][at..end]);
        }
        out
    }

    fn sine(hz: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * hz * i as f64 / RATE as f64).sin() as f32 * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn bands_add_back_up_to_the_same_level_at_every_frequency() {
        for bands in 0..3 {
            let e = Effect::new(Kind::Bands);
            e.set(0, bands as f64);
            assert_eq!(e.chains(), bands + 2);
            for hz in [
                40.0, 150.0, 200.0, 600.0, 2_000.0, 4_000.0, 6_000.0, 12_000.0,
            ] {
                let x = sine(hz, BLOCK * 160);
                let [l, r] = through(&e, [&x, &x]);
                let tail = BLOCK * 80;
                let ratio = rms(&l[tail..]) / rms(&x[tail..]);
                let db = 20.0 * ratio.log10();
                assert!(
                    db.abs() < 0.05,
                    "{} bands at {hz} Hz: {db:.3} dB",
                    bands + 2
                );
                assert_eq!(l, r);
            }
        }
    }

    #[test]
    fn a_band_turned_off_takes_its_frequencies_out() {
        let e = Effect::new(Kind::Bands);
        e.set(0, 0.0); // 2 bands, crossing at 200 Hz
        e.set(5, -60.0); // the high band off
        let lo = sine(50.0, BLOCK * 160);
        let hi = sine(5_000.0, BLOCK * 160);
        let tail = BLOCK * 80;
        let [l, _] = through(&e, [&lo, &lo]);
        assert!((rms(&l[tail..]) / rms(&lo[tail..]) - 1.0).abs() < 0.02);
        let [l, _] = through(&e, [&hi, &hi]);
        assert!(rms(&l[tail..]) / rms(&hi[tail..]) < 0.01);
    }

    #[test]
    fn mid_side_and_left_right_give_back_what_came_in() {
        let a = sine(440.0, BLOCK * 8);
        let b = sine(97.0, BLOCK * 8);
        for kind in [Kind::MidSide, Kind::LeftRight] {
            let e = Effect::new(kind);
            let [l, r] = through(&e, [&a, &b]);
            for i in 0..a.len() {
                assert!(
                    (l[i] - a[i]).abs() < 1e-6 && (r[i] - b[i]).abs() < 1e-6,
                    "{kind:?}"
                );
            }
        }
    }

    #[test]
    fn side_off_leaves_mono_and_right_off_leaves_left() {
        let a = sine(440.0, BLOCK * 64);
        let b = sine(97.0, BLOCK * 64);
        let tail = BLOCK * 32;
        let e = Effect::new(Kind::MidSide);
        e.set(1, -60.0);
        let [l, r] = through(&e, [&a, &b]);
        for i in tail..a.len() {
            let m = (a[i] + b[i]) * 0.5;
            assert!((l[i] - m).abs() < 1e-5 && (r[i] - m).abs() < 1e-5);
        }
        let e = Effect::new(Kind::LeftRight);
        e.set(1, -60.0);
        let [l, r] = through(&e, [&a, &b]);
        for i in tail..a.len() {
            assert!((l[i] - a[i]).abs() < 1e-5 && r[i].abs() < 1e-5);
        }
    }

    #[test]
    fn parallel_adds_the_chains_and_the_dry_audio_at_their_levels() {
        let e = Effect::new(Kind::Parallel);
        assert_eq!(e.chains(), 2);
        e.set(1, 0.0); // dry at full
        e.set(3, -6.0); // chain 2 at half
        let x = sine(300.0, BLOCK * 64);
        let [l, _] = through(&e, [&x, &x]);
        let want = 1.0 + 1.0 + 10f64.powf(-6.0 / 20.0);
        for i in BLOCK * 32..x.len() {
            assert!((l[i] as f64 - x[i] as f64 * want).abs() < 1e-4);
        }
    }

    #[test]
    fn settings_say_how_many_chains_and_which_apply() {
        let e = Effect::new(Kind::Bands);
        e.set(0, 0.0);
        assert_eq!(e.chains(), 2);
        assert!(e.shown("cross1") && !e.shown("cross2") && !e.shown("level3"));
        assert_eq!(Kind::Bands.chain_name(1, 2), "High band");
        assert_eq!(Kind::Parallel.chain_name(2, 4), "Chain 3");
        assert_eq!(Effect::new(Kind::Eq).chains(), 0);
        assert!(Kind::MidSide.is_container() && !Kind::Delay.is_container());
    }
}
