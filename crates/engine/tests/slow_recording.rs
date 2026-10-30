// SPDX-License-Identifier: GPL-3.0-or-later
//! Recording at a practice speed (Phase 4 D5): the song plays slowed
//! down, armed tracks record as played, and each take is sped back up to
//! the song's speed, placed where it was played.

use std::path::{Path, PathBuf};

use daw_engine::engine::{self, Command, Controller, Engine};
use daw_engine::graph::{Input, Session, TrackSpec};
use daw_engine::record::{Recorder, Take};
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};
use daw_engine::tuner::{Detector, window};

const RATE: u32 = 48_000;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-slow-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Every sample says which captured frame it is.
fn ramp(frame: usize) -> f32 {
    frame as f32 / 1e6
}

struct Rig {
    ctl: Controller,
    eng: Engine,
    rec: Recorder,
    fed: usize,
    dir: PathBuf,
    /// The input device's one channel, by captured frame.
    signal: fn(usize) -> f32,
}

fn rig(name: &str, signal: fn(usize) -> f32) -> Rig {
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    let mut t = TrackSpec::new(vec![]);
    t.input = Some(Input::mono(0));
    t.arm = true;
    s.tracks = vec![t];
    let (mut ctl, eng) = engine::new(512);
    let dir = temp_dir(name);
    ctl.set_record_dir(&dir);
    ctl.set_graph(s.compile(512).unwrap()).ok().unwrap();
    let rec = ctl.take_recorder().unwrap();
    Rig {
        ctl,
        eng,
        rec,
        fed: 0,
        dir,
        signal,
    }
}

impl Rig {
    fn run(&mut self, blocks: usize, n: usize) {
        for _ in 0..blocks {
            let inp: Vec<f32> = (self.fed..self.fed + n).map(self.signal).collect();
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            self.eng.process_duplex(&inp, 1, &mut l, &mut r, None);
            self.fed += n;
            self.rec.service();
        }
    }

    fn send(&mut self, c: Command) {
        self.ctl.send(c).ok().unwrap();
    }

    fn practice(&mut self, speed: f64, semitones: f64) {
        self.send(Command::SetPractice { speed, semitones });
    }

