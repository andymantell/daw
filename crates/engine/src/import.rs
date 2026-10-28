// SPDX-License-Identifier: GPL-3.0-or-later
//! Importing audio files (roadmap 1.1, 1.5): WAV at any sample rate, and
//! MP3, FLAC, OGG Vorbis, AAC/M4A, AIFF, ALAC and CAF, or the sound of an
//! MP4, MOV or MKV video.
//!
//! The engine streams WAV clips at the session's rate only. Any other file
//! is decoded (with the `symphonia` crate) and converted once, off the
//! audio thread, into a 32-bit float WAV file in a cache folder, and that
//! copy is what streams. The original is never changed. Converting again
//! with the same file and rate finds the copy already there. More than two
//! channels are mixed down to stereo.
//!
//! A video's sound is first taken out into a file of its own
//! ([`soundtrack`]), so a project keeps only the audio, not the picture.
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
use symphonia::core::audio::{Channels, Position};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioDecoderOptions, well_known::CODEC_ID_AAC};
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::stream::{StreamSource, read_samples};

/// Opens `path` for streaming at `rate`, converting it into `cache` first
/// if it is at another rate. Can take a few seconds for a long file, so
/// call it off the UI thread.
pub fn open_at_rate(path: &Path, rate: u32, cache: &Path) -> Result<Arc<StreamSource>, String> {
    if let Ok(src) = StreamSource::open(path)
        && src.rate == rate
    {
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
    let sound = read(path)?;
    let output = if sound.rate == rate {
        sound.samples
    } else {
        resample(&sound.samples, sound.channels, sound.rate, rate).map_err(|e| err(&e))?
    };

    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    // Written under a temporary name and renamed when complete, so an
    // interrupted conversion never leaves a short file that looks done.
    let part = out.with_extension("part");
    let mut w = hound::WavWriter::create(
        &part,
        hound::WavSpec {
            channels: sound.channels as u16,
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

/// A whole file's sound, decoded.
pub struct Sound {
    /// Interleaved, in -1.0..1.0.
    pub samples: Vec<f32>,
    /// 1 or 2.
    pub channels: usize,
    pub rate: u32,
}

/// Every sample of `path`, mono or stereo: a WAV file read directly, any
/// other file decoded.
pub fn read(path: &Path) -> Result<Sound, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    if let Ok(mut r) = hound::WavReader::new(BufReader::new(File::open(path).map_err(|e| err(&e))?))
        && r.spec().channels <= 2
    {
        let spec = r.spec();
        let channels = spec.channels as usize;
        let frames = r.duration() as usize;
        return Ok(Sound {
            samples: read_samples(&mut r, frames * channels).map_err(|e| err(&e))?,
            channels,
            rate: spec.sample_rate,
        });
    }
    decode(path).map_err(|e| err(&e))
}

/// Video file name endings, for files whose picture the decoder does not
/// list as a track.
const VIDEO: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi"];

/// The file holding `path`'s sound. For a video, its sound taken out into
/// `cache` (once; again finds it there); for anything else, `path` itself.
/// AAC sound, which most videos have, is copied out unchanged, so it is
/// no bigger than it was in the video; any other is decoded into a WAV.
pub fn soundtrack(path: &Path, cache: &Path) -> Result<PathBuf, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let format = probe(path).map_err(|e| err(&e))?;
    let video = format
        .tracks()
        .iter()
        .any(|t| matches!(t.codec_params, Some(CodecParameters::Video(_))))
        || path
            .extension()
            .is_some_and(|e| VIDEO.contains(&e.to_string_lossy().to_lowercase().as_str()));
    if !video {
        return Ok(path.to_path_buf());
    }
    let stem = path
        .file_stem()
        .map_or("sound".into(), |s| s.to_string_lossy());
    // In a folder of its own, so the file keeps the video's plain name,
    // which is what its track is called.
    let dir = cache_path(path, 0, cache)
        .map_err(|e| err(&e))?
        .with_extension("");
    let (aac, wav) = (
        dir.join(format!("{stem}.aac")),
        dir.join(format!("{stem}.wav")),
    );
    for done in [&aac, &wav] {
        if done.exists() {
            return Ok(done.clone());
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dir.join(format!("{stem}.part"));
    let out = if copy_aac(format, &part).map_err(|e| err(&e))? {
        aac
    } else {
        let sound = decode(path).map_err(|e| err(&e))?;
        write_float_wav(&part, &sound.samples, sound.channels, sound.rate).map_err(|e| err(&e))?;
        wav
    };
    std::fs::rename(&part, &out).map_err(|e| err(&e))?;
    Ok(out)
}

fn write_float_wav(path: &Path, samples: &[f32], channels: usize, rate: u32) -> hound::Result<()> {
    let mut w = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: channels as u16,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for &s in samples {
        w.write_sample(s)?;
    }
    w.finalize()
}

/// Opens `path` with whichever reader fits its contents.
fn probe(path: &Path) -> Result<Box<dyn FormatReader>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| match e {
            DecodeError::Unsupported(_) => "this app cannot open this kind of file".to_string(),
            e => e.to_string(),
        })
}

/// The first audio track: its id and how it is coded.
fn audio_track(
    format: &dyn FormatReader,
) -> Result<(u32, symphonia::core::codecs::audio::AudioCodecParameters), String> {
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("has no sound in it")?;
    match &track.codec_params {
        Some(CodecParameters::Audio(p)) => Ok((track.id, p.clone())),
        _ => Err("has no sound in it".into()),
    }
}

/// Decodes the whole of the first audio track, mixed down to stereo if
/// it has more channels. A damaged frame is left out, as players do.
fn decode(path: &Path) -> Result<Sound, String> {
    let mut format = probe(path)?;
    let (id, params) = audio_track(format.as_ref())?;
    let mut dec = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| match e {
            DecodeError::Unsupported(_) => {
                "its sound is in a format this app cannot decode".to_string()
            }
            e => e.to_string(),
        })?;
    let (mut samples, mut buf) = (Vec::new(), Vec::new());
    let mut layout: Option<(Channels, u32)> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(DecodeError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != id {
            continue;
        }
        let decoded = match dec.decode(&packet) {
            Ok(d) => d,
            Err(DecodeError::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let spec = decoded.spec();
        match &layout {
            None => layout = Some((spec.channels().clone(), spec.rate())),
            Some((c, r)) if c != spec.channels() || *r != spec.rate() => {
                return Err("its sound changes channels or rate partway".into());
            }
            Some(_) => {}
        }
        decoded.copy_to_vec_interleaved::<f32>(&mut buf);
        samples.extend_from_slice(&buf);
    }
    let Some((channels, rate)) = layout else {
        return Err("has no sound in it".into());
    };
    let (samples, channels) = to_stereo(samples, &channels);
    Ok(Sound {
        samples,
        channels,
        rate,
    })
}

/// Mono and stereo as they are; more channels mixed to stereo, centre
/// and surrounds 3 dB down and the low-frequency channel left out.
fn to_stereo(samples: Vec<f32>, channels: &Channels) -> (Vec<f32>, usize) {
    let n = channels.count();
    if n <= 2 {
        return (samples, n.max(1));
    }
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let gains: Vec<(f32, f32)> = match channels {
        Channels::Positioned(p) => p
            .iter()
            .map(|c| {
                if c == Position::FRONT_LEFT {
                    (1.0, 0.0)
                } else if c == Position::FRONT_RIGHT {
                    (0.0, 1.0)
                } else if c == Position::FRONT_CENTER {
                    (H, H)
                } else if c.intersects(Position::LFE1 | Position::LFE2) {
                    (0.0, 0.0)
                } else if c.intersects(
                    Position::REAR_LEFT
                        | Position::SIDE_LEFT
                        | Position::FRONT_LEFT_CENTER
                        | Position::FRONT_LEFT_WIDE,
                ) {
                    (H, 0.0)
                } else if c.intersects(
                    Position::REAR_RIGHT
                        | Position::SIDE_RIGHT
                        | Position::FRONT_RIGHT_CENTER
                        | Position::FRONT_RIGHT_WIDE,
                ) {
                    (0.0, H)
                } else {
                    (0.5, 0.5)
                }
            })
            .collect(),
        // Unnamed channels: the first two are taken as left and right.
        _ => (0..n)
            .map(|i| match i {
                0 => (1.0, 0.0),
                1 => (0.0, 1.0),
                _ => (0.0, 0.0),
            })
            .collect(),
    };
    let out = samples
        .chunks_exact(n)
        .flat_map(|f| {
            let (l, r) = f
                .iter()
                .zip(&gains)
                .fold((0.0, 0.0), |(l, r), (s, g)| (l + s * g.0, r + s * g.1));
            [l, r]
        })
        .collect();
    (out, 2)
}

/// Copies an AAC track out of its container into an ADTS stream (plain
/// AAC frames, each with a short header), which plays as it did. False,
/// with nothing written, if the track is not AAC that ADTS can carry.
fn copy_aac(mut format: Box<dyn FormatReader>, out: &Path) -> Result<bool, String> {
    let (id, params) = audio_track(format.as_ref())?;
    if params.codec != CODEC_ID_AAC {
        return Ok(false);
    }
    // The track's set-up: object type (5 bits), rate index (4), channel
    // layout (4). ADTS has no room for the rarer escaped forms.
    let Some(&[a, b, ..]) = params.extra_data.as_deref() else {
        return Ok(false);
    };
    let (object, rate_index, layout) = (a >> 3, ((a & 7) << 1) | (b >> 7), (b >> 3) & 0xf);
    if !(1..=4).contains(&object) || rate_index >= 13 || !(1..=7).contains(&layout) {
        return Ok(false);
    }
    let mut w = std::io::BufWriter::new(File::create(out).map_err(|e| e.to_string())?);
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(DecodeError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != id {
            continue;
        }
        let len = packet.data.len() + 7;
        if len >= 1 << 13 {
            return Err("an AAC frame is too long to copy".into());
        }
        let header = [
            0xff,
            0xf1,
            ((object - 1) << 6) | (rate_index << 2) | (layout >> 2),
            ((layout & 3) << 6) | (len >> 11) as u8,
            (len >> 3) as u8,
            ((len & 7) << 5) as u8 | 0x1f,
            0xfc,
        ];
        std::io::Write::write_all(&mut w, &header).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut w, &packet.data).map_err(|e| e.to_string())?;
    }
    std::io::Write::flush(&mut w).map_err(|e| e.to_string())?;
    Ok(true)
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
