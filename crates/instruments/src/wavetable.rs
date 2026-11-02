// SPDX-License-Identifier: GPL-3.0-or-later
//! Wavetables made in code for the Blue Wave (`docs/plans/classic-synths.md`,
//! S9): tables of 64 single-cycle waves each, the way the 1981 German
//! wavetable synth laid them out, with the last four waves of every
//! table the plain triangle, pulse, square and saw so a sweep can end
//! on an analogue shape. Each table is a formula over its position:
//! harmonics added one at a time, a resonant peak sweeping up the
//! series, a pulse narrowing, vowels, bells, organ stops, glass, a
//! synced saw, staircases and a folded sine. Nothing here is read from
//! a product; the waves are written as spectra or as shapes and worked
//! out when a table is first used.
//!
//! Every wave is kept two ways. The **smooth** set is band-limited: a
//! cycle for each octave, holding only the harmonics that fit under
//! half the sample rate at that pitch, read with interpolation, so the
//! sweep is clean. The **eight-bit** set keeps all 128 harmonics in
//! one cycle, rounded to eight bits, and is read without interpolation:
//! the top end folds back, the rounding adds a hiss, and a wave sweep
//! steps from one wave to the next, which is the sound that made the
//! original what it was.

use std::f32::consts::TAU;
use std::sync::OnceLock;

/// Waves in a table.
pub const WAVES: usize = 64;
/// The last position a table's own formula fills; the four after it are
/// the plain shapes.
pub const LAST_OWN: usize = 59;
/// Samples in a cycle.
pub const CYCLE: usize = 256;
/// Harmonics in a full wave.
pub const HARMONICS: usize = 128;
/// Band-limited copies of each wave: 128 harmonics, then 64, 32 … 1.
pub const MIPS: usize = 8;

/// The tables, by index; the names are the setting's choices.
pub const NAMES: &[&str] = &[
    "Harmonics",
    "Resonant",
    "Pulse width",
    "Vowels",
    "Bells",
    "Organ",
    "Glass",
    "Sync",
    "Steps",
    "Folded",
];

/// A wave as its harmonics: cosine and sine parts for harmonics 1 to
/// [`HARMONICS`], at index n − 1.
struct Spectrum {
    re: [f32; HARMONICS],
    im: [f32; HARMONICS],
}

impl Spectrum {
    fn silent() -> Spectrum {
        Spectrum {
            re: [0.0; HARMONICS],
            im: [0.0; HARMONICS],
        }
    }

    /// Adds a sine harmonic `n` with amplitude `a` and phase `phase`
    /// (radians).
    fn add(&mut self, n: usize, a: f32, phase: f32) {
        if (1..=HARMONICS).contains(&n) {
            self.re[n - 1] += a * phase.sin();
            self.im[n - 1] += a * phase.cos();
        }
    }

    /// The spectrum of one cycle given as a shape, by a plain discrete
    /// Fourier transform of [`CYCLE`] points.
    fn of_shape(shape: impl Fn(f32) -> f32) -> Spectrum {
        let x: Vec<f32> = (0..CYCLE).map(|i| shape(i as f32 / CYCLE as f32)).collect();
        let mut s = Spectrum::silent();
        for n in 1..=HARMONICS {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, &v) in x.iter().enumerate() {
                let p = TAU * (n * i) as f32 / CYCLE as f32;
                re += v * p.cos();
                im += v * p.sin();
            }
            s.re[n - 1] = 2.0 * re / CYCLE as f32;
            s.im[n - 1] = 2.0 * im / CYCLE as f32;
        }
        s
    }

    /// The wave over one cycle with harmonics up to `limit`, into `out`.
    fn render(&self, limit: usize, out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let mut v = 0.0f32;
            for n in 1..=limit.min(HARMONICS) {
                let p = TAU * ((n * i) % CYCLE) as f32 / CYCLE as f32;
                v += self.re[n - 1] * p.cos() + self.im[n - 1] * p.sin();
            }
            *o = v;
        }
    }
}

/// The spectrum of a saw rising from −1 to 1 over a cycle.
fn saw() -> Spectrum {
    let mut s = Spectrum::silent();
    for n in 1..=HARMONICS {
        s.im[n - 1] = -2.0 / (std::f32::consts::PI * n as f32);
    }
    s
}

/// A pulse of width `w` (0 to 1), high for `w` of the cycle: the
/// difference of two saws.
fn pulse(w: f32) -> Spectrum {
    let mut s = Spectrum::silent();
    for n in 1..=HARMONICS {
        let a = -2.0 / (std::f32::consts::PI * n as f32);
        let t = TAU * n as f32 * w;
        s.im[n - 1] = a * (1.0 - t.cos());
        s.re[n - 1] = a * t.sin();
    }
    s
}

