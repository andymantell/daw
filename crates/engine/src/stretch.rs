// SPDX-License-Identifier: GPL-3.0-or-later
//! Pitch-preserving speed change and transpose for practice (roadmap
//! 1.4): our phase vocoder from spike 0.6, made stereo and driven by the
//! output, reading its input wherever it needs through a [`Feed`].
//!
//! Written from published descriptions:
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
//! - Keeping formants when transposing: each frame's spectral envelope
//!   is taken by cepstral smoothing (low-quefrency liftering of the log
//!   magnitude; Oppenheim & Schafer, "Discrete-Time Signal Processing",
//!   on the real cepstrum), and every bin is scaled by the envelope where
//!   the resampler will move it over the envelope where it is, so the
//!   harmonics move and the resonances stay (after Röbel & Rodet,
//!   "Efficient spectral envelope estimation and its application to
//!   pitch shifting and envelope preservation", DAFx 2005, which uses a
//!   refined "true envelope"; plain cepstral smoothing is the simpler
//!   first step it starts from).
//!
//! Timing: after [`Stretcher::reset`] at input position `u0`, output
//! frame `o` plays input position `u0 + o × speed`, so a stretched track
//! stays in time with the transport. The first frames are made from
//! input before `u0`, as if it had been playing all along.
//!
//! The two channels share one analysis position and one onset detector,
//! so they stay in step. Everything is allocated in [`Stretcher::new`];
//! nothing after allocates.
//!
//! Which stretcher to ship is still open (spike 0.6 listening); another
//! library would sit behind the same three calls.

use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Supplies input audio, stereo, by input position.
pub trait Feed {
    /// Fills `l` and `r` (the same length) with input from `start` on.
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]);
}

impl<F: Feed + ?Sized> Feed for &mut F {
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]) {
        (**self).read(start, l, r)
    }
}

/// Analysis frame length: about 43 ms at 48 kHz.
pub const FRAME: usize = 2048;
/// Synthesis hop.
const HOP: usize = 512;
const TAPS: usize = 32;
const PHASES: usize = 256;
/// Frame centres remembered: enough to cover the stretched signal held
/// for a block of any likely size.
const CENTRES: usize = 64;
/// Cepstral coefficients kept for the spectral envelope: about 1 ms at
/// 48 kHz, shorter than the period of any sung or spoken pitch up to
/// about 1 kHz, so the envelope follows the resonances and not the
/// harmonics.
const LIFTER: usize = 48;
/// The most a bin is raised or lowered to keep the envelope, in natural
/// log units (about ±24 dB): beyond it the envelope of a near-silent
/// region is guesswork.
const MAX_FORMANT_LOG: f32 = 2.77;

/// Slowest and fastest practice speeds, and the widest transpose,
/// offered.
pub const SPEED_RANGE: (f64, f64) = (0.5, 1.5);
pub const SEMITONES: f64 = 12.0;
/// The speeds the stretcher itself takes: wider, for a clip following a
/// tempo far from its own (see [`crate::warp`]).
pub const FULL_SPEED_RANGE: (f64, f64) = (0.25, 4.0);

fn princarg(x: f32) -> f32 {
    x - 2.0 * PI * (x / (2.0 * PI)).round()
}

/// Per-channel state.
struct Chan {
    spec_a: Vec<Complex32>,
    spec_b: Vec<Complex32>,
    mag: Vec<f32>,
    phase_a: Vec<f32>,
    omega: Vec<f32>,
    syn_phase: Vec<f32>,
    /// Overlap-add accumulator, `FRAME` long.
    ola: Vec<f32>,
    /// Stretched signal waiting to be resampled.
    st: Vec<f32>,
    /// Input read for one hop's two analysis frames.
    input: Vec<f32>,
    /// The frame's spectral envelope, as a natural log of magnitude.
    env: Vec<f32>,
}

