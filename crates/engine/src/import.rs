// SPDX-License-Identifier: GPL-3.0-or-later
//! Importing audio files at another sample rate (roadmap 1.1).
//!
//! The engine plays clips at the session's rate only. A file at any other
//! rate is converted once, off the audio thread, into a 32-bit float WAV
//! file in a cache folder, and that copy is what streams. The original is
//! never changed. Converting again with the same file and rate finds the
//! copy already there.
//!
//! The conversion is a fixed-ratio FFT resampler (the `rubato` crate),
//! with its delay taken off, so the audio starts where it did, to within
//! one frame: a clip placed on a beat still sounds on that beat.

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use crate::stream::{StreamSource, read_samples};

/// Opens `path` for streaming at `rate`, converting it into `cache` first
/// if it is at another rate. Can take a few seconds for a long file, so
/// call it off the UI thread.
pub fn open_at_rate(path: &Path, rate: u32, cache: &Path) -> Result<Arc<StreamSource>, String> {
    let src = StreamSource::open(path)?;
    if src.rate == rate {
        return Ok(src);
    }
    let converted = convert(path, rate, cache)?;
    StreamSource::open(&converted)
}

/// The cached copy of `path` at `rate`, made now if it is not already
/// there. Returns its path.
pub fn convert(path: &Path, rate: u32, cache: &Path) -> Result<PathBuf, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let out = cache_path(path, rate, cache).map_err(|e| err(&e))?;
    if out.exists() {
        return Ok(out);
    }
    let mut r = hound::WavReader::new(BufReader::new(File::open(path).map_err(|e| err(&e))?))
        .map_err(|e| err(&e))?;
    let spec = r.spec();
    let channels = spec.channels as usize;
    let frames = r.duration() as usize;
    let input = read_samples(&mut r, frames * channels).map_err(|e| err(&e))?;
    let output = resample(&input, channels, spec.sample_rate, rate).map_err(|e| err(&e))?;

    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    // Written under a temporary name and renamed when complete, so an
    // interrupted conversion never leaves a short file that looks done.
    let part = out.with_extension("part");
    let mut w = hound::WavWriter::create(
        &part,
        hound::WavSpec {
            channels: spec.channels,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .map_err(|e| err(&e))?;
    for s in output {
        w.write_sample(s).map_err(|e| err(&e))?;
    }
    w.finalize().map_err(|e| err(&e))?;
    std::fs::rename(&part, &out).map_err(|e| err(&e))?;
    Ok(out)
}

/// Converts interleaved samples from one rate to another. The result has
/// `frames * to / from` frames, rounded up, with no added delay.
pub fn resample(input: &[f32], channels: usize, from: u32, to: u32) -> Result<Vec<f32>, String> {
    let frames = input.len() / channels;
    if frames == 0 {
        return Ok(Vec::new());
    }
    let mut rs = Fft::<f32>::new(from as usize, to as usize, 1024, channels, FixedSync::Both)
        .map_err(|e| e.to_string())?;
    let adapter = InterleavedSlice::new(input, channels, frames).map_err(|e| e.to_string())?;
    let out = rs
        .process_all(&adapter, frames, None)
        .map_err(|e| e.to_string())?;
    // Exactly `frames * to / from`, rounded up: the resampler's own count
    // works in floating point and can come out a frame long.
    let len = (frames as u64 * to as u64).div_ceil(from as u64) as usize;
    let mut data = out.take_data();
    data.truncate(len * channels);
    Ok(data)
}

/// Where the copy of `path` at `rate` goes: named after the file, with a
/// hash of its full path, size and modification time, so a changed file
/// gets a fresh copy.
fn cache_path(path: &Path, rate: u32, cache: &Path) -> std::io::Result<PathBuf> {
    let full = std::fs::canonicalize(path)?;
    let meta = std::fs::metadata(&full)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    full.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    let stem = path
        .file_stem()
        .map_or("audio".into(), |s| s.to_string_lossy());
    Ok(cache.join(format!("{stem}-{:016x}-{rate}.wav", h.finish())))
}
