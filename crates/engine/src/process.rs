// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline processing (roadmap 2.1): normalise, reverse or change the
//! gain of a file, written to a new file. The original is never changed,
//! so a clip pointed back at it (undo) sounds as it did.
//!
//! The file is read a chunk at a time, so a long clip doesn't have to fit
//! in memory. The new file is 32-bit float at the source's rate and
//! channel count, so turning a part up past full scale loses nothing
//! until the mix is played.

use std::ops::Range;
use std::path::Path;

use crate::stream::{CHUNK, StreamSource, read_samples};

/// The peak level [`Process::Normalise`] brings a clip to, in dB below
/// full scale: a little headroom, so it doesn't clip once it plays
/// through a converter.
pub const NORMALISE_PEAK_DB: f32 = -1.0;

/// What to do to the audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Process {
    /// Change the level so the loudest sample is [`NORMALISE_PEAK_DB`].
    Normalise,
    /// Play backwards.
    Reverse,
    /// Change the level by this many dB.
    Gain(f32),
}

impl Process {
    /// What was done, for naming the new clip: "normalised", "+3 dB".
    pub fn label(self) -> String {
        match self {
            Process::Normalise => "normalised".into(),
            Process::Reverse => "reversed".into(),
            Process::Gain(db) => format!("{db:+} dB"),
        }
    }
}

/// What [`process`] wrote.
#[derive(Clone, Copy, Debug)]
pub struct Processed {
    /// Highest sample level in the processed part; above 1.0 is past
    /// full scale.
    pub peak: f32,
}

fn gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Writes the whole of `source`, processed, to a new WAV file at `out`,
/// so a clip playing `part` of it can still be dragged longer. `part` is
/// what [`Process::Normalise`] measures. The file appears only when
/// complete: it is written beside `out` first and renamed. Can take a
/// moment for a long file, so call it off the UI thread.
pub fn process(
    source: &StreamSource,
    part: Range<usize>,
    how: Process,
    out: &Path,
) -> Result<Processed, String> {
    let src = |e: &dyn std::fmt::Display| format!("{}: {e}", source.path.display());
    if part.is_empty() || part.end > source.frames {
        return Err(src(&"that part is outside the file"));
    }
    let mut r = hound::WavReader::open(&source.path).map_err(|e| src(&e))?;
    let ch = source.channels;
    // Reads `n` frames from frame `at`.
    let mut read = |at: usize, n: usize| -> Result<Vec<f32>, String> {
        r.seek(at as u32).map_err(|e| src(&e))?;
        let s = read_samples(&mut r, n * ch).map_err(|e| src(&e))?;
        if s.len() < n * ch {
            return Err(src(&"the file is shorter than it says"));
        }
        Ok(s)
    };
    let mut part_peak = 0.0f32;
    for at in part.clone().step_by(CHUNK) {
        for x in read(at, CHUNK.min(part.end - at))? {
            part_peak = part_peak.max(x.abs());
        }
    }
    let scale = match how {
        Process::Normalise => {
            if part_peak == 0.0 {
                return Err("the clip is silent, so there is nothing to normalise".into());
            }
            gain(NORMALISE_PEAK_DB) / part_peak
        }
        Process::Reverse => 1.0,
        Process::Gain(db) => gain(db),
    };

    let spec = hound::WavSpec {
        channels: ch as u16,
        sample_rate: source.rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let dst = |e: &dyn std::fmt::Display| format!("could not write {}: {e}", out.display());
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| dst(&e))?;
    }
    let tmp = out.with_extension("wav.part");
    let mut w = hound::WavWriter::create(&tmp, spec).map_err(|e| dst(&e))?;
    let len = source.frames;
    let mut write = || -> Result<(), String> {
        for i in 0..len.div_ceil(CHUNK) {
            // Backwards, the last chunk comes first.
            let (at, n) = match how {
                Process::Reverse => {
                    let end = len - i * CHUNK;
                    (end.saturating_sub(CHUNK), end.min(CHUNK))
                }
                _ => (i * CHUNK, CHUNK.min(len - i * CHUNK)),
            };
            let mut s = read(at, n)?;
            if how == Process::Reverse {
                // Frame order reversed, each frame's channels kept in order.
                s.reverse();
                for f in s.chunks_exact_mut(ch) {
                    f.reverse();
                }
            }
            for x in s {
                w.write_sample(x * scale).map_err(|e| dst(&e))?;
            }
        }
        Ok(())
    };
    let written = write();
    let finished = w.finalize().map_err(|e| dst(&e));
    if let Err(e) = written.and(finished) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out).map_err(|e| dst(&e))?;
    Ok(Processed {
        peak: part_peak * scale,
    })
}