impl Chan {
    fn new(max_block: usize) -> Chan {
        let bins = FRAME / 2 + 1;
        Chan {
            spec_a: vec![Complex32::default(); bins],
            spec_b: vec![Complex32::default(); bins],
            mag: vec![0.0; bins],
            phase_a: vec![0.0; bins],
            omega: vec![0.0; bins],
            syn_phase: vec![0.0; bins],
            ola: vec![0.0; FRAME],
            st: vec![0.0; st_len(max_block)],
            input: vec![0.0; FRAME + HOP],
            env: vec![0.0; bins],
        }
    }
}

/// Room for one block's worth of stretched signal at the highest pitch,
/// plus a frame and the resampler's reach.
fn st_len(max_block: usize) -> usize {
    let ratio = 2f64.powf(SEMITONES / 12.0);
    (max_block as f64 * ratio).ceil() as usize + 2 * FRAME + 2 * TAPS
}

pub struct Stretcher {
    window: Vec<f32>,
    norm: f32,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    frame: Vec<f32>,
    scratch: Vec<Complex32>,
    chans: [Chan; 2],
    owner: Vec<u32>,
    peaks: Vec<u32>,
    prev_flux: f32,
    cooldown: f64,
    first: bool,
    st_len: usize,
    rpos: f64,
    /// Analysis position in the input.
    t: f64,
    speed: f64,
    /// The speed the last frame was made at: what the output between it
    /// and the one before plays at, whatever `speed` has since been set
    /// to.
    hop_speed: f64,
    /// Where recent frames were centred: in the stretched signal, counted
    /// from the start (`s_base` is where `st[0]` is in that count), and
    /// in the input. Between two centres the input position is linear,
    /// so [`position`](Self::position) is exact. Capacity [`CENTRES`].
    centres: VecDeque<(f64, f64)>,
    s_base: f64,
    ratio: f64,
    kernel: Vec<f32>,
    max_block: usize,
    /// Keep the spectral envelope where it is when transposing.
    formants: bool,
    /// Moves the spectral envelope by this ratio on top (1 for none):
    /// a voice made bigger (below 1) or smaller (above).
    formant_ratio: f64,
    /// The low-pass cutoff the resampler's kernel was built for.
    kernel_fc: f32,
    /// Room for the envelope's cepstrum and its spectrum.
    cep: Vec<f32>,
    cep_spec: Vec<Complex32>,
}

