// SPDX-License-Identifier: GPL-3.0-or-later
//! Our own speed and pitch changer, written from published descriptions:
//!
//! - Phase vocoder time-scale modification: Flanagan & Golden, "Phase
//!   vocoder" (1966); Portnoff (1976); Dolson, "The phase vocoder: a
//!   tutorial" (1986).
//! - Identity phase locking around spectral peaks: Laroche & Dolson,
//!   "Improved phase vocoder time-scale modification of audio" (IEEE Trans.
//!   Speech and Audio Processing, 1999).
//! - Instantaneous frequency from a second analysis frame a fixed hop
//!   earlier, so the analysis hop can vary freely with speed.
//! - Phase reset at detected onsets to keep pick attacks sharp, after
//!   Duxbury, Davies & Sandler (DAFx 2002) and Röbel (ICMC 2003).
//! - Pitch change by time-stretching then resampling with a windowed-sinc
//!   interpolator (Smith, "Digital Audio Resampling", CCRMA).
//!
//! Mono. All buffers are allocated up front; `process` never allocates.

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::{Feed, Stretcher};

const TAPS: usize = 32;
const PHASES: usize = 256;

fn princarg(x: f32) -> f32 {
    x - 2.0 * PI * (x / (2.0 * PI)).round()
}

pub struct PhaseVocoder {
    name: &'static str,
    n: usize,
    hs: usize,
    window: Vec<f32>,
    norm: f32,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    frame: Vec<f32>,
    spec_a: Vec<Complex32>,
    spec_b: Vec<Complex32>,
    scratch: Vec<Complex32>,
    mag: Vec<f32>,
    phase_a: Vec<f32>,
    omega: Vec<f32>,
    syn_phase: Vec<f32>,
    owner: Vec<u32>,
    peaks: Vec<u32>,
    prev_flux: f32,
    cooldown: f64,
    first: bool,
    /// Overlap-add accumulator, `n` long.
    ola: Vec<f32>,
    /// Stretched signal waiting to be resampled.
    st: Vec<f32>,
    st_len: usize,
    rpos: f64,
    /// Analysis position in the input, in samples.
    t: f64,
    speed: f64,
    ratio: f64,
    kernel: Vec<f32>,
    pub onsets: u32,
}

