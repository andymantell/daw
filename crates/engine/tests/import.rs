// SPDX-License-Identifier: GPL-3.0-or-later
//! Importing files: other sample rates (roadmap 1.1), other formats and
//! the sound of videos (1.5).

use std::f32::consts::TAU;
use std::path::PathBuf;

use daw_engine::import::{self, resample};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-import-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sine(hz: f32, rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (TAU * hz * i as f32 / rate as f32).sin() * 0.5)
        .collect()
}

/// How closely `out` matches a 1 kHz sine at `rate`, delayed by the best
/// fraction of a frame within one frame either way: (delay, largest error).
fn fit(out: &[f32], rate: u32) -> (f32, f32) {
    let n = out.len();
    (-20..=20)
        .map(|k| {
            let lag = k as f32 / 20.0;
            let err = (n / 10..n * 9 / 10)
                .map(|i| {
                    (out[i] - 0.5 * (TAU * 1000.0 * (i as f32 - lag) / rate as f32).sin()).abs()
                })
                .fold(0.0, f32::max);
            (lag, err)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
}

#[test]
fn a_sine_keeps_its_pitch_level_and_timing() {
    // Within a frame of the right moment: the resampler's delay is
    // corrected to the nearest whole frame (21 µs at 48 kHz).
    for (from, to) in [
        (44_100, 48_000),
        (48_000, 44_100),
        (44_100, 96_000),
        (96_000, 48_000),
        (22_050, 48_000),
    ] {
        let out = resample(&sine(1000.0, from, from as usize), 1, from, to).unwrap();
        assert_eq!(out.len(), to as usize);
        let (lag, err) = fit(&out, to);
        assert!(
            err < 0.01,
            "{from} to {to}: largest difference {err} at delay {lag}"
        );
    }
}

#[test]
fn audio_starts_on_the_same_moment() {
    // A click 0.1 s in stays 0.1 s in.
    let mut x = vec![0.0; 44_100];
    x[4_410] = 1.0;
    let out = resample(&x, 1, 44_100, 96_000).unwrap();
    let peak = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert!((9_599..=9_601).contains(&peak), "peak at {peak}");
}

#[test]
fn stereo_channels_stay_apart() {
    let frames = 22_050;
    let (l, r) = (sine(440.0, 22_050, frames), sine(0.0, 22_050, frames));
    let x: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
    let out = resample(&x, 2, 22_050, 48_000).unwrap();
    assert_eq!(out.len(), 2 * 48_000);
    let right_peak = out
        .iter()
        .skip(1)
        .step_by(2)
        .map(|s| s.abs())
        .fold(0.0, f32::max);
    let left_peak = out.iter().step_by(2).map(|s| s.abs()).fold(0.0, f32::max);
    assert!(right_peak < 1e-4, "right {right_peak}");
    assert!((left_peak - 0.5).abs() < 0.02, "left {left_peak}");
}

#[test]
fn files_convert_once_into_the_cache() {
    let dir = temp_dir("cache");
    let path = dir.join("tone.wav");
    let mut w = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for s in sine(1000.0, 44_100, 44_100) {
        w.write_sample((s * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();

    let cache = dir.join("cache");
    let src = import::open_at_rate(&path, 48_000, &cache).unwrap();
    assert_eq!((src.rate, src.frames, src.channels), (48_000, 48_000, 1));
    assert!(src.path.starts_with(&cache));
    let made = std::fs::metadata(&src.path).unwrap().modified().unwrap();

    // The same file again uses the copy.
    let again = import::open_at_rate(&path, 48_000, &cache).unwrap();
    assert_eq!(again.path, src.path);
    assert_eq!(
        std::fs::metadata(&again.path).unwrap().modified().unwrap(),
        made
    );

    // At its own rate, the original streams directly.
    let direct = import::open_at_rate(&path, 44_100, &cache).unwrap();
    assert_eq!(direct.path, path);
    let _ = std::fs::remove_dir_all(&dir);
}

fn media(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/media")
        .join(name)
}

/// The level (RMS) and frequency (from rising zero crossings) of one
/// channel, over the middle of the sound.
fn tone(samples: &[f32], channels: usize, ch: usize, rate: u32) -> (f32, f32) {
    let x: Vec<f32> = samples.iter().skip(ch).step_by(channels).copied().collect();
    let mid = &x[x.len() / 5..x.len() * 4 / 5];
    let rms = (mid.iter().map(|s| s * s).sum::<f32>() / mid.len() as f32).sqrt();
    let ups = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    (rms, ups as f32 * rate as f32 / mid.len() as f32)
}

/// Where the sound starts: the first frame above a tenth of full scale.
fn onset(samples: &[f32], channels: usize) -> usize {
    samples.iter().position(|s| s.abs() > 0.05).unwrap_or(0) / channels
}

#[test]
fn compressed_and_lossless_files_decode_at_pitch_and_level() {
    // Each holds 1 s of a 1 kHz sine peaking at 0.5 (RMS 0.354).
    for (file, channels, rate) in [
        ("tone.mp3", 2, 44_100),
        ("tone.flac", 1, 48_000),
        ("tone.ogg", 2, 44_100),
        ("tone.m4a", 2, 44_100),
        ("tone.mp4", 2, 48_000),
        ("tone.mov", 2, 48_000),
        ("tone.mkv", 2, 44_100),
    ] {
        let s = import::read(&media(file)).unwrap();
        assert_eq!((s.channels, s.rate), (channels, rate), "{file}");
        let frames = s.samples.len() / s.channels;
        // AAC keeps its encoder's start-up frames (a known limit): up to
        // about 0.05 s longer.
        assert!(
            (rate as usize..rate as usize * 21 / 20).contains(&frames),
            "{file}: {frames} frames"
        );
        for ch in 0..channels {
            let (rms, hz) = tone(&s.samples, s.channels, ch, s.rate);
            assert!((rms - 0.354).abs() < 0.03, "{file}: level {rms}");
            assert!((hz - 1000.0).abs() < 5.0, "{file}: {hz} Hz");
        }
    }
}

#[test]
fn lossless_and_mp3_files_start_on_time() {
    for file in ["tone.mp3", "tone.flac", "tone.ogg", "tone.mkv"] {
        let s = import::read(&media(file)).unwrap();
        assert!(onset(&s.samples, s.channels) <= 2, "{file}");
    }
}

#[test]
fn surround_is_mixed_to_stereo() {
    // The sine is in the centre channel only, so comes out of both sides
    // 3 dB down.
    let s = import::read(&media("surround.flac")).unwrap();
    assert_eq!(s.channels, 2);
    for ch in 0..2 {
        let (rms, _) = tone(&s.samples, 2, ch, s.rate);
        assert!((rms - 0.25).abs() < 0.01, "channel {ch}: {rms}");
    }
}

#[test]
fn a_video_keeps_only_its_sound() {
    let cache = temp_dir("video");
    for (file, ext) in [
        ("tone.mp4", "aac"),
        ("tone.mov", "aac"),
        ("tone.mkv", "wav"),
    ] {
        let sound = import::soundtrack(&media(file), &cache).unwrap();
        assert!(sound.starts_with(&cache), "{file}");
        assert_eq!(sound.extension().unwrap(), ext, "{file}");
        // Named like the video, so its track is too.
        assert_eq!(sound.file_stem().unwrap(), "tone");
        // AAC copied out plays exactly as it did in the video.
        let (a, b) = (
            import::read(&sound).unwrap(),
            import::read(&media(file)).unwrap(),
        );
        assert_eq!((a.channels, a.rate), (b.channels, b.rate), "{file}");
        assert_eq!(a.samples, b.samples, "{file}");
        // Again: the same file, not made a second time.
        assert_eq!(import::soundtrack(&media(file), &cache).unwrap(), sound);
    }
    // Sound files are left as they are.
    for file in ["tone.mp3", "tone.m4a", "tone.flac"] {
        assert_eq!(
            import::soundtrack(&media(file), &cache).unwrap(),
            media(file)
        );
    }
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn compressed_files_stream_from_the_cache() {
    let cache = temp_dir("compressed");
    for rate in [44_100, 48_000] {
        let src = import::open_at_rate(&media("tone.mp3"), rate, &cache).unwrap();
        assert_eq!((src.rate, src.channels), (rate, 2));
        assert!(src.path.starts_with(&cache));
        assert_eq!(src.frames, 44_100 * rate as usize / 44_100);
    }
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn a_file_that_is_not_audio_says_so() {
    let dir = temp_dir("not-audio");
    let path = dir.join("notes.mp3");
    std::fs::write(&path, "not really an mp3").unwrap();
    let e = import::open_at_rate(&path, 48_000, &dir.join("cache")).unwrap_err();
    assert!(e.contains("notes.mp3"), "{e}");
    assert!(e.contains("cannot open"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