impl Stretcher {
    /// A stretcher for blocks of up to `max_block` output frames.
    pub fn new(max_block: usize) -> Stretcher {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(FRAME);
        let c2r = planner.plan_fft_inverse(FRAME);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        let window: Vec<f32> = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / FRAME as f32).cos())
            .collect();
        // Sum of squared windows at the hop: dividing by it makes analysis
        // and synthesis windowing overlap-add to one.
        let sum: f32 = (0..FRAME / HOP).map(|m| window[m * HOP].powi(2)).sum();
        let bins = FRAME / 2 + 1;
        let mut s = Stretcher {
            window,
            norm: 1.0 / (sum * FRAME as f32),
            r2c,
            c2r,
            frame: vec![0.0; FRAME],
            scratch: vec![Complex32::default(); scratch_len],
            chans: [Chan::new(max_block), Chan::new(max_block)],
            owner: vec![0; bins],
            peaks: Vec::with_capacity(bins),
            prev_flux: 0.0,
            cooldown: 0.0,
            first: true,
            st_len: 0,
            rpos: 0.0,
            t: 0.0,
            speed: 1.0,
            hop_speed: 1.0,
            centres: VecDeque::with_capacity(CENTRES),
            s_base: 0.0,
            ratio: 1.0,
            kernel: vec![0.0; PHASES * TAPS],
            max_block: max_block.max(1),
            formants: false,
            formant_ratio: 1.0,
            kernel_fc: f32::NAN,
            cep: vec![0.0; FRAME],
            cep_spec: vec![Complex32::default(); bins],
        };
        s.build_kernel();
        s.reset(0.0);
        s
    }

    /// Sets speed (1 is normal; within [`FULL_SPEED_RANGE`]) and transpose
    /// in semitones (±[`SEMITONES`], fractions for cents). Takes effect
    /// from the next frame made; doesn't allocate.
    pub fn set(&mut self, speed: f64, semitones: f64) {
        self.speed = speed.clamp(FULL_SPEED_RANGE.0, FULL_SPEED_RANGE.1);
        let ratio = 2f64.powf(semitones.clamp(-SEMITONES, SEMITONES) / 12.0);
        if ratio != self.ratio {
            self.ratio = ratio;
            // The kernel only depends on the cutoff, which only changes
            // when pitching up; a cutoff a little off is inaudible, so a
            // pitch that moves every block (pitch editing) doesn't
            // rebuild it every block.
            if (kernel_cutoff(ratio) - self.kernel_fc).abs() > 0.004 || self.kernel_fc.is_nan() {
                self.build_kernel();
            }
        }
    }

    /// Moves the formants by `semitones` (±[`SEMITONES`]) on top of
    /// whatever the transpose does to them: down makes a voice sound
    /// bigger, up smaller. Takes effect from the next frame made.
    pub fn set_formant_shift(&mut self, semitones: f64) {
        self.formant_ratio = 2f64.powf(semitones.clamp(-SEMITONES, SEMITONES) / 12.0);
    }

    /// The formant shift set, in semitones.
    pub fn formant_shift(&self) -> f64 {
        12.0 * self.formant_ratio.log2()
    }

    /// Whether a transpose keeps the formants (the resonances of a voice
    /// or body) where they are, moving only the pitch, rather than
    /// moving them with it as a tape sped up does. Off at first; takes
    /// effect from the next frame made.
    pub fn set_formants(&mut self, keep: bool) {
        self.formants = keep;
    }

    /// Whether a transpose keeps the formants.
    pub fn formants(&self) -> bool {
        self.formants
    }

    /// Starts again so that the next output frame plays input `u0`.
    pub fn reset(&mut self, u0: f64) {
        for c in &mut self.chans {
            c.ola.fill(0.0);
            c.st[..TAPS / 2].fill(0.0);
        }
        self.st_len = TAPS / 2;
        // The first FRAME − HOP stretched samples lack earlier frames'
        // overlap; start reading after them, with the analysis that far
        // back so output 0 still lines up with u0.
        self.rpos = TAPS as f64 / 2.0 + (FRAME - HOP) as f64;
        self.t = u0 - (FRAME / 2 - HOP) as f64 * self.speed / self.ratio;
        self.hop_speed = self.speed;
        self.centres.clear();
        self.s_base = 0.0;
        self.first = true;
        self.prev_flux = 0.0;
        self.cooldown = 0.0;
    }

    /// The transpose set, in semitones.
    pub fn semitones(&self) -> f64 {
        12.0 * self.ratio.log2()
    }

    /// Input position the next output frame plays.
    pub fn position(&self) -> f64 {
        // Each frame is centred on its analysis position; between two
        // centres the input position runs evenly, and beyond the ones
        // remembered it runs at the last speed used.
        let s = self.s_base + self.rpos;
        let step = self.hop_speed / self.ratio;
        let n = self.centres.len();
        if n == 0 {
            // Nothing made yet: back from where the next frame's centre
            // will be.
            return self.t - ((self.st_len + FRAME / 2) as f64 - self.rpos) * step;
        }
        let i = self.centres.partition_point(|&(at, _)| at <= s);
        if i == 0 {
            let (at, c) = self.centres[0];
            return c - (at - s) * step;
        }
        if i == n {
            let (at, c) = self.centres[n - 1];
            return c + (s - at) * step;
        }
        let ((a_at, a), (b_at, b)) = (self.centres[i - 1], self.centres[i]);
        a + (s - a_at) * (b - a) / (b_at - a_at)
    }

    /// Blackman-windowed sinc, low-passed below the new Nyquist when
    /// pitching up so the resampler does not alias.
    fn build_kernel(&mut self) {
        let fc = kernel_cutoff(self.ratio);
        self.kernel_fc = fc;
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

    /// Makes one hop of stretched signal on both channels.
    fn synth_frame(&mut self, feed: &mut impl Feed) {
        let bins = FRAME / 2 + 1;
        let start = self.t.round() as i64;
        {
            let [a, b] = &mut self.chans;
            // Centred on `start`: a click stays where it belongs in time
            // instead of moving with its place in the frame.
            feed.read(start - (FRAME / 2 + HOP) as i64, &mut a.input, &mut b.input);
        }

        // Analysis, and spectral flux summed over both channels.
        let mut flux = 0.0f32;
        let mut total = 1e-9f32;
        for c in &mut self.chans {
            for (into_a, off) in [(false, 0), (true, HOP)] {
                for (i, (f, w)) in self.frame.iter_mut().zip(&self.window).enumerate() {
                    *f = c.input[off + i] * w;
                }
                let spec = if into_a { &mut c.spec_a } else { &mut c.spec_b };
                self.r2c
                    .process_with_scratch(&mut self.frame, spec, &mut self.scratch)
                    .expect("forward FFT sizes");
            }
            for k in 0..bins {
                let (a, b) = (c.spec_a[k], c.spec_b[k]);
                let (ma, mb) = (a.norm(), b.norm());
                let pa = a.arg();
                let expected = 2.0 * PI * k as f32 * HOP as f32 / FRAME as f32;
                c.omega[k] = (expected + princarg(pa - b.arg() - expected)) / HOP as f32;
                c.mag[k] = ma;
                c.phase_a[k] = pa;
                flux += (ma - mb).max(0.0);
                total += ma;
            }
        }
        let flux = flux / total;
        let onset = flux > 0.35 && flux > 1.5 * self.prev_flux && self.cooldown <= 0.0;
        self.prev_flux = flux;
        self.cooldown -= self.speed * HOP as f64;
        let reset = self.first || onset;
        if onset {
            self.cooldown = FRAME as f64;
        }
        self.first = false;

        for c in &mut self.chans {
            if reset {
                // Take the analysis phases as they are: no smearing of
                // the attack.
                c.syn_phase.copy_from_slice(&c.phase_a);
            } else {
                // Peaks, then each bin follows its nearest peak.
                self.peaks.clear();
                let max_mag = c.mag.iter().fold(0.0f32, |m, &x| m.max(x));
                let floor = max_mag * 1e-4;
                for k in 2..bins - 2 {
                    let m = c.mag[k];
                    if m > floor
                        && m > c.mag[k - 1]
                        && m >= c.mag[k + 1]
                        && m > c.mag[k - 2]
                        && m >= c.mag[k + 2]
                    {
                        self.peaks.push(k as u32);
                    }
                }
                if self.peaks.is_empty() {
                    for k in 0..bins {
                        c.syn_phase[k] += c.omega[k] * HOP as f32;
                    }
                } else {
                    let mut pi = 0;
                    for k in 0..bins {
                        let d = |p: u32| (p as i64 - k as i64).abs();
                        while pi + 1 < self.peaks.len()
                            && d(self.peaks[pi + 1]) <= d(self.peaks[pi])
                        {
                            pi += 1;
                        }
                        self.owner[k] = self.peaks[pi];
                    }
                    for &p in &self.peaks {
                        let p = p as usize;
                        c.syn_phase[p] = princarg(c.syn_phase[p] + c.omega[p] * HOP as f32);
                    }
                    for k in 0..bins {
                        let p = self.owner[k] as usize;
                        if p != k {
                            c.syn_phase[k] = c.syn_phase[p] + c.phase_a[k] - c.phase_a[p];
                        }
                    }
                }
            }

            // Where each final frequency's envelope comes from, as a
            // ratio: 1 keeps the formants, the pitch ratio moves them
            // with it, and a formant shift multiplies in.
            let moved = self.formant_ratio * if self.formants { 1.0 } else { self.ratio };
            if moved != self.ratio {
                envelope(
                    &c.mag,
                    &mut c.env,
                    &mut self.cep,
                    &mut self.cep_spec,
                    &self.r2c,
                    &self.c2r,
                    &mut self.scratch,
                );
                // Bin k comes out at k × ratio once resampled: give it the
                // envelope from k × ratio / moved instead of its own.
                let ratio = (self.ratio / moved) as f32;
                let last = (bins - 1) as f32;
                for k in 0..bins {
                    let at = (k as f32 * ratio).min(last);
                    let i = at as usize;
                    let f = at - i as f32;
                    let there = match c.env.get(i + 1) {
                        Some(&b) => c.env[i] + f * (b - c.env[i]),
                        None => c.env[i],
                    };
                    let gain = (there - c.env[k]).clamp(-MAX_FORMANT_LOG, MAX_FORMANT_LOG);
                    c.mag[k] *= gain.exp();
                }
            }
            for k in 0..bins {
                c.spec_a[k] = Complex32::from_polar(c.mag[k], c.syn_phase[k]);
            }
            c.spec_a[0].im = 0.0;
            c.spec_a[bins - 1].im = 0.0;
            self.c2r
                .process_with_scratch(&mut c.spec_a, &mut self.frame, &mut self.scratch)
                .expect("inverse FFT sizes");
            for i in 0..FRAME {
                c.ola[i] += self.frame[i] * self.window[i] * self.norm;
            }
            // The first hop is complete: move it on.
            let end = self.st_len + HOP;
            c.st[self.st_len..end].copy_from_slice(&c.ola[..HOP]);
            c.ola.copy_within(HOP.., 0);
            c.ola[FRAME - HOP..].fill(0.0);
        }
        if self.centres.len() == CENTRES {
            self.centres.pop_front();
        }
        self.centres
            .push_back((self.s_base + (self.st_len + FRAME / 2) as f64, self.t));
        self.st_len += HOP;

        // Stretch by speed over pitch; the resampler then reads it back
        // faster by the pitch ratio.
        self.hop_speed = self.speed;
        self.t += self.speed / self.ratio * HOP as f64;
    }

    /// Fills `l` and `r` (at most `max_block` frames) at the current
    /// settings, reading input from `feed`.
    pub fn process(&mut self, feed: &mut impl Feed, l: &mut [f32], r: &mut [f32]) {
        self.drive(feed, None, l, r)
    }

    /// Fills `l` and `r` (at most `max_block` frames) so that output
    /// frame `o` of them plays input `at(o)`, exactly at each frame
    /// centre and evenly between: for audio that follows a warp. `at` is
    /// asked about frames past the end of the block too, since frames
    /// are made ahead of the output. The transpose set stays.
    pub fn process_along(
        &mut self,
        feed: &mut impl Feed,
        at: &dyn Fn(f64) -> f64,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        self.drive(feed, Some(at), l, r)
    }

    fn drive(
        &mut self,
        feed: &mut impl Feed,
        at: Option<&dyn Fn(f64) -> f64>,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        let n = l.len().min(r.len()).min(self.max_block);
        let half = TAPS / 2;
        let exact = self.ratio == 1.0;
        for o in 0..n {
            let i = self.rpos as usize;
            while i + half >= self.st_len {
                if let Some(at) = at {
                    // The frame about to be made is centred this far
                    // ahead of output `o`.
                    let oc = o as f64 + ((self.st_len + FRAME / 2) as f64 - self.rpos) / self.ratio;
                    let c = at(oc);
                    let speed = (c - self.t) / HOP as f64 * self.ratio;
                    self.speed = speed.clamp(FULL_SPEED_RANGE.0, FULL_SPEED_RANGE.1);
                    self.t = c;
                }
                self.synth_frame(feed);
            }
            let frac = self.rpos - i as f64;
            let k =
                &self.kernel[((frac * PHASES as f64) as usize).min(PHASES - 1) * TAPS..][..TAPS];
            for (c, out) in self.chans.iter().zip([&mut *l, &mut *r]) {
                out[o] = if exact {
                    c.st[i]
                } else {
                    c.st[i + 1 - half..i + 1 + half]
                        .iter()
                        .zip(k)
                        .map(|(a, b)| a * b)
                        .sum()
                };
            }
            self.rpos += self.ratio;
        }
        // Drop what the resampler no longer needs.
        let drop = (self.rpos as usize).saturating_sub(half);
        if drop > 0 {
            for c in &mut self.chans {
                c.st.copy_within(drop..self.st_len, 0);
            }
            self.st_len -= drop;
            self.rpos -= drop as f64;
            self.s_base += drop as f64;
        }
    }
}

