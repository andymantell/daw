// SPDX-License-Identifier: GPL-3.0-or-later
//! Exporting to WAV (roadmap 1.8).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use daw_engine::export::write_wav;
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 44_100;

fn session(samples: Vec<f32>) -> Session {
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    let mut t = TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        Arc::new(AudioData {
            channels: vec![samples],
            rate: RATE,
        }),
    )]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    s
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("daw-export-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn writes_the_mix_as_24_bit_wav() {
    let samples: Vec<f32> = (0..10_000)
        .map(|i| ((i % 200) as f32 - 100.0) / 200.0)
        .collect();
    let s = session(samples.clone());
    let path = dir("mix").join("mix.wav");
    let done = AtomicU64::new(0);
    let w = write_wav(
        s.compile(512).unwrap(),
        Frames(0),
        10_000,
        &path,
        &done,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!w.clipped());
    assert_eq!(done.load(Ordering::Relaxed), 10_000);

    let mut r = hound::WavReader::open(&path).unwrap();
    let spec = r.spec();
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (2, RATE, 24)
    );
    let got: Vec<i32> = r.samples::<i32>().map(Result::unwrap).collect();
    assert_eq!(got.len(), 20_000);
    for (i, x) in samples.iter().enumerate() {
        let want = (x * 8_388_607.0).round() as i32;
        assert!((got[2 * i] - want).abs() <= 1, "frame {i}");
        assert_eq!(got[2 * i + 1], 0, "hard left leaves right silent");
    }
    assert!(!path.with_extension("wav.part").exists());
}

#[test]
fn reports_clipping_and_cancels_cleanly() {
    let s = session(vec![1.5; 1000]);
    let d = dir("clip");
    let w = write_wav(
        s.compile(512).unwrap(),
        Frames(0),
        1000,
        &d.join("loud.wav"),
        &AtomicU64::new(0),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(w.clipped());
    assert!((w.peak - 1.5).abs() < 1e-6);

    let s = session(vec![0.1; 100_000]);
    let path = d.join("cancelled.wav");
    let err = write_wav(
        s.compile(512).unwrap(),
        Frames(0),
        100_000,
        &path,
        &AtomicU64::new(0),
        &AtomicBool::new(true),
    )
    .unwrap_err();
    assert_eq!(err, "cancelled");
    assert!(!path.exists());
    assert!(!path.with_extension("wav.part").exists());
}