fn square() -> Spectrum {
    let mut s = Spectrum::silent();
    for n in (1..=HARMONICS).step_by(2) {
        s.im[n - 1] = 4.0 / (std::f32::consts::PI * n as f32);
    }
    s
}

fn triangle() -> Spectrum {
    let mut s = Spectrum::silent();
    for (k, n) in (1..=HARMONICS).step_by(2).enumerate() {
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        s.im[n - 1] = sign * 8.0 / (std::f32::consts::PI.powi(2) * (n * n) as f32);
    }
    s
}

/// The vowels' first two formants in Hz, as a sweep goes through them.
const VOWELS: [(f32, f32); 5] = [
    (730.0, 1090.0),
    (570.0, 840.0),
    (300.0, 870.0),
    (530.0, 1840.0),
    (270.0, 2290.0),
];

/// Interpolates `t` (0 to 1) over the rows of `keys`.
fn across<const N: usize, const K: usize>(keys: &[[f32; N]; K], t: f32) -> [f32; N] {
    let x = t.clamp(0.0, 1.0) * (K - 1) as f32;
    let i = (x.floor() as usize).min(K - 2);
    let f = x - i as f32;
    let mut out = [0.0; N];
    for (k, o) in out.iter_mut().enumerate() {
        *o = keys[i][k] * (1.0 - f) + keys[i + 1][k] * f;
    }
    out
}