    fn stop_and_take(&mut self) -> Vec<Take> {
        self.send(Command::Stop);
        self.run(1, 64);
        self.rec.service();
        let mut t = self.ctl.finished_takes();
        t.sort_by_key(|t| t.path.clone());
        t
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn read(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_take_recorded_at_half_speed_is_sped_back_up_where_it_was_played() {
    let mut r = rig("half", ramp);
    r.practice(0.5, 0.0);
    r.send(Command::Seek(Frames(4800)));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run(40, 480);
    let takes = r.stop_and_take();
    assert_eq!(takes.len(), 1, "{takes:?}");
    let t = &takes[0];
    assert_eq!(t.error, None);
    assert_eq!(t.start, Frames(4800));
    let s = t.slowed.as_ref().expect("played slowed");
    assert_eq!((s.speed, s.semitones), (0.5, 0.0));
    // As played: every frame captured, in order.
    let played = read(&s.as_played);
    let want: Vec<f32> = (0..played.len()).map(ramp).collect();
    assert_eq!(played, want);
    assert!(played.len() >= 40 * 480);
    // Sped up: half as long, in its own file.
    assert_ne!(t.path, s.as_played);
    assert_eq!(t.frames, (played.len() as f64 * 0.5).round() as usize);
    assert_eq!(read(&t.path).len(), t.frames);
    // The round trip passes at the practice speed: half as much song.
    assert_eq!(t.clip_start(Frames(200)), Frames(4700));
}

fn tone(frame: usize) -> f32 {
    let at = frame as f32 / RATE as f32;
    let mut v = 0.4 * (std::f32::consts::TAU * 220.0 * at).sin();
    // A click every 9600 captured frames.
    if frame.is_multiple_of(9600) {
        v += 0.9;
    }
    v
}

#[test]
fn a_slowed_take_keeps_its_pitch_and_its_timing() {
    let mut r = rig("tone", tone);
    r.practice(0.7, 0.0);
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run(200, 480);
    let takes = r.stop_and_take();
    assert_eq!(takes.len(), 1, "{takes:?}");
    let out = read(&takes[0].path);
    // The pitch played is the pitch kept.
    let mut d = Detector::new(RATE);
    let mid = out.len() / 2;
    let got = d.detect(&out[mid..mid + window(RATE)]).expect("a pitch");
    let cents = 1200.0 * (got / 220.0).log2();
    assert!(cents.abs() < 5.0, "{got} Hz");
    // A click played at captured frame 9600 k sits at 9600 k × 0.7.
    for k in 2..8 {
        let want = (9600.0 * k as f64 * 0.7) as usize;
        let around = &out[want - 600..want + 600];
        let peak = (around.iter().enumerate())
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0 as i64
            - 600;
        assert!(peak.abs() < 120, "click {k} is {peak} frames out");
    }
}

#[test]
fn a_take_is_transposed_back_to_the_song_s_key() {
    let mut r = rig("transposed", tone);
    r.practice(1.0, 2.0);
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run(100, 480);
    let takes = r.stop_and_take();
    assert_eq!(takes.len(), 1, "{takes:?}");
    let t = &takes[0];
    assert_eq!(t.slowed.as_ref().map(|s| s.semitones), Some(2.0));
    let out = read(&t.path);
    assert_eq!(out.len(), read(&t.slowed.as_ref().unwrap().as_played).len());
    let mut d = Detector::new(RATE);
    let mid = out.len() / 2;
    let got = d.detect(&out[mid..mid + window(RATE)]).expect("a pitch");
    let want = 220.0 * 2f32.powf(-2.0 / 12.0);
    assert!((1200.0 * (got / want).log2()).abs() < 5.0, "{got} Hz");
}

#[test]
fn a_punch_range_at_a_practice_speed_is_on_the_song_s_timeline() {
    let mut r = rig("punch", ramp);
    r.practice(0.5, 0.0);
    r.send(Command::SetPunch(Some((Frames(1000), Frames(3000)))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    // 700-frame blocks: both ends fall mid-block.
    r.run(10, 700);
    assert!(r.ctl.status().playing());
    r.rec.service();
    let takes = r.ctl.finished_takes();
    assert_eq!(takes.len(), 1, "{takes:?}");
    let t = &takes[0];
    assert_eq!(t.start, Frames(1000));
    // Song frames 1000..3000 take 4000 frames to play at half speed.
    let played = read(&t.slowed.as_ref().unwrap().as_played);
    assert_eq!(played.len(), 4000);
    assert_eq!(played[0], ramp(2000));
    assert_eq!(t.frames, 2000);
}

#[test]
fn each_loop_pass_and_each_speed_is_a_take_of_its_own() {
    let mut r = rig("loop", ramp);
    r.practice(0.5, 0.0);
    r.send(Command::SetLoop(Some((Frames(0), Frames(4800)))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    // Two passes of 9600 output frames each, and a little more.
    r.run(41, 480);
    // Now faster, mid-pass: a new take from where the song is.
    r.practice(0.75, 0.0);
    r.run(5, 480);
    let takes = r.stop_and_take();
    assert_eq!(takes.len(), 4, "{takes:?}");
    for t in &takes[..2] {
        assert_eq!(t.start, Frames(0));
        assert_eq!(t.slowed.as_ref().unwrap().speed, 0.5);
        assert_eq!(t.frames, 4800);
    }
    let last = takes.last().unwrap();
    assert_eq!(last.slowed.as_ref().unwrap().speed, 0.75);
    // 480 frames into the third pass at half speed.
    assert_eq!(last.start, Frames(240));
}

#[test]
fn post_roll_stops_playback_at_a_practice_speed_too() {
    let mut r = rig("stop", ramp);
    r.practice(0.5, 0.0);
    r.send(Command::StopAt(Some(Frames(2000))));
    r.send(Command::Play);
    r.run(10, 512);
    assert!(!r.ctl.status().playing());
    assert_eq!(r.ctl.status().position(), Frames(2000));
}
