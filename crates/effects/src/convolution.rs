// SPDX-License-Identifier: GPL-3.0-or-later
//! A convolution reverb: the input is convolved with a room's impulse
//! response (its echo pattern from a single click).
//!
//! The first block of the response is convolved directly, sample by
//! sample, and the rest by uniformly partitioned overlap-save convolution
//! in the frequency domain (as described, for example, by F. Wefers,
//! "Partitioned convolution algorithms for real-time auralization", 2015).
//! The frequency-domain part answers a block late, which is exactly where
//! its share of the response starts, so the reverb adds no delay.
//!
//! The responses are made here, from decaying noise that darkens as it
//! dies away, with a few early reflections: no recordings are shipped.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::{Dsp, Param, Unit};

pub const ROOMS: &[&str] = &["Small room", "Studio", "Hall", "Plate"];

pub const PARAMS: &[Param] = &[
    Param::new(
        "room",
        "Room",
        0.0,
        (ROOMS.len() - 1) as f64,
        1.0,
        Unit::Choice,
    )
    .choices(ROOMS),
    Param::new("predelay", "Pre-delay", 0.0, 200.0, 0.0, Unit::Ms),
    Param::new("mix", "Mix", 0.0, 100.0, 25.0, Unit::Percent),
];

/// Frames in a block: the part convolved directly, and each partition.
const BLOCK: usize = 256;
const MAX_PREDELAY_SECONDS: f64 = 0.2;

/// How a made-up room sounds.
struct Shape {
    /// Seconds to die away by 60 dB.
    decay: f64,
    /// The tail's brightness as it starts and as it ends, in Hz.
    bright: (f64, f64),
    /// When the first reflections arrive, in ms; the dense tail builds
    /// up over them.
    early_ms: &'static [f64],
    seed: u32,
}

const SHAPES: [Shape; 4] = [
    Shape {
        decay: 0.45,
        bright: (9000.0, 2500.0),
        early_ms: &[3.1, 5.3, 7.9, 11.2, 14.8, 19.5],
        seed: 11,
    },
    Shape {
        decay: 0.8,
        bright: (10000.0, 3000.0),
        early_ms: &[5.7, 9.1, 13.3, 17.9, 23.4, 29.0],
        seed: 23,
    },
    Shape {
        decay: 2.2,
        bright: (8000.0, 1500.0),
        early_ms: &[12.5, 19.7, 27.1, 33.8, 42.3, 51.6],
        seed: 37,
    },
    Shape {
        decay: 1.6,
        bright: (14000.0, 6000.0),
        early_ms: &[],
        seed: 41,
    },
];

/// The impulse response of room `room`, left and right, at `rate`.
pub(crate) fn response(room: usize, rate: f64) -> [Vec<f32>; 2] {
    let s = &SHAPES[room.min(SHAPES.len() - 1)];
    let len = ((s.decay * 1.25).min(3.0) * rate) as usize;
    let onset = s.early_ms.last().copied().unwrap_or(1.0) / 1000.0;
    std::array::from_fn(|ch| {
        let mut seed = s
            .seed
            .wrapping_mul(2654435761)
            .wrapping_add(ch as u32 * 7919);
        let mut noise = move || {
            // xorshift32; the sum of two is near enough to normal.
            let mut one = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f64 / u32::MAX as f64 - 0.5
            };
            one() + one()
        };
        let mut h = vec![0f32; len];
        let mut low = 0f64;
        for (n, v) in h.iter_mut().enumerate() {
            let t = n as f64 / rate;
            let env = (-6.9078 * t / s.decay).exp() * (t / onset).min(1.0);
            let k = (t / s.decay).min(1.0);
            let hz = s.bright.0 * (s.bright.1 / s.bright.0).powf(k);
            let a = (-2.0 * std::f64::consts::PI * hz / rate).exp();
            low = noise() * (1.0 - a) + low * a;
            // The filter takes away level as it darkens; give it back.
            *v = (low * env * ((1.0 + a) / (1.0 - a)).sqrt()) as f32;
        }
        for (k, &ms) in s.early_ms.iter().enumerate() {
            // Left and right hear each reflection a little apart.
            let at = ((ms + ch as f64 * 0.7 * (k % 3) as f64) / 1000.0 * rate) as usize;
            if let Some(v) = h.get_mut(at) {
                let sign = if (k + ch) % 2 == 0 { 1.0 } else { -1.0 };
                *v += sign * 0.85f32.powi(k as i32);
            }
        }
        // A short fade, so the end of the response isn't heard.
        let fade = len / 10;
        for i in 0..fade {
            h[len - 1 - i] *= i as f32 / fade as f32;
        }
        // About half the input's energy in each channel.
        let energy: f64 = h.iter().map(|x| (*x as f64).powi(2)).sum();
        let scale = (0.5 / energy.max(1e-12)).sqrt() as f32;
        for v in &mut h {
            *v *= scale;
        }
        h
    })
}