/// A table's own wave at `t` (0 at position 0, 1 at [`LAST_OWN`]).
fn own_wave(table: usize, t: f32) -> Spectrum {
    match table {
        // Harmonics: a sine, then a saw's harmonics added one at a
        // time up to thirty-two.
        0 => {
            let count = 1 + (t * 31.0).round() as usize;
            let mut s = Spectrum::silent();
            for n in 1..=count {
                s.im[n - 1] = -2.0 / (std::f32::consts::PI * n as f32);
            }
            s
        }
        // Resonant: a saw with a sharp peak sweeping up its series,
        // the sound of a filter sweep frozen into waves.
        1 => {
            let centre = 1.0 + 40.0 * t * t;
            let width = 0.6 + 0.2 * centre;
            let mut s = saw();
            for n in 1..=HARMONICS {
                let d = (n as f32 - centre) / width;
                let peak = 1.0 + 8.0 * (-d * d).exp();
                s.im[n - 1] *= peak;
            }
            s
        }
        // Pulse width: a pulse from a sliver to a square.
        2 => pulse(0.04 + 0.46 * t),
        // Vowels: two formants moving a, o, u, e, i over a 110 Hz voice.
        3 => {
            let keys: [[f32; 2]; 5] = VOWELS.map(|(a, b)| [a, b]);
            let [f1, f2] = across(&keys, t);
            let mut s = Spectrum::silent();
            for n in 1..=HARMONICS {
                let f = 110.0 * n as f32;
                let g1 = (-((f - f1) / 110.0).powi(2)).exp();
                let g2 = 0.6 * (-((f - f2) / 160.0).powi(2)).exp();
                let g3 = 0.15 * (-((f - 2600.0) / 250.0).powi(2)).exp();
                let a = (g1 + g2 + g3 + 0.03) / (1.0 + 0.02 * n as f32);
                s.add(n, a, if n % 2 == 0 { 0.0 } else { 0.4 });
            }
            s
        }
        // Bells: a few sparse partials, the cluster moving up as the
        // sweep goes, over a thin fundamental.
        4 => {
            let partials = [
                1, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 79, 83,
            ];
            let centre = (1.0 + 5.5 * t).exp2();
            let mut s = Spectrum::silent();
            for (k, &n) in partials.iter().enumerate() {
                let d = (n as f32 / centre).ln() / 0.9;
                let a = (-d * d).exp() / (n as f32).sqrt() + if n == 1 { 0.2 } else { 0.0 };
                s.add(n, a, k as f32 * 1.9);
            }
            s
        }
        // Organ: drawbar stops pulled out one by one, 8' to 1'.
        5 => {
            let stops = [1usize, 2, 3, 4, 5, 6, 8];
            let keys: [[f32; 7]; 5] = [
                [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [1.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0],
                [1.0, 0.8, 0.6, 0.0, 0.0, 0.0, 0.0],
                [1.0, 0.8, 0.6, 0.5, 0.0, 0.0, 0.4],
                [1.0, 0.9, 0.7, 0.6, 0.5, 0.4, 0.5],
            ];
            let levels = across(&keys, t);
            let mut s = Spectrum::silent();
            for (n, a) in stops.iter().zip(levels) {
                s.add(*n, a, 0.0);
            }
            s
        }
        // Glass: a handful of partials three apart, the weight moving
        // from the fundamental to the high ones, in scattered phases.
        6 => {
            let mut s = Spectrum::silent();
            for k in 0..10 {
                let n = 1 + 3 * k;
                let d = (k as f32 - 9.0 * t) / 2.2;
                let a = (-d * d).exp() + if k == 0 { 0.15 } else { 0.0 };
                s.add(n, a, k as f32 * 2.4);
            }
            s
        }
        // Sync: the spectrum a saw takes when hard-synced to a pitch
        // rising from the note to three octaves above it.
        7 => {
            let ratio = 1.0 + 7.0 * t;
            let mut s = Spectrum::silent();
            for n in 1..=HARMONICS {
                let mut a = 0.0;
                for m in 1..=6 {
                    let d = (n as f32 - m as f32 * ratio) / 0.8;
                    a += 1.0 / (1.0 + d * d) / m as f32;
                }
                s.im[n - 1] = -a / (n as f32).sqrt();
            }
            s
        }
        // Steps: a saw drawn as a staircase of two to sixteen steps.
        8 => {
            let steps = 2.0 + (14.0 * t).round();
            Spectrum::of_shape(move |phase| {
                let step = (phase * steps).floor();
                (step / (steps - 1.0)) * 2.0 - 1.0
            })
        }
        // Folded: a sine pushed harder and harder into a wavefolder.
        _ => {
            let gain = 1.0 + 9.0 * t;
            Spectrum::of_shape(move |phase| {
                let v = gain * (TAU * phase).sin();
                // Folds back at ±1 as often as it needs to.
                let w = (v + 1.0).rem_euclid(4.0);
                if w < 2.0 { w - 1.0 } else { 3.0 - w }
            })
        }
    }
}

/// The spectrum of wave `w` in table `table`.
fn spectrum(table: usize, w: usize) -> Spectrum {
    match w {
        60 => triangle(),
        61 => pulse(0.25),
        62 => square(),
        63 => saw(),
        _ => own_wave(table, w as f32 / LAST_OWN as f32),
    }
}

/// One table, both ways.
pub struct Table {
    /// `[wave][mip][CYCLE]`, every wave at peak 1.
    smooth: Vec<f32>,
    /// `[wave][CYCLE]`, every wave at peak 1 in eight bits.
    rough: Vec<f32>,
}

impl Table {
    fn build(table: usize) -> Table {
        let mut smooth = vec![0.0f32; WAVES * MIPS * CYCLE];
        let mut rough = vec![0.0f32; WAVES * CYCLE];
        for w in 0..WAVES {
            let s = spectrum(table, w);
            let full = &mut smooth[w * MIPS * CYCLE..][..CYCLE];
            s.render(HARMONICS, full);
            let peak = full.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
            let gain = 1.0 / peak;
            for (m, r) in rough[w * CYCLE..][..CYCLE].iter_mut().enumerate() {
                *r = (full[m] * gain * 127.0).round() / 127.0;
            }
            for mip in 0..MIPS {
                let out = &mut smooth[(w * MIPS + mip) * CYCLE..][..CYCLE];
                if mip > 0 {
                    s.render(HARMONICS >> mip, out);
                }
                for v in out.iter_mut() {
                    *v *= gain;
                }
            }
        }
        Table { smooth, rough }
    }

    /// Table `i`, built the first time it is asked for.
    pub fn get(i: usize) -> &'static Table {
        static TABLES: [OnceLock<Table>; 10] = [const { OnceLock::new() }; 10];
        let i = i.min(TABLES.len() - 1);
        TABLES[i].get_or_init(|| Table::build(i))
    }

    /// The band-limited copy to read at `dt` cycles a sample: the one
    /// whose harmonics all fit under half the sample rate.
    #[inline]
    pub fn mip_for(dt: f32) -> usize {
        let fits = 0.5 / dt.max(1e-6);
        let mut mip = 0;
        while mip < MIPS - 1 && (HARMONICS >> mip) as f32 > fits {
            mip += 1;
        }
        mip
    }

    /// The smooth set read at `position` (0 to 63, fractional: between
    /// two waves) and `phase` (0 to 1), with the copy `mip`.
    #[inline]
    pub fn smooth(&self, position: f32, mip: usize, phase: f32) -> f32 {
        let position = position.clamp(0.0, (WAVES - 1) as f32);
        let w = position as usize;
        let wf = position - w as f32;
        let x = phase.rem_euclid(1.0) * CYCLE as f32;
        let i = x as usize;
        let f = x - i as f32;
        let read = |w: usize| -> f32 {
            let c = &self.smooth[(w.min(WAVES - 1) * MIPS + mip) * CYCLE..][..CYCLE];
            let a = c[i.min(CYCLE - 1)];
            let b = c[(i + 1) % CYCLE];
            a + (b - a) * f
        };
        if wf < 1e-3 {
            read(w)
        } else {
            let a = read(w);
            let b = read(w + 1);
            a + (b - a) * wf
        }
    }

    /// The eight-bit set read at the nearest wave to `position` and the
    /// sample under `phase`, no interpolation.
    #[inline]
    pub fn rough(&self, position: f32, phase: f32) -> f32 {
        let w = (position.round().clamp(0.0, (WAVES - 1) as f32)) as usize;
        let i = (phase.rem_euclid(1.0) * CYCLE as f32) as usize;
        self.rough[w * CYCLE + i.min(CYCLE - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(x: &[f32], n: usize) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &v) in x.iter().enumerate() {
            let p = TAU * (n * i) as f32 / CYCLE as f32;
            re += v * p.cos();
            im += v * p.sin();
        }
        (re * re + im * im).sqrt() * 2.0 / CYCLE as f32
    }

    #[test]
    fn every_table_has_sixty_four_waves_at_peak_one_ending_in_the_plain_shapes() {
        for t in 0..NAMES.len() {
            let table = Table::get(t);
            for w in 0..WAVES {
                let cycle: Vec<f32> = (0..CYCLE)
                    .map(|i| table.smooth(w as f32, 0, i as f32 / CYCLE as f32))
                    .collect();
                let peak = cycle.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!((peak - 1.0).abs() < 0.02, "table {t} wave {w}: peak {peak}");
                let rough: Vec<f32> = (0..CYCLE)
                    .map(|i| table.rough(w as f32, i as f32 / CYCLE as f32))
                    .collect();
                // Eight bits: every value a multiple of 1/127.
                assert!(
                    rough
                        .iter()
                        .all(|v| (v * 127.0 - (v * 127.0).round()).abs() < 1e-4)
                );
            }
            // The square at 62 has no even harmonics; the saw at 63 does.
            let square: Vec<f32> = (0..CYCLE)
                .map(|i| table.smooth(62.0, 0, i as f32 / CYCLE as f32))
                .collect();
            let saw: Vec<f32> = (0..CYCLE)
                .map(|i| table.smooth(63.0, 0, i as f32 / CYCLE as f32))
                .collect();
            assert!(level(&square, 2) < 0.01, "table {t}");
            assert!(level(&saw, 2) > 0.2, "table {t}");
            assert!((level(&saw, 1) / level(&saw, 2) - 2.0).abs() < 0.1);
        }
    }

    #[test]
    fn the_band_limited_copies_leave_out_the_harmonics_that_would_fold() {
        let table = Table::get(0);
        // At 10 kHz on a 48 kHz rate only two harmonics fit.
        assert_eq!(Table::mip_for(10_000.0 / 48_000.0), 6);
        assert_eq!(Table::mip_for(100.0 / 48_000.0), 0);
        let cycle: Vec<f32> = (0..CYCLE)
            .map(|i| table.smooth(63.0, 6, i as f32 / CYCLE as f32))
            .collect();
        assert!(level(&cycle, 2) > 0.1);
        assert!(level(&cycle, 3) < 0.01);
    }

    #[test]
    fn the_sweeps_change_the_wave_and_a_fractional_position_sits_between() {
        for t in 0..NAMES.len() {
            let table = Table::get(t);
            let at = |p: f32| -> Vec<f32> {
                (0..CYCLE)
                    .map(|i| table.smooth(p, 0, i as f32 / CYCLE as f32))
                    .collect()
            };
            let (a, b) = (at(0.0), at(LAST_OWN as f32));
            let diff: f32 =
                a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f32>() / CYCLE as f32;
            assert!(diff > 0.05, "table {t} does not sweep: {diff}");
            let mid = at(0.5);
            let (a, b) = (at(0.0), at(1.0));
            for i in 0..CYCLE {
                let want = 0.5 * (a[i] + b[i]);
                assert!((mid[i] - want).abs() < 1e-4);
            }
        }
    }
}