/// Where frame `at` of a file of `frames` frames is once reversed.
pub fn reversed(at: usize, frames: usize) -> usize {
    frames.saturating_sub(at + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("daw-process-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A 16-bit file of `frames` stereo frames: left counts up, right
    /// counts down.
    fn stereo(name: &str, frames: usize) -> std::sync::Arc<StreamSource> {
        let path = dir().join(name);
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..frames {
            let v = (i % 10_000) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(-v).unwrap();
        }
        w.finalize().unwrap();
        StreamSource::open(&path).unwrap()
    }

    fn read(path: &Path) -> (hound::WavSpec, Vec<f32>) {
        let r = hound::WavReader::open(path).unwrap();
        let spec = r.spec();
        (spec, r.into_samples::<f32>().map(Result::unwrap).collect())
    }

    const STEP: f32 = 1.0 / 32_768.0;

    #[test]
    fn reversing_plays_the_whole_file_backwards_across_chunks() {
        // More than two chunks, so the chunk order matters too.
        let frames = CHUNK * 2 + 500;
        let src = stereo("rev-in.wav", frames);
        let out = dir().join("rev-out.wav");
        process(&src, 300..1_300, Process::Reverse, &out).unwrap();
        let (spec, s) = read(&out);
        assert_eq!((spec.channels, spec.sample_rate), (2, 48_000));
        assert_eq!(s.len(), frames * 2);
        for (k, f) in s.as_chunks::<2>().0.iter().enumerate() {
            let v = (reversed(k, frames) % 10_000) as f32 * STEP;
            assert_eq!(*f, [v, -v], "frame {k}");
        }
        assert!(!out.with_extension("wav.part").exists());
    }

    #[test]
    fn normalising_brings_the_loudest_sample_of_the_part_to_the_target() {
        let src = stereo("norm-in.wav", 20_000);
        let out = dir().join("norm-out.wav");
        // Frames 0..5000: the loudest is 4999.
        let done = process(&src, 0..5_000, Process::Normalise, &out).unwrap();
        let (_, s) = read(&out);
        assert_eq!(s.len(), 40_000, "the whole file, for dragging longer");
        let target = gain(NORMALISE_PEAK_DB);
        let peak = s[..10_000].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - target).abs() < 1e-6, "{peak}");
        // Past the part, the same gain goes on.
        assert!((s[2 * 9_999] - target * 9_999.0 / 4_999.0).abs() < 1e-4);
        assert!((done.peak - target).abs() < 1e-6);
        assert!((s[2] - target / 4999.0).abs() < 1e-6);
    }

    #[test]
    fn gain_scales_every_sample_and_may_go_past_full_scale() {
        let src = stereo("gain-in.wav", 12_000);
        let out = dir().join("gain-out.wav");
        let done = process(&src, 9_000..10_000, Process::Gain(12.0), &out).unwrap();
        let (_, s) = read(&out);
        let k = gain(12.0);
        assert!((s[2 * 9_000] - 9_000.0 * STEP * k).abs() < 1e-5);
        assert!((s[2 * 9_000 + 1] + 9_000.0 * STEP * k).abs() < 1e-5);
        assert!(done.peak > 1.0);

        let quiet = dir().join("gain-quiet.wav");
        process(&src, 9_000..10_000, Process::Gain(-6.0), &quiet).unwrap();
        let (_, q) = read(&quiet);
        assert!((q[2 * 9_000] / s[2 * 9_000] - gain(-18.0)).abs() < 1e-5);
    }

    #[test]
    fn silence_cant_be_normalised_and_leaves_no_file() {
        let src = stereo("silent-in.wav", 100);
        let out = dir().join("silent-out.wav");
        // Frame 0 is silent on both sides.
        assert!(process(&src, 0..1, Process::Normalise, &out).is_err());
        assert!(process(&src, 50..150, Process::Reverse, &out).is_err());
        assert!(!out.exists());
    }

    #[test]
    fn labels_say_what_was_done() {
        assert_eq!(Process::Gain(3.0).label(), "+3 dB");
        assert_eq!(Process::Gain(-6.0).label(), "-6 dB");
        assert_eq!(Process::Reverse.label(), "reversed");
    }
}
