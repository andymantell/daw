// SPDX-License-Identifier: GPL-3.0-or-later
//! Importing files at other sample rates (roadmap 1.1).

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