/// A room's response, ready to convolve with.
struct Room {
    /// The first block, reversed, per channel.
    head: [Vec<f32>; 2],
    /// The spectrum of each later block, per channel.
    parts: [Vec<Vec<Complex32>>; 2],
}

impl Room {
    fn new(h: &[Vec<f32>; 2], fft: &dyn RealToComplex<f32>) -> Room {
        let mut scratch = fft.make_scratch_vec();
        let mut work = vec![0f32; 2 * BLOCK];
        let mut head: [Vec<f32>; 2] = Default::default();
        let mut parts: [Vec<Vec<Complex32>>; 2] = Default::default();
        for ch in 0..2 {
            let mut first: Vec<f32> = h[ch].iter().take(BLOCK).copied().collect();
            first.resize(BLOCK, 0.0);
            first.reverse();
            head[ch] = first;
            for chunk in h[ch].get(BLOCK..).unwrap_or(&[]).chunks(BLOCK) {
                work.fill(0.0);
                work[..chunk.len()].copy_from_slice(chunk);
                let mut spec = fft.make_output_vec();
                let _ = fft.process_with_scratch(&mut work, &mut spec, &mut scratch);
                parts[ch].push(spec);
            }
        }
        Room { head, parts }
    }
}

pub struct Convolution {
    rooms: Vec<Room>,
    room: usize,
    want: usize,
    /// The room faded from over the current block, if changing.
    fading: Option<usize>,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    fft_scratch: Vec<Complex32>,
    ifft_scratch: Vec<Complex32>,
    rate: f64,
    predelay: Vec<f32>,
    pre_at: usize,
    pre_frames: f64,
    /// The input, twice over, for direct convolution without wrapping.
    hist: Vec<f32>,
    /// The last two blocks of input.
    window: Vec<f32>,
    work: Vec<f32>,
    /// Spectra of past input windows, newest at `fdl_at`.
    fdl: Vec<Vec<Complex32>>,
    fdl_at: usize,
    acc: Vec<Complex32>,
    /// The frequency-domain part's output for this block, per channel,
    /// for the room now and the room being faded from.
    tail: [Vec<f32>; 2],
    tail_old: [Vec<f32>; 2],
    pos: usize,
    wet: f32,
    /// Has had its first settings, so changes of room fade.
    started: bool,
}

impl Convolution {
    pub fn new(rate: f64) -> Convolution {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(2 * BLOCK);
        let ifft = planner.plan_fft_inverse(2 * BLOCK);
        let rooms: Vec<Room> = (0..ROOMS.len())
            .map(|r| Room::new(&response(r, rate), fft.as_ref()))
            .collect();
        let most = rooms
            .iter()
            .map(|r| r.parts[0].len())
            .max()
            .unwrap_or(0)
            .max(1);
        Convolution {
            room: 0,
            want: 0,
            fading: None,
            fft_scratch: fft.make_scratch_vec(),
            ifft_scratch: ifft.make_scratch_vec(),
            fdl: (0..most).map(|_| fft.make_output_vec()).collect(),
            acc: fft.make_output_vec(),
            fft,
            ifft,
            rooms,
            rate,
            predelay: vec![0.0; (MAX_PREDELAY_SECONDS * rate) as usize + 4],
            pre_at: 0,
            pre_frames: 0.0,
            hist: vec![0.0; 2 * BLOCK],
            window: vec![0.0; 2 * BLOCK],
            work: vec![0.0; 2 * BLOCK],
            fdl_at: 0,
            tail: [vec![0.0; BLOCK], vec![0.0; BLOCK]],
            tail_old: [vec![0.0; BLOCK], vec![0.0; BLOCK]],
            pos: 0,
            wet: 0.0,
            started: false,
        }
    }

    /// `room`'s frequency-domain output for the next block, into `out`.
    fn tail_into(&mut self, room: usize, old: bool) {
        let n = self.fdl.len();
        for ch in 0..2 {
            self.acc.fill(Complex32::new(0.0, 0.0));
            for (p, h) in self.rooms[room].parts[ch].iter().enumerate() {
                let x = &self.fdl[(self.fdl_at + n - p) % n];
                for ((a, x), h) in self.acc.iter_mut().zip(x).zip(h) {
                    *a += x * h;
                }
            }
            // The ends of a real signal's spectrum are real.
            self.acc[0].im = 0.0;
            if let Some(last) = self.acc.last_mut() {
                last.im = 0.0;
            }
            let _ = self.ifft.process_with_scratch(
                &mut self.acc,
                &mut self.work,
                &mut self.ifft_scratch,
            );
            let scale = 1.0 / (2 * BLOCK) as f32;
            let out = if old {
                &mut self.tail_old[ch]
            } else {
                &mut self.tail[ch]
            };
            for (o, w) in out.iter_mut().zip(&self.work[BLOCK..]) {
                *o = w * scale;
            }
        }
    }

