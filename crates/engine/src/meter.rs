// SPDX-License-Identifier: GPL-3.0-or-later
//! Meters (roadmap 2.3): loudness, true peak, spectrum and phase
//! correlation.
//!
//! The audio thread only copies one bus, the master or a track chosen
//! with [`Command::SetMeterTap`](crate::engine::Command::SetMeterTap),
//! into a queue; [`Analyser`] does the work on the UI thread, as the
//! tuner does. Peak and RMS on every track are cheaper and kept by the
//! audio thread itself, in [`Status`](crate::engine::Status).
//!
//! Loudness follows Recommendation ITU-R BS.1770-5 (K-weighting, 400 ms
//! gating blocks with 75 % overlap, the −70 LKFS absolute and −10 LU
//! relative gates) and the meter of EBU Tech 3341 (momentary 400 ms,
//! short-term 3 s). Loudness range follows EBU Tech 3342. True peak uses
//! the 4× oversampling filter given as the example in BS.1770 Annex 2.
//! The tests below play the minimum-requirement signals of Tech 3341
//! and 3342 that can be made from their descriptions.

use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Which bus [`Analyser`] is fed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    Master,
    /// A track, by its index in the graph, after its fader and pan.
    Track(usize),
}

/// A second-order filter, transposed direct form II, in `f64` so the
/// low K-weighting corner stays accurate.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Biquad {
        Biquad { b, a, z: [0.0; 2] }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages of BS.1770 at `rate`: a high shelf for the