/// The resampler's low-pass cutoff at pitch ratio `ratio`, as a
/// fraction of the input's Nyquist: below the new Nyquist when pitching
/// up so it does not alias.
fn kernel_cutoff(ratio: f64) -> f32 {
    (1.0 / ratio).min(1.0) as f32 * 0.95
}

/// The spectral envelope of magnitudes `mag` into `env`, as natural log
/// magnitude per bin: the log spectrum's real cepstrum, cut to its first
/// [`LIFTER`] coefficients and turned back. Doesn't allocate.
fn envelope(
    mag: &[f32],
    env: &mut [f32],
    cep: &mut [f32],
    spec: &mut [Complex32],
    r2c: &Arc<dyn RealToComplex<f32>>,
    c2r: &Arc<dyn ComplexToReal<f32>>,
    scratch: &mut [Complex32],
) {
    let peak = mag.iter().fold(0.0f32, |m, &x| m.max(x));
    // A floor well under the loudest bin keeps silent bins from pulling
    // the envelope down without end.
    let floor = (peak * 1e-5).max(1e-12);
    for (s, &m) in spec.iter_mut().zip(mag) {
        *s = Complex32::new(m.max(floor).ln(), 0.0);
    }
    c2r.process_with_scratch(spec, cep, scratch)
        .expect("inverse FFT sizes");
    let n = cep.len();
    // Keep the low quefrencies (both ends, the cepstrum being even),
    // tapering the last few so the envelope doesn't ring.
    for (i, x) in cep.iter_mut().enumerate() {
        let q = i.min(n - i);
        let w = if q < LIFTER / 2 {
            1.0
        } else if q < LIFTER {
            0.5 + 0.5 * (PI * (q - LIFTER / 2) as f32 / (LIFTER / 2) as f32).cos()
        } else {
            0.0
        };
        *x *= w / n as f32;
    }
    r2c.process_with_scratch(cep, spec, scratch)
        .expect("forward FFT sizes");
    for (e, s) in env.iter_mut().zip(spec.iter()) {
        *e = s.re;
    }
}

/// Plays all of `l` and `r` (the same length) at `speed` and transpose,
/// off the audio thread: `len / speed` frames.
pub fn whole(l: &[f32], r: &[f32], speed: f64, semitones: f64) -> (Vec<f32>, Vec<f32>) {
    let out = (l.len() as f64 / speed).round() as usize;
    let mut s = Stretcher::new(FRAME);
    s.set(speed, semitones);
    s.reset(0.0);
    let mut feed = Buffer { l, r };
    let (mut ol, mut or) = (vec![0.0; out], vec![0.0; out]);
    for (a, b) in ol.chunks_mut(FRAME).zip(or.chunks_mut(FRAME)) {
        s.process(&mut feed, a, b);
    }
    (ol, or)
}

/// Input held in memory; silence outside it.
struct Buffer<'a> {
    l: &'a [f32],
    r: &'a [f32],
}

impl Feed for Buffer<'_> {
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]) {
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let at = start + i as i64;
            (*a, *b) = match usize::try_from(at) {
                Ok(k) if k < self.l.len().min(self.r.len()) => (self.l[k], self.r[k]),
                _ => (0.0, 0.0),
            };
        }
    }
}
