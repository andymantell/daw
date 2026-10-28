// SPDX-License-Identifier: GPL-3.0-or-later
//! Clips streamed from WAV files on disk.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::path::PathBuf;
use std::sync::Arc;

use daw_engine::engine::{self, Command};
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::stream::{CHUNK, StreamSource};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
/// Long enough for several chunks.
const LEN: usize = CHUNK * 4 + 1234;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

fn temp_wav(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-engine-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Sample `i` of the left channel is `(i + 1) / 1e6`; the right is its
/// negative.
fn value(i: usize) -> f32 {
    (i + 1) as f32 / 1e6
}

fn write_float_stereo(name: &str) -> PathBuf {
    let path = temp_wav(name);
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..LEN {
        w.write_sample(value(i)).unwrap();
        w.write_sample(-value(i)).unwrap();
    }
    w.finalize().unwrap();
    path
}

fn memory_copy() -> Arc<AudioData> {
    Arc::new(AudioData {
        channels: vec![
            (0..LEN).map(value).collect(),
            (0..LEN).map(|i| -value(i)).collect(),
        ],
        rate: RATE,
    })
}

#[test]
fn streamed_clip_plays_the_same_as_in_memory() {
    let src = StreamSource::open(&write_float_stereo("same.wav")).unwrap();
    assert_eq!(src.frames, LEN);
    let render = |clip: ClipSpec| {
        let mut s = session();
        s.tracks.push(TrackSpec::new(vec![clip]));
        engine::render_offline(s.compile(512).unwrap(), Frames(0), LEN + 48_000, 512)
    };
    let mut a = ClipSpec::new(Ticks(960), src.clone());
    a.offset = 1000;
    let mut b = ClipSpec::new(Ticks(960), memory_copy());
    b.offset = 1000;
    let (streamed, memory) = (render(a), render(b));
    assert_eq!(streamed, memory);
    assert!(streamed[0].iter().any(|&x| x != 0.0));
}

#[test]
fn sixteen_bit_files_are_scaled() {
    let path = temp_wav("int16.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for v in [16384i16, -32768, 0, 32767] {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        StreamSource::open(&path).unwrap(),
    )]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let [l, _] = engine::render_offline(s.compile(64).unwrap(), Frames(0), 4, 64);
    assert_eq!(l, vec![0.5, -1.0, 0.0, 32767.0 / 32768.0]);
}

#[test]
fn seeks_and_loops_across_chunks_without_gaps() {
    let src = StreamSource::open(&write_float_stereo("loop.wav")).unwrap();
    let mut s = session();
    s.tracks
        .push(TrackSpec::new(vec![ClipSpec::new(Ticks(0), src)]));
    let (mut ctl, mut eng) = engine::new(512);
    let mut disk = ctl.take_streamer().unwrap();
    ctl.set_graph(s.compile(512).unwrap()).ok().unwrap();
    // A loop that crosses a chunk boundary, entered from before it.
    let (a, b) = (CHUNK as i64 * 2 - 3000, CHUNK as i64 * 3 + 500);
    ctl.send(Command::SetLoop(Some((Frames(a), Frames(b)))))
        .ok()
        .unwrap();
    ctl.send(Command::Seek(Frames(a - 10_000))).ok().unwrap();
    eng.process(&mut [], &mut []);
    disk.service();
    ctl.send(Command::Play).ok().unwrap();
    let mut out = Vec::new();
    for _ in 0..400 {
        let (mut l, mut r) = (vec![0.0; 333], vec![0.0; 333]);
        eng.process(&mut l, &mut r);
        disk.service();
        out.extend(l);
    }
    assert_eq!(ctl.status().underruns(), 0);
    let mut p = a - 10_000;
    for (i, &x) in out.iter().enumerate() {
        // Centred stereo: balance leaves the left channel untouched.
        assert_eq!(x, value(p as usize), "output {i}, file frame {p}");
        p += 1;
        if p == b {
            p = a;
        }
    }
}

#[test]
fn missing_chunks_play_silence_and_are_counted() {
    let src = StreamSource::open(&write_float_stereo("missing.wav")).unwrap();
    let mut s = session();
    s.tracks
        .push(TrackSpec::new(vec![ClipSpec::new(Ticks(0), src)]));
    let (mut ctl, mut eng) = engine::new(256);
    ctl.set_graph(s.compile(256).unwrap()).ok().unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    eng.process(&mut l, &mut r);
    assert!(l.iter().all(|&x| x == 0.0));
    assert_eq!(ctl.status().underruns(), 256);
}

#[test]
fn a_new_graph_keeps_loaded_chunks() {
    let src = StreamSource::open(&write_float_stereo("keep.wav")).unwrap();
    let mut s = session();
    s.tracks
        .push(TrackSpec::new(vec![ClipSpec::new(Ticks(0), src)]));
    let (mut ctl, mut eng) = engine::new(256);
    let mut disk = ctl.take_streamer().unwrap();
    ctl.set_graph(s.compile(256).unwrap()).ok().unwrap();
    eng.process(&mut [], &mut []);
    assert!(disk.service() > 0);
    // An edit: the same file, now quieter, on a new graph.
    s.tracks[0].mix.gain_db = -6.0;
    ctl.set_graph(s.compile(256).unwrap()).ok().unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    for _ in 0..50 {
        eng.process(&mut l, &mut r);
    }
    assert_eq!(ctl.status().underruns(), 0, "no gap after the edit");
    assert!(l[0] != 0.0);
}

#[test]
fn streaming_never_allocates_on_the_audio_thread() {
    let src = StreamSource::open(&write_float_stereo("rt.wav")).unwrap();
    let mut s = session();
    for i in 0..4 {
        s.tracks.push(TrackSpec::new(vec![ClipSpec::new(
            Ticks(i * 480),
            src.clone(),
        )]));
    }
    let (mut ctl, mut eng) = engine::new(256);
    let disk = ctl.take_streamer().unwrap().spawn();
    ctl.set_graph(s.compile(256).unwrap()).ok().unwrap();
    // Loop over most of the file so chunks keep being loaded and dropped.
    ctl.send(Command::SetLoop(Some((
        Frames(1000),
        Frames(LEN as i64 - 1000),
    ))))
    .ok()
    .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let audio = std::thread::spawn(move || {
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        let rt = rt_check::RtGuard::enter();
        // Faster than real time, but paced so the disk thread keeps up.
        for _ in 0..3000 {
            eng.process(&mut l, &mut r);
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        drop(rt);
        rt_check::violations()
    });
    assert_eq!(audio.join().unwrap(), 0, "allocations on the audio thread");
    drop(disk);
}

#[test]
fn peaks_cover_the_whole_file_across_chunks() {
    let path = write_float_stereo("peaks.wav");
    let src = StreamSource::open(&path).unwrap();
    let hop = 1000;
    let p = src.peaks(hop).unwrap();
    assert_eq!(p.len(), LEN.div_ceil(hop));
    for (k, &(lo, hi)) in p.iter().enumerate() {
        // The last frame of each hop is its largest either way.
        let last = ((k + 1) * hop).min(LEN) - 1;
        assert!((hi - value(last)).abs() < 1e-9, "hop {k}: {hi}");
        assert!((lo + value(last)).abs() < 1e-9, "hop {k}: {lo}");
    }
}