    /// Takes in a whole block of input.
    fn block(&mut self) {
        self.work.copy_from_slice(&self.window);
        let n = self.fdl.len();
        self.fdl_at = (self.fdl_at + 1) % n;
        let _ = self.fft.process_with_scratch(
            &mut self.work,
            &mut self.fdl[self.fdl_at],
            &mut self.fft_scratch,
        );
        self.window.copy_within(BLOCK.., 0);
        self.fading = None;
        if self.want != self.room {
            self.fading = Some(self.room);
            self.room = self.want;
        }
        if let Some(old) = self.fading {
            self.tail_into(old, true);
        }
        self.tail_into(self.room, false);
    }

    /// The direct part of `room`'s response for channel `ch`, at the
    /// newest input.
    #[inline]
    fn head(&self, room: usize, ch: usize) -> f32 {
        let h = &self.rooms[room].head[ch];
        let x = &self.hist[self.pos + 1..self.pos + 1 + BLOCK];
        let mut lanes = [0f32; 8];
        for (hc, xc) in h.as_chunks::<8>().0.iter().zip(x.as_chunks::<8>().0) {
            for k in 0..8 {
                lanes[k] += hc[k] * xc[k];
            }
        }
        lanes.iter().sum()
    }
}

impl Dsp for Convolution {
    fn update(&mut self, v: &[f64]) {
        self.want = (v[0] as usize).min(self.rooms.len() - 1);
        if !self.started {
            self.room = self.want;
            self.started = true;
        }
        self.pre_frames = (v[1] / 1000.0 * self.rate).min(self.predelay.len() as f64 - 3.0);
        self.wet = (v[2] / 100.0) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let pn = self.predelay.len();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.predelay[self.pre_at] = (*l + *r) * 0.5;
            let x = crate::delay_read(&self.predelay, self.pre_at, self.pre_frames);
            self.pre_at = (self.pre_at + 1) % pn;
            let p = self.pos;
            self.hist[p] = x;
            self.hist[p + BLOCK] = x;
            self.window[BLOCK + p] = x;
            let mut wet = [0f32; 2];
            for (ch, w) in wet.iter_mut().enumerate() {
                let now = self.head(self.room, ch) + self.tail[ch][p];
                *w = match self.fading {
                    Some(old) => {
                        let was = self.head(old, ch) + self.tail_old[ch][p];
                        let a = p as f32 / BLOCK as f32;
                        was * (1.0 - a) + now * a
                    }
                    None => now,
                };
            }
            *l = *l * (1.0 - self.wet) + wet[0] * self.wet;
            *r = *r * (1.0 - self.wet) + wet[1] * self.wet;
            self.pos += 1;
            if self.pos == BLOCK {
                self.pos = 0;
                self.block();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;

    /// Feeding a click through all wet gives back the room's response,
    /// with no delay, in blocks of any size.
    #[test]
    fn a_click_gives_back_the_response() {
        for room in 0..ROOMS.len() {
            let h = response(room, 48_000.0);
            let e = Effect::new(Kind::Convolution);
            e.set(0, room as f64);
            e.set(2, 100.0);
            let mut p = e.processor(48_000);
            let len = h[0].len() + 1000;
            let mut l = vec![0f32; len];
            l[0] = 1.0;
            let mut r = l.clone();
            let mut at = 0;
            for size in [1usize, 7, 64, 300, 1024].iter().cycle() {
                if at >= len {
                    break;
                }
                let end = (at + size).min(len);
                p.process(&mut l[at..end], &mut r[at..end]);
                at = end;
            }
            // The input is the mono sum of the two channels, so the
            // click goes in at full level.
            let err = (0..h[0].len())
                .map(|i| (l[i] - h[0][i]).abs().max((r[i] - h[1][i]).abs()))
                .fold(0f32, f32::max);
            assert!(err < 1e-4, "room {room}: {err}");
            assert!(l[h[0].len()..].iter().all(|x| x.abs() < 1e-4));
        }
    }

    #[test]
    fn responses_die_away_and_have_the_energy_asked_for() {
        for room in 0..ROOMS.len() {
            let h = response(room, 48_000.0);
            for ch in &h {
                let e: f64 = ch.iter().map(|x| (*x as f64).powi(2)).sum();
                assert!((e - 0.5).abs() < 1e-3);
                let tail = ch[ch.len() - 100..]
                    .iter()
                    .fold(0f32, |m, x| m.max(x.abs()));
                assert!(tail < 1e-3);
            }
        }
    }

    #[test]
    fn changing_room_fades_without_a_jump() {
        let e = Effect::new(Kind::Convolution);
        e.set(2, 100.0);
        let mut p = e.processor(48_000);
        let mut prev = 0f32;
        let mut jump = 0f32;
        for block in 0..400 {
            if block == 200 {
                e.set(0, 2.0);
            }
            let mut l: Vec<f32> = (0..128)
                .map(|i| ((block * 128 + i) as f32 * 0.05).sin() * 0.5)
                .collect();
            let mut r = l.clone();
            p.process(&mut l, &mut r);
            for x in &l {
                jump = jump.max((x - prev).abs());
                prev = *x;
            }
        }
        assert!(jump < 0.2, "{jump}");
    }
}
