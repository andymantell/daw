// SPDX-License-Identifier: GPL-3.0-or-later
//! Impulse responses read from sound files, for the convolution reverb:
//! the user's own, and the recorded rooms that come with the app
//! ([`crate::library`]).

use std::io::Cursor;
use std::path::Path;

/// The longest response kept, in seconds; longer ones are cut, faded.
pub const MAX_SECONDS: f64 = 10.0;

/// A room's response, left and right, ready to convolve with.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl Response {
    /// From a sound file: WAV with 1, 2 or 4 channels (4 is first-order
    /// B-format, in W, X, Y, Z order), or any other file the app can
    /// play, in mono or stereo.
    pub fn from_file(path: &Path) -> Result<Response, String> {
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        if let Ok(r) = hound::WavReader::open(path) {
            return Response::from_wav(r).map_err(|e| err(&e));
        }
        let s = daw_engine::import::read(path)?;
        let chans = deinterleave(&s.samples, s.channels);
        Response::from_channels(s.rate, chans).map_err(|e| err(&e))
    }

    /// From a WAV file's bytes.
    pub fn from_wav_bytes(bytes: &[u8]) -> Result<Response, String> {
        let r = hound::WavReader::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        Response::from_wav(r)
    }

    fn from_wav<R: std::io::Read>(mut r: hound::WavReader<R>) -> Result<Response, String> {
        let spec = r.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                r.samples::<i32>()
                    .map(|s| s.map(|v| v as f32 * scale))
                    .collect::<Result<_, _>>()
            }
        }
        .map_err(|e| e.to_string())?;
        let chans = deinterleave(&samples, spec.channels as usize);
        Response::from_channels(spec.sample_rate, chans)
    }

    /// From the file's channels: made stereo, trimmed of silence before
    /// and after, cut to [`MAX_SECONDS`], and brought to a standard level.
    pub fn from_channels(rate: u32, chans: Vec<Vec<f32>>) -> Result<Response, String> {
        if rate == 0 {
            return Err("the file has no sample rate".into());
        }
        let [mut left, mut right] = match chans.len() {
            1 => [chans[0].clone(), chans[0].clone()],
            2 => {
                let mut c = chans.into_iter();
                [c.next().unwrap_or_default(), c.next().unwrap_or_default()]
            }
            4 => b_format_to_stereo(&chans),
            n => {
                return Err(format!(
                    "it has {n} channels; a response needs 1, 2 or 4 (B-format)"
                ));
            }
        };
        let peak = left.iter().chain(&right).fold(0f32, |m, x| m.max(x.abs()));
        if peak <= 0.0 || !peak.is_finite() {
            return Err("the file is silent".into());
        }
        // Silence before the sound starts would only delay the reverb;
        // a little is kept so the start isn't cut.
        let quiet = |x: &f32| x.abs() < peak * 0.001;
        let first = (0..left.len())
            .find(|&i| !quiet(&left[i]) || !quiet(&right[i]))
            .unwrap_or(0);
        let first = first.saturating_sub((rate / 1000) as usize);
        let tail = peak * 1e-4;
        let last = (0..left.len())
            .rev()
            .find(|&i| left[i].abs() > tail || right[i].abs() > tail)
            .map_or(0, |i| i + 1);
        let end = last.min(first + (MAX_SECONDS * rate as f64) as usize);
        left = left[first..end].to_vec();
        right = right[first..end].to_vec();
        // A short fade, so where it was cut isn't heard.
        let fade = (left.len() / 20).min(rate as usize / 10);
        let n = left.len();
        for i in 0..fade {
            let g = i as f32 / fade as f32;
            left[n - 1 - i] *= g;
            right[n - 1 - i] *= g;
        }
        // About half the input's energy in each channel, as the rooms made
        // here have.
        let energy: f64 = (left.iter().chain(&right))
            .map(|x| (*x as f64).powi(2))
            .sum::<f64>()
            / 2.0;
        let scale = (0.5 / energy.max(1e-20)).sqrt() as f32;
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x *= scale;
        }
        Ok(Response { rate, left, right })
    }

    /// Its length in seconds.
    pub fn seconds(&self) -> f64 {
        self.left.len() as f64 / self.rate as f64
    }

    /// The same response at `rate`.
    pub fn resampled(&self, rate: u32) -> Response {
        let [left, right] = self.at_rate(rate);
        Response { rate, left, right }
    }

    /// Left and right at `rate`.
    pub(crate) fn at_rate(&self, rate: u32) -> [Vec<f32>; 2] {
        if rate == self.rate {
            return [self.left.clone(), self.right.clone()];
        }
        let inter: Vec<f32> = (self.left.iter().zip(&self.right))
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        match daw_engine::import::resample(&inter, 2, self.rate, rate) {
            Ok(v) => {
                let mut c = deinterleave(&v, 2).into_iter();
                [c.next().unwrap_or_default(), c.next().unwrap_or_default()]
            }
            Err(_) => [self.left.clone(), self.right.clone()],
        }
    }

    /// As a 16-bit stereo WAV file, as the app's own rooms are kept.
    pub fn to_wav(&self, path: &Path) -> Result<(), String> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: self.rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let peak = (self.left.iter().chain(&self.right)).fold(0f32, |m, x| m.max(x.abs()));
        // Kept just under full scale; the level is set again on reading.
        let scale = 32_000.0 / peak.max(1e-9);
        let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
        for (l, r) in self.left.iter().zip(&self.right) {
            for x in [l, r] {
                w.write_sample((x * scale).round() as i16)
                    .map_err(|e| e.to_string())?;
            }
        }
        w.finalize().map_err(|e| e.to_string())
    }
}

fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    let channels = channels.max(1);
    (0..channels)
        .map(|c| samples.iter().skip(c).step_by(channels).copied().collect())
        .collect()
}

/// First-order B-format (W, X, Y, Z, with W 3 dB down) heard by two
/// cardioid microphones at the same point, angled 45° left and right.
fn b_format_to_stereo(c: &[Vec<f32>]) -> [Vec<f32>; 2] {
    let n = c.iter().map(Vec::len).min().unwrap_or(0);
    let a = std::f32::consts::FRAC_PI_4;
    let mic = |side: f32| -> Vec<f32> {
        (0..n)
            .map(|i| {
                0.5 * (std::f32::consts::SQRT_2 * c[0][i]
                    + a.cos() * c[1][i]
                    + side * a.sin() * c[2][i])
            })
            .collect()
    };
    [mic(1.0), mic(-1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decay(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|i| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((s >> 8) as f32 / (1 << 23) as f32 - 1.0) * (-(i as f32) / 4800.0).exp()
            })
            .collect()
    }

    #[test]
    fn silence_is_trimmed_and_the_level_set() {
        let mut ch = vec![0.0; 2400];
        ch.extend(decay(48_000, 3));
        ch.extend(vec![0.0; 48_000]);
        let r = Response::from_channels(48_000, vec![ch.clone(), ch]).unwrap();
        // 50 ms of silence before, less the millisecond kept.
        assert!(r.left[..40].iter().all(|x| *x == 0.0));
        assert!(r.left[40..200].iter().any(|x| *x != 0.0));
        // The silence after is gone.
        assert!(r.left.len() < 50_000);
        let e: f64 = r.left.iter().map(|x| (*x as f64).powi(2)).sum();
        assert!((e - 0.5).abs() < 1e-3);
    }

    #[test]
    fn b_format_from_the_left_is_louder_on_the_left() {
        let w = decay(4800, 5);
        // A sound arriving from the left: X 0, Y positive.
        let y: Vec<f32> = w.iter().map(|v| v * std::f32::consts::SQRT_2).collect();
        let r =
            Response::from_channels(48_000, vec![w.clone(), vec![0.0; 4800], y, vec![0.0; 4800]])
                .unwrap();
        let e = |v: &[f32]| v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
        assert!(e(&r.left) > 10.0 * e(&r.right));
    }

    #[test]
    fn long_responses_are_cut() {
        let ch = decay(48_000 * 12, 7)
            .iter()
            .map(|x| x.signum() * 0.1)
            .collect::<Vec<_>>();
        let r = Response::from_channels(48_000, vec![ch]).unwrap();
        assert_eq!(r.left.len(), 48_000 * 10);
        assert!(r.left.last().unwrap().abs() < 1e-6);
    }

    #[test]
    fn silent_or_odd_files_are_refused() {
        assert!(Response::from_channels(48_000, vec![vec![0.0; 100]]).is_err());
        assert!(Response::from_channels(48_000, vec![vec![1.0; 100]; 3]).is_err());
    }

    #[test]
    fn written_and_read_back_the_same() {
        let mut a = decay(9000, 1);
        a[0] = 1.0;
        let r = Response::from_channels(44_100, vec![a, decay(9000, 2)]).unwrap();
        let path = std::env::temp_dir().join(format!("daw-ir-{}.wav", std::process::id()));
        r.to_wav(&path).unwrap();
        let back = Response::from_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.rate, 44_100);
        // The faded end may be trimmed again by a frame or two.
        assert!(r.left.len() - back.left.len() < 8);
        let worst =
            (r.left.iter().zip(&back.left).take(8000)).fold(0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-3, "{worst}");
        let at = back.at_rate(48_000);
        assert_eq!(at[0].len(), (back.left.len() * 48_000).div_ceil(44_100));
    }
}
