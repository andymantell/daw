// SPDX-License-Identifier: GPL-3.0-or-later
//! Copies of a video with new sound (roadmap 3.10): the picture comes
//! through unchanged and the new sound replaces the old.

#![cfg(any(target_os = "linux", windows))]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use daw_video::{Frame, Reader, wrap, wrapped_extension};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Which frame a fixture frame is, from its colour.
fn counted(f: &Frame) -> u64 {
    let mid = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
    (f.rgba[mid] as f64 / 10.0).round() as u64
}

/// Two seconds of a stereo tone at 48 kHz, 24-bit like the app's
/// exports.
fn tone(path: &Path) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..96_000 {
        let v = ((i as f64 * 440.0 * std::f64::consts::TAU / 48_000.0).sin() * 3e6) as i32;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-wrap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// How long the file's sound runs, in seconds, decoded by the system.
#[cfg(target_os = "linux")]
fn sound_seconds(path: &Path) -> f64 {
    use gstreamer as gst;
    use gstreamer::prelude::*;
    gst::init().unwrap();
    let desc = format!(
        "filesrc location=\"{}\" ! decodebin caps=audio/x-raw ! audioconvert \
         ! audio/x-raw,format=F32LE,channels=1 ! appsink name=sink sync=false",
        path.display()
    );
    let p = gst::parse::launch(&desc)
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
    let sink = p
        .by_name("sink")
        .unwrap()
        .downcast::<gstreamer_app::AppSink>()
        .unwrap();
    p.set_state(gst::State::Playing).unwrap();
    let (mut samples, mut rate) = (0usize, 1);
    while let Some(s) = sink.try_pull_sample(gst::ClockTime::from_seconds(10)) {
        let caps = s.caps().unwrap();
        rate = caps.structure(0).unwrap().get::<i32>("rate").unwrap();
        samples += s.buffer().unwrap().size() / 4;
    }
    p.set_state(gst::State::Null).unwrap();
    samples as f64 / rate as f64
}

fn check(video: &Path, name: &str) {
    let d = dir(name);
    let mix = d.join("mix.wav");
    tone(&mix);
    let out = d.join(format!("with mix.{}", wrapped_extension(video)));
    wrap(video, &mix, &out, &AtomicBool::new(false)).unwrap();
    assert!(out.is_file());
    assert!(
        !d.join(format!("with mix.part.{}", wrapped_extension(video)))
            .exists()
    );

    // The same picture, frame for frame.
    let mut r = Reader::open(&out, (64, 48)).unwrap();
    assert_eq!(r.info().frames(), 24, "{:?}", r.info());
    let mut got = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        got.push((counted(&f), format!("{:.4}", f.time)));
    }
    let want: Vec<u64> = (0..24).collect();
    assert_eq!(
        got.iter().map(|g| g.0).collect::<Vec<_>>(),
        want,
        "frames (number, time): {got:?}"
    );
    #[cfg(target_os = "linux")]
    {
        let s = sound_seconds(&out);
        assert!((s - 2.0).abs() < 0.1, "{s} seconds of sound");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_mix_goes_into_a_copy_of_the_video() {
    if cfg!(windows) {
        // The system's writer makes MP4 files, and reads them.
        check(&fixture("counting.mp4"), "mp4");
        check(&fixture("counting-bframes.mp4"), "bframes");
    } else if daw_video::CAN_WRAP {
        check(&fixture("counting.ogv"), "ogv");
    } else {
        // Not offered yet: it says so, and leaves nothing behind.
        let d = dir("not-yet");
        let mix = d.join("mix.wav");
        tone(&mix);
        let out = d.join("with mix.mp4");
        let e = wrap(
            &fixture("counting.mp4"),
            &mix,
            &out,
            &AtomicBool::new(false),
        );
        assert!(matches!(e, Err(daw_video::Error::CantWrap(_))), "{e:?}");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// MP4 on Linux, where the system has what it takes: a reader and
/// writer for MP4 and an H.264 decoder, which CI doesn't install.
#[cfg(target_os = "linux")]
#[test]
fn an_mp4_copy_where_the_system_can() {
    use gstreamer as gst;
    gst::init().unwrap();
    let has = |n: &str| gst::ElementFactory::find(n).is_some();
    if !(has("qtdemux") && has("mp4mux")) {
        eprintln!("skipped: no MP4 support here");
        return;
    }
    let d = dir("mp4");
    let mix = d.join("mix.wav");
    tone(&mix);
    let out = d.join("with mix.mp4");
    wrap(
        &fixture("counting.mp4"),
        &mix,
        &out,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(std::fs::metadata(&out).unwrap().len() > 1000);
    let s = sound_seconds(&out);
    assert!((s - 2.0).abs() < 0.1, "{s} seconds of sound");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_file_that_isnt_video_says_so() {
    let d = dir("bad");
    let mix = d.join("mix.wav");
    tone(&mix);
    let out = d.join("out.mp4");
    let e = wrap(&mix, &mix, &out, &AtomicBool::new(false));
    assert!(e.is_err(), "{e:?}");
    assert!(!out.exists());
    let _ = std::fs::remove_dir_all(&d);
}