impl PhaseVocoder {
    pub fn new(name: &'static str, n: usize, hs: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(n);
        let c2r = planner.plan_fft_inverse(n);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        let window: Vec<f32> = (0..n)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos())
            .collect();
        // Sum of squared windows at hop hs: dividing by it makes
        // analysis * synthesis windowing overlap-add to one.
        let sum: f32 = (0..n / hs).map(|m| window[m * hs].powi(2)).sum();
        let bins = n / 2 + 1;
        let mut pv = PhaseVocoder {
            name,
            n,
            hs,
            window,
            norm: 1.0 / (sum * n as f32),
            r2c,
            c2r,
            frame: vec![0.0; n],
            spec_a: vec![Complex32::default(); bins],
            spec_b: vec![Complex32::default(); bins],
            scratch: vec![Complex32::default(); scratch_len],
            mag: vec![0.0; bins],
            phase_a: vec![0.0; bins],
            omega: vec![0.0; bins],
            syn_phase: vec![0.0; bins],
            owner: vec![0; bins],
            peaks: Vec::with_capacity(bins),
            prev_flux: 0.0,
            cooldown: 0.0,
            first: true,
            ola: vec![0.0; n],
            st: vec![0.0; 8 * n + TAPS],
            st_len: TAPS / 2,
            rpos: (TAPS / 2) as f64,
            t: 0.0,
            speed: 1.0,
            ratio: 1.0,
            kernel: vec![0.0; PHASES * TAPS],
            onsets: 0,
        };
        pv.build_kernel();
        pv
    }

    /// Blackman-windowed sinc, low-passed below the new Nyquist when
    /// pitching up so the resampler does not alias.
    fn build_kernel(&mut self) {
        let fc = (1.0 / self.ratio).min(1.0) as f32 * 0.95;
        let half = (TAPS / 2) as f32;
        for p in 0..PHASES {
            let frac = p as f32 / PHASES as f32;
            for j in 0..TAPS {
                let x = j as f32 - (half - 1.0) - frac;
                let s = if x.abs() < 1e-6 {
                    1.0
                } else {
                    (PI * fc * x).sin() / (PI * fc * x)
                };
                let w = x / half;
                let bw = if w.abs() >= 1.0 {
                    0.0
                } else {
                    0.42 + 0.5 * (PI * w).cos() + 0.08 * (2.0 * PI * w).cos()
                };
                self.kernel[p * TAPS + j] = fc * s * bw;
            }
        }
    }

    fn analyse(&mut self, feed: &Feed, start: i64, into_a: bool) {
        for (i, (f, w)) in self.frame.iter_mut().zip(&self.window).enumerate() {
            *f = feed.at(start + i as i64) * w;
        }
        let spec = if into_a {
            &mut self.spec_a
        } else {
            &mut self.spec_b
        };
        self.r2c
            .process_with_scratch(&mut self.frame, spec, &mut self.scratch)
            .expect("forward FFT sizes");
    }

    fn synth_frame(&mut self, feed: &Feed) {
        let n = self.n;
        let hs = self.hs;
        let bins = n / 2 + 1;
        let start = self.t.round() as i64;
        self.analyse(feed, start - hs as i64, false);
        self.analyse(feed, start, true);

        // Magnitudes, phases, instantaneous frequencies, spectral flux.
        let mut flux = 0.0f32;
        let mut total = 1e-9f32;
        let mut max_mag = 0.0f32;
        for k in 0..bins {
            let a = self.spec_a[k];
            let b = self.spec_b[k];
            let ma = a.norm();
            let mb = b.norm();
            let pa = a.arg();
            let expected = 2.0 * PI * k as f32 * hs as f32 / n as f32;
            self.omega[k] = (expected + princarg(pa - b.arg() - expected)) / hs as f32;
            self.mag[k] = ma;
            self.phase_a[k] = pa;
            flux += (ma - mb).max(0.0);
            total += ma;
            max_mag = max_mag.max(ma);
        }
        let flux = flux / total;
        let onset = flux > 0.35 && flux > 1.5 * self.prev_flux && self.cooldown <= 0.0;
        self.prev_flux = flux;
        self.cooldown -= self.speed * hs as f64;

        if self.first || onset {
            // Take the analysis phases as they are: no smearing of the attack.
            self.syn_phase.copy_from_slice(&self.phase_a);
            self.first = false;
            if onset {
                self.onsets += 1;
                self.cooldown = n as f64;
            }
        } else {
            // Peaks, then each bin belongs to its nearest peak.
            self.peaks.clear();
            let floor = max_mag * 1e-4;
            for k in 2..bins - 2 {
                let m = self.mag[k];
                if m > floor
                    && m > self.mag[k - 1]
                    && m >= self.mag[k + 1]
                    && m > self.mag[k - 2]
                    && m >= self.mag[k + 2]
                {
                    self.peaks.push(k as u32);
                }
            }
            if self.peaks.is_empty() {
                for k in 0..bins {
                    self.syn_phase[k] += self.omega[k] * hs as f32;
                }
            } else {
                let mut pi = 0;
                for k in 0..bins {
                    let d = |p: u32| (p as i64 - k as i64).abs();
                    while pi + 1 < self.peaks.len() && d(self.peaks[pi + 1]) <= d(self.peaks[pi]) {
                        pi += 1;
                    }
                    self.owner[k] = self.peaks[pi];
                }
                for &p in &self.peaks {
                    let p = p as usize;
                    self.syn_phase[p] = princarg(self.syn_phase[p] + self.omega[p] * hs as f32);
                }
                for k in 0..bins {
                    let p = self.owner[k] as usize;
                    if p != k {
                        self.syn_phase[k] = self.syn_phase[p] + self.phase_a[k] - self.phase_a[p];
                    }
                }
            }
        }

        for k in 0..bins {
            self.spec_a[k] = Complex32::from_polar(self.mag[k], self.syn_phase[k]);
        }
        self.spec_a[0].im = 0.0;
        self.spec_a[bins - 1].im = 0.0;
        self.c2r
            .process_with_scratch(&mut self.spec_a, &mut self.frame, &mut self.scratch)
            .expect("inverse FFT sizes");
        for i in 0..n {
            self.ola[i] += self.frame[i] * self.window[i] * self.norm;
        }

        // The first hs samples are complete: move them on.
        let end = self.st_len + hs;
        assert!(end <= self.st.len(), "stretch buffer overflow");
        self.st[self.st_len..end].copy_from_slice(&self.ola[..hs]);
        self.st_len = end;
        self.ola.copy_within(hs.., 0);
        self.ola[n - hs..].fill(0.0);

        // Advance the analysis position: stretch by speed / pitch ratio,
        // the resampler then reads it back faster by the pitch ratio.
        self.t += self.speed / self.ratio * hs as f64;
    }
}

impl Stretcher for PhaseVocoder {
    fn name(&self) -> &str {
        self.name
    }

    fn set(&mut self, speed: f64, semitones: f64) -> bool {
        self.speed = speed;
        let ratio = 2f64.powf(semitones / 12.0);
        if ratio != self.ratio {
            self.ratio = ratio;
            self.build_kernel();
        }
        true
    }

    fn process(&mut self, feed: &mut Feed, out: &mut [f32]) {
        let half = TAPS / 2;
        let exact = self.ratio == 1.0;
        for o in out.iter_mut() {
            let i = self.rpos as usize;
            while i + half >= self.st_len {
                self.synth_frame(feed);
            }
            *o = if exact {
                self.st[i]
            } else {
                let frac = self.rpos - i as f64;
                let k = &self.kernel[((frac * PHASES as f64) as usize).min(PHASES - 1) * TAPS..]
                    [..TAPS];
                let s = &self.st[i + 1 - half..i + 1 + half];
                s.iter().zip(k).map(|(a, b)| a * b).sum()
            };
            self.rpos += self.ratio;
        }
        // Drop what the resampler no longer needs.
        let drop = (self.rpos as usize).saturating_sub(half);
        if drop > 0 {
            self.st.copy_within(drop..self.st_len, 0);
            self.st_len -= drop;
            self.rpos -= drop as f64;
        }
        feed.consumed = self.t;
    }
}