/// head, then a high-pass (the "RLB" curve). The standard gives
/// coefficients for 48 kHz only; these come from the analogue design
/// they were made from, so at 48 kHz they match its tables (see the
/// test) and other rates get the same curve.
fn k_weighting(rate: u32) -> [Biquad; 2] {
    let fs = rate as f64;
    // Stage 1: shelf.
    let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (std::f64::consts::PI * f0 / fs).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::new(
        [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    // Stage 2: high-pass.
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (std::f64::consts::PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad::new(
        [1.0, -2.0, 1.0],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    [shelf, hp]
}

/// Loudness from a mean square summed over channels (BS.1770 eq. 2).
fn lufs(power: f64) -> f64 {
    if power > 0.0 {
        -0.691 + 10.0 * power.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// A sum over the last `len` values, kept as they arrive.
struct Window {
    ring: Vec<f64>,
    at: usize,
    sum: f64,
}

impl Window {
    fn new(len: usize) -> Window {
        Window {
            ring: vec![0.0; len.max(1)],
            at: 0,
            sum: 0.0,
        }
    }

    fn push(&mut self, x: f64) {
        self.sum += x - self.ring[self.at];
        self.ring[self.at] = x;
        self.at = (self.at + 1) % self.ring.len();
    }

    /// Adds it up again, so rounding doesn't build up over hours.
    fn resum(&mut self) {
        self.sum = self.ring.iter().sum();
    }

    fn mean(&self) -> f64 {
        self.sum.max(0.0) / self.ring.len() as f64
    }
}

/// Programme loudness of a stereo signal: momentary, short-term,
/// integrated and loudness range.
pub struct Loudness {
    filters: [[Biquad; 2]; 2],
    momentary: Window,
    short: Window,
    /// Samples (frames) seen since the reset.
    seen: usize,
    /// Frames per 100 ms: gating blocks start this far apart.
    hop: usize,
    /// Mean square of each 400 ms gating block, for integrated loudness.
    blocks: Vec<f64>,
    /// Mean square of each 3 s window, every 100 ms, for the range.
    shorts: Vec<f64>,
    max_momentary: f64,
    max_short: f64,
}

/// Blocks below this are left out of integrated loudness and range.
const ABSOLUTE_GATE: f64 = -70.0;

impl Loudness {
    pub fn new(rate: u32) -> Loudness {
        let f = k_weighting(rate);
        let r = rate as usize;
        Loudness {
            filters: [f, f],
            momentary: Window::new(r * 4 / 10),
            short: Window::new(r * 3),
            seen: 0,
            hop: (r / 10).max(1),
            blocks: Vec::new(),
            shorts: Vec::new(),
            max_momentary: f64::NEG_INFINITY,
            max_short: f64::NEG_INFINITY,
        }
    }

    /// Takes interleaved stereo frames.
    pub fn push(&mut self, stereo: &[f32]) {
        for f in stereo.as_chunks::<2>().0 {
            let mut p = 0.0;
            for (c, &x) in f.iter().enumerate() {
                let [a, b] = &mut self.filters[c];
                let y = b.run(a.run(x as f64));
                // Every channel of a stereo pair weighs 1.0 (BS.1770 table 3).
                p += y * y;
            }
            self.momentary.push(p);
            self.short.push(p);
            self.seen += 1;
            let m_full = self.seen >= self.momentary.ring.len();
            let s_full = self.seen >= self.short.ring.len();
            if m_full {
                self.max_momentary = self.max_momentary.max(lufs(self.momentary.mean()));
            }
            if s_full {
                self.max_short = self.max_short.max(lufs(self.short.mean()));
            }
            if self.seen.is_multiple_of(self.hop) {
                self.momentary.resum();
                if m_full {
                    self.blocks.push(self.momentary.mean());
                }
                if self.seen.is_multiple_of(self.hop * 10) {
                    self.short.resum();
                }
                if s_full {
                    self.shorts.push(self.short.mean());
                }
            }
        }
    }

    /// Over the last 400 ms, in LUFS.
    pub fn momentary(&self) -> f64 {
        lufs(self.momentary.mean())
    }

    /// Over the last 3 s, in LUFS.
    pub fn short_term(&self) -> f64 {
        lufs(self.short.mean())
    }

    pub fn max_momentary(&self) -> f64 {
        self.max_momentary
    }

    pub fn max_short_term(&self) -> f64 {
        self.max_short
    }

    /// Gated loudness of everything since the reset (BS.1770 eq. 7), in
    /// LUFS; minus infinity until a block passes the gates.
    pub fn integrated(&self) -> f64 {
        let loud: Vec<f64> = self
            .blocks
            .iter()
            .copied()
            .filter(|&z| lufs(z) > ABSOLUTE_GATE)
            .collect();
        if loud.is_empty() {
            return f64::NEG_INFINITY;
        }
        let relative = lufs(mean(&loud)) - 10.0;
        let kept: Vec<f64> = loud.into_iter().filter(|&z| lufs(z) > relative).collect();
        if kept.is_empty() {
            return f64::NEG_INFINITY;
        }
        lufs(mean(&kept))
    }

    /// Loudness range (EBU Tech 3342), in LU: the spread between the 10th
    /// and 95th percentile of short-term loudness, after gates at −70
    /// LUFS and 20 LU under their mean. Zero until there is any.
    pub fn range(&self) -> f64 {
        let loud: Vec<f64> = self
            .shorts
            .iter()
            .copied()
            .filter(|&z| lufs(z) > ABSOLUTE_GATE)
            .collect();
        if loud.is_empty() {
            return 0.0;
        }
        let relative = lufs(mean(&loud)) - 20.0;
        let mut kept: Vec<f64> = loud
            .into_iter()
            .map(lufs)
            .filter(|&l| l > relative)
            .collect();
        if kept.is_empty() {
            return 0.0;
        }
        kept.sort_by(f64::total_cmp);
        let at = |p: f64| kept[((kept.len() - 1) as f64 * p).round() as usize];
        at(0.95) - at(0.10)
    }
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// The 48-tap, 4-phase interpolating filter of BS.1770 Annex 2, one row
/// per phase, digit for digit as the standard prints them.
#[allow(clippy::excessive_precision)]
const TRUE_PEAK_FIR: [[f32; 12]; 4] = [
    [
        0.0017089843750,
        0.0109863281250,
        -0.0196533203125,
        0.0332031250000,
        -0.0594482421875,
        0.1373291015625,
        0.9721679687500,
        -0.1022949218750,
        0.0476074218750,
        -0.0266113281250,
        0.0148925781250,
        -0.0083007812500,
    ],
    [
        -0.0291748046875,
        0.0292968750000,
        -0.0517578125000,
        0.0891113281250,
        -0.1665039062500,
        0.4650878906250,
        0.7797851562500,
        -0.2003173828125,
        0.1015625000000,
        -0.0582275390625,
        0.0330810546875,
        -0.0189208984375,
    ],
    [
        -0.0189208984375,
        0.0330810546875,
        -0.0582275390625,
        0.1015625000000,
        -0.2003173828125,
        0.7797851562500,
        0.4650878906250,
        -0.1665039062500,
        0.0891113281250,
        -0.0517578125000,
        0.0292968750000,
        -0.0291748046875,
    ],
    [
        -0.0083007812500,
        0.0148925781250,
        -0.0266113281250,
        0.0476074218750,
        -0.1022949218750,
        0.9721679687500,
        0.1373291015625,
        -0.0594482421875,
        0.0332031250000,
        -0.0196533203125,
        0.0109863281250,
        0.0017089843750,
    ],
];

/// The highest level between samples as well as at them, by 4×
/// oversampling (BS.1770 Annex 2), for each of two channels.
pub struct TruePeak {
    history: [[f32; 12]; 2],
    at: usize,
    max: [f32; 2],
}

impl Default for TruePeak {
    fn default() -> TruePeak {
        TruePeak {
            history: [[0.0; 12]; 2],
            at: 0,
            max: [0.0; 2],
        }
    }
}

impl TruePeak {
    /// Takes interleaved stereo frames.
    pub fn push(&mut self, stereo: &[f32]) {
        for f in stereo.as_chunks::<2>().0 {
            self.at = (self.at + 11) % 12;
            for (c, &x) in f.iter().enumerate() {
                let h = &mut self.history[c];
                h[self.at] = x;
                let mut m = x.abs();
                for phase in &TRUE_PEAK_FIR {
                    let mut y = 0.0;
                    for (k, coef) in phase.iter().enumerate() {
                        y += coef * h[(self.at + k) % 12];
                    }
                    m = m.max(y.abs());
                }
                self.max[c] = self.max[c].max(m);
            }
        }
    }

    /// Highest true peak (left, right) since the reset, linear.
    pub fn max(&self) -> (f32, f32) {
        (self.max[0], self.max[1])
    }
}

/// Phase correlation of two channels: +1 when they are the same, 0 when
/// unrelated, −1 when one is the other upside down. Averaged over about
/// a third of a second.
pub struct Correlation {
    decay: f64,
    lr: f64,
    ll: f64,
    rr: f64,
}

impl Correlation {
    pub fn new(rate: u32) -> Correlation {
        Correlation {
            decay: (-1.0 / (0.3 * rate as f64)).exp(),
            lr: 0.0,
            ll: 0.0,
            rr: 0.0,
        }
    }

    pub fn push(&mut self, stereo: &[f32]) {
        for f in stereo.as_chunks::<2>().0 {
            let (l, r) = (f[0] as f64, f[1] as f64);
            self.lr = self.lr * self.decay + l * r;
            self.ll = self.ll * self.decay + l * l;
            self.rr = self.rr * self.decay + r * r;
        }
    }

    /// −1 to +1; `None` while silent.
    pub fn value(&self) -> Option<f32> {
        let d = (self.ll * self.rr).sqrt();
        (d > 1e-10).then(|| (self.lr / d).clamp(-1.0, 1.0) as f32)
    }
}

/// Frames in each spectrum frame: about 85 ms at 48 kHz, 12 Hz apart.
pub const SPECTRUM_SIZE: usize = 4096;

/// The level of each frequency in the last [`SPECTRUM_SIZE`] frames,
/// left and right mixed.
pub struct Spectrum {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    frame: Vec<f32>,
    out: Vec<realfft::num_complex::Complex<f32>>,
    /// dB per FFT bin, where a full-scale sine reads 0.
    db: Vec<f32>,
}

impl Default for Spectrum {
    fn default() -> Spectrum {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(SPECTRUM_SIZE);
        let window: Vec<f32> = (0..SPECTRUM_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / SPECTRUM_SIZE as f32).cos())
            .collect();
        let out = fft.make_output_vec();
        Spectrum {
            db: vec![-200.0; out.len()],
            frame: vec![0.0; SPECTRUM_SIZE],
            fft,
            window,
            out,
        }
    }
}

impl Spectrum {
    /// Analyses the newest [`SPECTRUM_SIZE`] samples of `mono` (fewer
    /// are padded with silence before them). Returns dB per bin.
    pub fn analyse(&mut self, mono: &[f32]) -> &[f32] {
        let take = mono.len().min(SPECTRUM_SIZE);
        let pad = SPECTRUM_SIZE - take;
        self.frame[..pad].fill(0.0);
        for (i, &x) in mono[mono.len() - take..].iter().enumerate() {
            self.frame[pad + i] = x * self.window[pad + i];
        }
        if self.fft.process(&mut self.frame, &mut self.out).is_ok() {
            // A sine of amplitude A peaks at A × (sum of the window) / 2.
            let scale = 4.0 / SPECTRUM_SIZE as f32;
            for (d, c) in self.db.iter_mut().zip(&self.out) {
                *d = 20.0 * (c.norm() * scale).max(1e-10).log10();
            }
        }
        &self.db
    }
}

/// The loudest bin between each pair of `edges` (Hz), for drawing a
/// spectrum on a log scale. Bands holding no bin read between the two
/// nearest, so the low end is a slope rather than steps.
pub fn bands(db: &[f32], rate: u32, edges: &[f32]) -> Vec<f32> {
    let bin_hz = rate as f32 / SPECTRUM_SIZE as f32;
    let last = db.len().saturating_sub(1);
    edges
        .windows(2)
        .map(|e| {
            let lo = ((e[0] / bin_hz).ceil() as usize).min(last);
            let hi = ((e[1] / bin_hz).floor() as usize).min(last);
            if hi >= lo {
                db[lo..=hi].iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b))
            } else {
                let at = (e[0] * e[1]).sqrt() / bin_hz;
                let k = (at.floor() as usize).min(last);
                let t = at - k as f32;
                db[k] + (db[(k + 1).min(last)] - db[k]) * t
            }
        })
        .collect()
}

/// Everything shown for the tapped bus, fed from the engine's queue.
pub struct Analyser {
    rate: u32,
    pub loudness: Loudness,
    pub true_peak: TruePeak,
    pub correlation: Correlation,
    pub spectrum: Spectrum,
    /// The newest samples, left and right mixed, for the spectrum.
    recent: Vec<f32>,
    /// Highest sample (left, right) since the reset.
    sample_peak: [f32; 2],
}

impl Analyser {
    pub fn new(rate: u32) -> Analyser {
        Analyser {
            rate,
            loudness: Loudness::new(rate),
            true_peak: TruePeak::default(),
            correlation: Correlation::new(rate),
            spectrum: Spectrum::default(),
            recent: Vec::with_capacity(2 * SPECTRUM_SIZE),
            sample_peak: [0.0; 2],
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Starts again from nothing: integrated loudness, range and the
    /// highest readings.
    pub fn reset(&mut self) {
        *self = Analyser::new(self.rate);
    }

    /// Takes interleaved stereo frames.
    pub fn push(&mut self, stereo: &[f32]) {
        self.loudness.push(stereo);
        self.true_peak.push(stereo);
        self.correlation.push(stereo);
        for f in stereo.as_chunks::<2>().0 {
            self.sample_peak[0] = self.sample_peak[0].max(f[0].abs());
            self.sample_peak[1] = self.sample_peak[1].max(f[1].abs());
            self.recent.push(0.5 * (f[0] + f[1]));
        }
        if self.recent.len() > SPECTRUM_SIZE {
            self.recent.drain(..self.recent.len() - SPECTRUM_SIZE);
        }
    }

    /// The spectrum of the newest audio: dB per bin.
    pub fn spectrum(&mut self) -> &[f32] {
        self.spectrum.analyse(&self.recent)
    }

    pub fn sample_peak(&self) -> (f32, f32) {
        (self.sample_peak[0], self.sample_peak[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A stereo 1 kHz sine, the same on both channels, peaking at `dbfs`
    /// for `secs`, continuing the phase from `t` (frames).
    fn tone(out: &mut Vec<f32>, t: &mut usize, dbfs: f64, secs: f64) {
        let a = 10f64.powf(dbfs / 20.0);
        let n = (secs * RATE as f64).round() as usize;
        for i in *t..*t + n {
            let x = (a * (std::f64::consts::TAU * 1000.0 * i as f64 / RATE as f64).sin()) as f32;
            out.push(x);
            out.push(x);
        }
        *t += n;
    }

    fn silence(out: &mut Vec<f32>, t: &mut usize, secs: f64) {
        let n = (secs * RATE as f64).round() as usize;
        out.extend(std::iter::repeat_n(0.0, 2 * n));
        *t += n;
    }

    fn segments(parts: &[(f64, f64)]) -> Vec<f32> {
        let (mut v, mut t) = (Vec::new(), 0);
        for &(db, secs) in parts {
            tone(&mut v, &mut t, db, secs);
        }
        v
    }

    fn measure(signal: &[f32]) -> Loudness {
        let mut l = Loudness::new(RATE);
        // Fed in uneven pieces, as the queue delivers it.
        for piece in signal.chunks(2 * 1237) {
            l.push(piece);
        }
        l
    }

    fn near(what: &str, got: f64, want: f64, tol: f64) {
        assert!(
            (got - want).abs() <= tol,
            "{what}: got {got:.3}, want {want} ± {tol}"
        );
    }

    #[test]
    fn k_weighting_matches_the_standards_48k_tables() {
        let [shelf, hp] = k_weighting(48_000);
        let want_shelf = (
            [1.53512485958697, -2.69169618940638, 1.19839281085285],
            [-1.69065929318241, 0.73248077421585],
        );
        let want_hp = ([1.0, -2.0, 1.0], [-1.99004745483398, 0.99007225036621]);
        for (got, want) in [(shelf, want_shelf), (hp, want_hp)] {
            for (g, w) in got.b.iter().zip(want.0) {
                assert!((g - w).abs() < 1e-8, "b {g} vs {w}");
            }
            for (g, w) in got.a.iter().zip(want.1) {
                assert!((g - w).abs() < 1e-8, "a {g} vs {w}");
            }
        }
    }

    // EBU Tech 3341, table 1: tolerance ±0.1 LU.

    #[test]
    fn tech3341_tests_1_and_2_steady_tones() {
        for (db, want) in [(-23.0, -23.0), (-33.0, -33.0)] {
            let l = measure(&segments(&[(db, 20.0)]));
            near("momentary", l.momentary(), want, 0.1);
            near("short-term", l.short_term(), want, 0.1);
            near("integrated", l.integrated(), want, 0.1);
        }
    }

    #[test]
    fn tech3341_test_3_relative_gate() {
        let l = measure(&segments(&[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)]));
        near("integrated", l.integrated(), -23.0, 0.1);
    }

    #[test]
    fn tech3341_test_4_absolute_gate() {
        let l = measure(&segments(&[
            (-72.0, 10.0),
            (-36.0, 10.0),
            (-23.0, 60.0),
            (-36.0, 10.0),
            (-72.0, 10.0),
        ]));
        near("integrated", l.integrated(), -23.0, 0.1);
    }

    #[test]
    fn tech3341_test_5_averaging() {
        let l = measure(&segments(&[(-26.0, 20.0), (-20.0, 20.1), (-26.0, 20.0)]));
        near("integrated", l.integrated(), -23.0, 0.1);
    }

    #[test]
    fn tech3341_test_9_short_term_window() {
        let parts: Vec<_> = (0..5)
            .flat_map(|_| [(-20.0, 1.34), (-30.0, 1.66)])
            .collect();
        let signal = segments(&parts);
        let mut l = Loudness::new(RATE);
        // Constant after 3 s: read it every 100 ms from then on.
        for (i, piece) in signal.chunks(2 * RATE as usize / 10).enumerate() {
            l.push(piece);
            if i >= 30 {
                near("short-term", l.short_term(), -23.0, 0.1);
            }
        }
    }

    #[test]
    fn tech3341_test_12_momentary_window() {
        let parts: Vec<_> = (0..25)
            .flat_map(|_| [(-20.0, 0.18), (-30.0, 0.22)])
            .collect();
        let signal = segments(&parts);
        let mut l = Loudness::new(RATE);
        for (i, piece) in signal.chunks(2 * RATE as usize / 100).enumerate() {
            l.push(piece);
            if i >= 100 {
                near("momentary", l.momentary(), -23.0, 0.1);
            }
        }
    }

    #[test]
    fn tech3341_tests_11_and_14_highest_readings_in_steps() {
        // 11: 3 s tones; 14: 400 ms tones, each after a growing silence.
        for (len, step, test) in [(3.0, 0.15, 11), (0.4, 0.02, 14)] {
            let (mut v, mut t) = (Vec::new(), 0);
            let mut ends = Vec::new();
            for i in 0..20 {
                silence(&mut v, &mut t, i as f64 * step);
                tone(&mut v, &mut t, -38.0 + i as f64, len);
                silence(&mut v, &mut t, len - i as f64 * step);
                ends.push(v.len());
            }
            let mut l = Loudness::new(RATE);
            let mut from = 0;
            for (i, &end) in ends.iter().enumerate() {
                l.push(&v[from..end]);
                from = end;
                let got = if test == 11 {
                    l.max_short_term()
                } else {
                    l.max_momentary()
                };
                near(
                    &format!("test {test}, step {i}"),
                    got,
                    -38.0 + i as f64,
                    0.1,
                );
            }
        }
    }

    #[test]
    fn calibration_tone_reads_its_level() {
        let l = measure(&segments(&[(-18.0, 10.0)]));
        near("integrated", l.integrated(), -18.0, 0.1);
    }

    #[test]
    fn other_rates_measure_the_same() {
        for rate in [44_100u32, 96_000] {
            let mut v = Vec::new();
            let a = 10f64.powf(-23.0 / 20.0);
            for i in 0..rate as usize * 10 {
                let x =
                    (a * (std::f64::consts::TAU * 1000.0 * i as f64 / rate as f64).sin()) as f32;
                v.extend([x, x]);
            }
            let mut l = Loudness::new(rate);
            l.push(&v);
            near(&format!("{rate} Hz"), l.integrated(), -23.0, 0.1);
        }
    }

    #[test]
    fn silence_has_no_loudness() {
        let mut l = Loudness::new(RATE);
        l.push(&vec![0.0; 2 * RATE as usize * 5]);
        assert_eq!(l.integrated(), f64::NEG_INFINITY);
        assert_eq!(l.momentary(), f64::NEG_INFINITY);
        assert_eq!(l.range(), 0.0);
    }

    // EBU Tech 3342: tolerance ±1 LU.

    #[test]
    fn tech3342_loudness_range() {
        for (parts, want) in [
            (vec![(-20.0, 20.0), (-30.0, 20.0)], 10.0),
            (vec![(-20.0, 20.0), (-15.0, 20.0)], 5.0),
            (vec![(-40.0, 20.0), (-20.0, 20.0)], 20.0),
            (
                vec![
                    (-50.0, 20.0),
                    (-35.0, 20.0),
                    (-20.0, 20.0),
                    (-35.0, 20.0),
                    (-50.0, 20.0),
                ],
                15.0,
            ),
        ] {
            let l = measure(&segments(&parts));
            near(&format!("{parts:?}"), l.range(), want, 1.0);
        }
    }

    // EBU Tech 3341 tests 15 to 19: tolerance +0.2 / −0.4 dB.

    fn true_peak_db(cycle: f64, amp: f64, phase_deg: f64) -> f64 {
        let n = RATE as usize;
        let fade = (0.01 * RATE as f64) as usize;
        let mut v = Vec::with_capacity(2 * n);
        for i in 0..n {
            let ramp = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
            let x = amp
                * ramp
                * (std::f64::consts::TAU * i as f64 / cycle + phase_deg.to_radians()).sin();
            v.extend([x as f32, x as f32]);
        }
        let mut tp = TruePeak::default();
        tp.push(&v);
        let (l, r) = tp.max();
        assert_eq!(l, r);
        20.0 * (l as f64).log10()
    }

    #[test]
    fn tech3341_true_peak_tests_15_to_19() {
        for (test, cycle, amp, phase, want) in [
            (15, 4.0, 0.5, 0.0, -6.0),
            (16, 4.0, 0.5, 45.0, -6.0),
            (17, 6.0, 0.5, 60.0, -6.0),
            (18, 8.0, 0.5, 67.5, -6.0),
            (19, 4.0, 1.41, 45.0, 3.0),
        ] {
            let got = true_peak_db(cycle, amp, phase);
            assert!(
                got <= want + 0.2 && got >= want - 0.4,
                "test {test}: {got:.2} dBTP, want {want}"
            );
        }
    }

    #[test]
    fn true_peak_is_above_the_samples_when_peaks_fall_between() {
        // fs/4 at 45°: every sample is at 0.707 of the real peak.
        let got = true_peak_db(4.0, 0.5, 45.0);
        let samples = 20.0 * (0.5f64 * 0.5f64.sqrt()).log10();
        assert!(got > samples + 2.5, "{got} vs samples {samples}");
    }

    #[test]
    fn correlation_of_same_opposite_and_unrelated() {
        let n = RATE as usize;
        let s =
            |i: usize, hz: f64| (std::f64::consts::TAU * hz * i as f64 / RATE as f64).sin() as f32;
        for (want, right) in [
            (
                1.0f32,
                Box::new(|i| s(i, 440.0)) as Box<dyn Fn(usize) -> f32>,
            ),
            (-1.0, Box::new(|i| -s(i, 440.0))),
            (0.0, Box::new(|i| s(i, 1234.5))),
        ] {
            let mut c = Correlation::new(RATE);
            let v: Vec<f32> = (0..n).flat_map(|i| [s(i, 440.0), right(i)]).collect();
            c.push(&v);
            let got = c.value().unwrap();
            assert!((got - want).abs() < 0.05, "{got} vs {want}");
        }
        assert_eq!(Correlation::new(RATE).value(), None);
    }

    #[test]
    fn spectrum_shows_a_sine_at_its_frequency_and_level() {
        let a = 0.5f32; // −6 dBFS
        let mono: Vec<f32> = (0..SPECTRUM_SIZE * 2)
            .map(|i| a * (std::f32::consts::TAU * 1000.0 * i as f32 / RATE as f32).sin())
            .collect();
        let mut s = Spectrum::default();
        let db = s.analyse(&mono).to_vec();
        let (peak_bin, peak) = db
            .iter()
            .enumerate()
            .fold((0, f32::MIN), |m, (i, &d)| if d > m.1 { (i, d) } else { m });
        let hz = peak_bin as f32 * RATE as f32 / SPECTRUM_SIZE as f32;
        assert!((hz - 1000.0).abs() < 12.0, "{hz} Hz");
        // A Hann window loses at most 1.42 dB between bins.
        assert!((-6.0 - 1.5..=-6.0 + 0.1).contains(&peak), "{peak} dB");
        // Third-octave bands: the one holding 1 kHz is the loudest.
        let edges: Vec<f32> = (0..31).map(|i| 20.0 * 2f32.powf(i as f32 / 3.0)).collect();
        let b = bands(&db, RATE, &edges);
        let loudest = b
            .iter()
            .enumerate()
            .fold((0, f32::MIN), |m, (i, &d)| if d > m.1 { (i, d) } else { m })
            .0;
        assert!(edges[loudest] <= 1000.0 && edges[loudest + 1] >= 1000.0);
        assert!(b[0] < -80.0, "20 Hz band {}", b[0]);
    }

    #[test]
    fn analyser_resets() {
        let mut a = Analyser::new(RATE);
        a.push(&segments(&[(-10.0, 2.0)]));
        assert!(a.loudness.integrated() > -20.0);
        assert!(a.sample_peak().0 > 0.3);
        a.reset();
        assert_eq!(a.loudness.integrated(), f64::NEG_INFINITY);
        assert_eq!(a.sample_peak(), (0.0, 0.0));
    }
}
