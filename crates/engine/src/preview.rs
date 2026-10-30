// SPDX-License-Identifier: GPL-3.0-or-later
//! Hearing a sound from the content browser (roadmap 3.6) before it is
//! in the project: one preview voice, mixed into the main output after
//! the master fader, whether the transport plays or not.
//!
//! The sound is made ready off the audio thread by [`prepare`]: decoded,
//! converted to the device's rate and, to hear a loop at the project's
//! tempo, stretched with the practice stretcher (pitch kept). The engine
//! then only copies samples. A finished or replaced sound goes back to
//! the controller to be freed (see [`crate::engine::Controller::collect_garbage`]).

use crate::import::{self, Sound};
use crate::stretch::{FRAME, Feed, SPEED_RANGE, Stretcher};

/// A sound ready to play: stereo, at the device's rate.
pub struct PreviewSound {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Linear gain.
    pub gain: f32,
    /// Plays round again from the start until stopped.
    pub looped: bool,
}

impl PreviewSound {
    pub fn len(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The longest preview made, in seconds: enough to judge a sound, and
/// quick to stretch.
pub const MAX_SECONDS: f64 = 30.0;

/// Makes `sound` ready to preview on a device at `rate`, at `speed`
/// (1 is as recorded; kept within the stretcher's range). Only the first
/// [`MAX_SECONDS`] of the result are kept.
pub fn prepare(sound: &Sound, rate: u32, speed: f64) -> Result<PreviewSound, String> {
    let ch = sound.channels.max(1);
    let speed = speed.clamp(SPEED_RANGE.0, SPEED_RANGE.1);
    // Enough input for the longest output kept.
    let in_frames = sound.samples.len() / ch;
    let want_in = ((MAX_SECONDS * speed + 1.0) * sound.rate as f64).ceil() as usize;
    let samples = &sound.samples[..in_frames.min(want_in) * ch];
    let at_rate = if sound.rate == rate {
        samples.to_vec()
    } else {
        import::resample(samples, ch, sound.rate, rate)?
    };
    let frames = at_rate.len() / ch;
    let (mut l, mut r): (Vec<f32>, Vec<f32>) = (0..frames)
        .map(|i| (at_rate[i * ch], at_rate[i * ch + ch - 1]))
        .unzip();
    if (speed - 1.0).abs() > 1e-6 {
        (l, r) = stretch(&l, &r, speed);
    }
    let keep = (MAX_SECONDS * rate as f64) as usize;
    l.truncate(keep);
    r.truncate(keep);
    Ok(PreviewSound {
        left: l,
        right: r,
        gain: 1.0,
        looped: false,
    })
}

struct Buffer<'a> {
    l: &'a [f32],
    r: &'a [f32],
}

impl Feed for Buffer<'_> {
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]) {
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let at = start + i as i64;
            (*a, *b) = match usize::try_from(at) {
                Ok(k) if k < self.l.len() => (self.l[k], self.r[k]),
                _ => (0.0, 0.0),
            };
        }
    }
}

/// Plays `l` and `r` at `speed`, pitch kept: `len / speed` frames.
fn stretch(l: &[f32], r: &[f32], speed: f64) -> (Vec<f32>, Vec<f32>) {
    let out = (l.len() as f64 / speed).round() as usize;
    let mut s = Stretcher::new(FRAME);
    s.set(speed, 0.0);
    s.reset(0.0);
    let mut feed = Buffer { l, r };
    let (mut ol, mut or) = (vec![0.0; out], vec![0.0; out]);
    for (a, b) in ol.chunks_mut(FRAME).zip(or.chunks_mut(FRAME)) {
        s.process(&mut feed, a, b);
    }
    (ol, or)
}

/// The engine's side: the sound playing, and where.
#[derive(Default)]
pub(crate) struct Voice {
    pub(crate) sound: Option<Box<PreviewSound>>,
    pub(crate) pos: usize,
}

impl Voice {
    /// Adds the next frames into `left` and `right`. Returns true when
    /// a sound that does not loop has just finished.
    pub(crate) fn mix(&mut self, left: &mut [f32], right: &mut [f32]) -> bool {
        let Some(s) = &self.sound else { return false };
        let len = s.len();
        let mut o = 0;
        let n = left.len().min(right.len());
        while o < n && len > 0 {
            if self.pos >= len {
                if !s.looped {
                    return true;
                }
                self.pos = 0;
            }
            let run = (n - o).min(len - self.pos);
            let (sl, sr) = (&s.left[self.pos..][..run], &s.right[self.pos..][..run]);
            for i in 0..run {
                left[o + i] += sl[i] * s.gain;
                right[o + i] += sr[i] * s.gain;
            }
            o += run;
            self.pos += run;
        }
        len == 0 || (!s.looped && self.pos >= len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f64) -> Sound {
        let n = (rate as f64 * secs) as usize;
        Sound {
            samples: (0..n)
                .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5)
                .collect(),
            channels: 1,
            rate,
        }
    }

    #[test]
    fn converts_rate_and_length() {
        let p = prepare(&tone(44_100, 1.0), 48_000, 1.0).unwrap();
        assert!((p.len() as i64 - 48_000).abs() < 8, "{}", p.len());
        // Mono plays on both sides.
        assert_eq!(p.left[1000], p.right[1000]);
    }

    #[test]
    fn stretches_to_the_speed() {
        // A 2 s loop sped up by 1.25 lasts 1.6 s.
        let p = prepare(&tone(48_000, 2.0), 48_000, 1.25).unwrap();
        assert_eq!(p.len(), 76_800);
        // And still sounds: not silence.
        let rms = (p.left[10_000..60_000].iter().map(|x| x * x).sum::<f32>() / 50_000.0).sqrt();
        assert!(rms > 0.2, "{rms}");
    }

    #[test]
    fn keeps_only_the_start_of_long_sounds() {
        let p = prepare(&tone(8_000, MAX_SECONDS + 5.0), 8_000, 1.0).unwrap();
        assert_eq!(p.len(), (MAX_SECONDS * 8_000.0) as usize);
    }

    #[test]
    fn voice_plays_once_or_loops() {
        let sound = |looped| PreviewSound {
            left: vec![1.0; 5],
            right: vec![0.5; 5],
            gain: 1.0,
            looped,
        };
        let mut v = Voice {
            sound: Some(Box::new(sound(false))),
            pos: 0,
        };
        let (mut l, mut r) = (vec![0.0; 8], vec![0.0; 8]);
        assert!(v.mix(&mut l, &mut r));
        assert_eq!(l, [1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(r[4], 0.5);
        let mut v = Voice {
            sound: Some(Box::new(sound(true))),
            pos: 0,
        };
        let (mut l, mut r) = (vec![0.0; 8], vec![0.0; 8]);
        assert!(!v.mix(&mut l, &mut r));
        assert_eq!(l, [1.0; 8]);
        assert_eq!(v.pos, 3);
    }
}
