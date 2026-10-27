// SPDX-License-Identifier: GPL-3.0-or-later
//! Monitoring and recording (roadmap 1.2).

#[path = "common/rt_check.rs"]
mod rt_check;

use std::path::PathBuf;

use daw_engine::engine::{self, Command, Controller, Engine};
use daw_engine::graph::{Input, Session, TrackSpec};
use daw_engine::record::{Recorder, Take};
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};

const RATE: u32 = 48_000;
const CH: usize = 4;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-rec-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A four-channel device input where every sample says which frame and
/// channel it is: `frame + channel / 10`, scaled down.
fn input(from: usize, frames: usize) -> Vec<f32> {
    (from..from + frames)
        .flat_map(|f| (0..CH).map(move |c| value(f, c)))
        .collect()
}

fn value(frame: usize, ch: usize) -> f32 {
    (frame as f32 + ch as f32 / 10.0) / 1e6
}

struct Rig {
    ctl: Controller,
    eng: Engine,
    rec: Recorder,
    /// Input frames fed so far.
    fed: usize,
    dir: PathBuf,
}

/// An engine with one empty track per entry of `tracks`.
fn rig(name: &str, tracks: Vec<TrackSpec>) -> Rig {
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    s.tracks = tracks;
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
    }
}

fn track(input: Input, arm: bool, monitor: bool) -> TrackSpec {
    let mut t = TrackSpec::new(vec![]);
    t.input = Some(input);
    t.arm = arm;
    t.monitor = monitor;
    t
}

impl Rig {
    /// Runs `blocks` blocks of `n` frames, returning the output.
    fn run(&mut self, blocks: usize, n: usize) -> [Vec<f32>; 2] {
        let mut out = [Vec::new(), Vec::new()];
        for _ in 0..blocks {
            let inp = input(self.fed, n);
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            self.eng.process_duplex(&inp, CH, &mut l, &mut r, None);
            self.fed += n;
            out[0].extend(l);
            out[1].extend(r);
            self.rec.service();
        }
        out
    }

    fn send(&mut self, c: Command) {
        self.ctl.send(c).ok().unwrap();
    }

    fn takes(&mut self) -> Vec<Take> {
        self.rec.service();
        let mut t = self.ctl.finished_takes();
        t.sort_by_key(|t| (t.track, t.path.clone()));
        t
    }
}

fn read(path: &PathBuf) -> (u16, Vec<f32>) {
    let mut r = hound::WavReader::open(path).unwrap();
    let ch = r.spec().channels;
    (ch, r.samples::<f32>().map(Result::unwrap).collect())
}

#[test]
fn monitoring_plays_the_input_while_stopped() {
    let mut r = rig(
        "monitor",
        vec![
            track(Input::mono(1), false, true),
            track(Input::stereo(2), false, false),
        ],
    );
    let [l, rr] = r.run(1, 64);
    // Mono input, panned centre (constant power): both sides at 0.707.
    let g = std::f32::consts::FRAC_1_SQRT_2;
    for i in 0..64 {
        assert!((l[i] - value(i, 1) * g).abs() < 1e-9, "frame {i}");
        assert!((rr[i] - value(i, 1) * g).abs() < 1e-9);
    }
    // The input meter shows both inputs, monitored or not.
    let s = r.ctl.status();
    assert!((s.take_input_peak(0).0 - value(63, 1)).abs() < 1e-9);
    assert!((s.take_input_peak(1).1 - value(63, 3)).abs() < 1e-9);
    assert!(r.takes().is_empty());
}

#[test]
fn armed_tracks_record_exactly_what_came_in() {
    let mut r = rig(
        "basic",
        vec![
            track(Input::mono(1), true, false),
            track(Input::stereo(2), true, false),
            track(Input::mono(0), false, false),
        ],
    );
    r.send(Command::SetRecording(true));
    r.send(Command::Seek(Frames(10_000)));
    r.send(Command::Play);
    r.run(10, 300);
    r.send(Command::Stop);
    r.run(1, 300);
    let takes = r.takes();
    assert_eq!(takes.len(), 2, "{takes:?}");

    let t = &takes[0];
    assert_eq!(
        (t.track, t.start, t.frames, t.channels),
        (0, Frames(10_000), 3000, 1)
    );
    assert!(t.error.is_none());
    let (ch, s) = read(&t.path);
    assert_eq!(ch, 1);
    let want: Vec<f32> = (0..3000).map(|f| value(f, 1)).collect();
    assert_eq!(s, want);

    let t = &takes[1];
    assert_eq!((t.track, t.frames, t.channels), (1, 3000, 2));
    let (_, s) = read(&t.path);
    let want: Vec<f32> = (0..3000).flat_map(|f| [value(f, 2), value(f, 3)]).collect();
    assert_eq!(s, want);
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn recording_starts_after_the_count_in_on_the_exact_frame() {
    let mut r = rig("count-in", vec![track(Input::mono(0), true, false)]);
    let bar = 4 * RATE as usize / 2; // 4/4 at 120 bpm
    r.send(Command::SetRecording(true));
    r.send(Command::Seek(Frames(bar as i64)));
    r.send(Command::CountIn { bars: 1 });
    // One bar of count-in, then 1000 frames of recording; blocks of 700
    // so the start falls mid-block.
    let blocks = (bar + 1000).div_ceil(700);
    r.run(blocks, 700);
    r.send(Command::Stop);
    r.run(1, 700);
    let takes = r.takes();
    assert_eq!(takes.len(), 1);
    let t = &takes[0];
    assert_eq!(t.start, Frames(bar as i64));
    assert_eq!(t.frames, blocks * 700 - bar);
    // The first recorded sample is the input that arrived as playback
    // reached the bar, not during the count-in.
    let (_, s) = read(&t.path);
    assert_eq!(s[0], value(bar, 0));
    // Placed earlier by the interface's round trip.
    assert_eq!(t.clip_start(Frames(250)), Frames(bar as i64 - 250));
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn each_loop_pass_is_a_new_take() {
    let mut r = rig("loop", vec![track(Input::mono(0), true, false)]);
    r.send(Command::SetLoop(Some((Frames(0), Frames(1000)))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run(10, 256); // 2560 frames: 1000 + 1000 + 560
    r.send(Command::SetRecording(false));
    r.run(1, 256);
    let takes = r.takes();
    let got: Vec<_> = takes.iter().map(|t| (t.start, t.frames)).collect();
    assert_eq!(
        got,
        [(Frames(0), 1000), (Frames(0), 1000), (Frames(0), 560)]
    );
    // Each pass holds its own audio.
    let (_, s) = read(&takes[1].path);
    assert_eq!(s[0], value(1000, 0));
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn a_take_in_progress_is_readable() {
    // As if the app died mid-take: the file on disk, while still being
    // written, reads back up to the last checkpoint.
    let mut r = rig("crash", vec![track(Input::mono(0), true, false)]);
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run(10, 480);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    r.run(1, 480);
    let path = std::fs::read_dir(&r.dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let (_, s) = read(&path);
    assert!(s.len() >= 4800, "only {} frames readable", s.len());
    assert_eq!(
        s[..4800],
        (0..4800).map(|f| value(f, 0)).collect::<Vec<_>>()
    );
    drop(r.rec);
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn recording_and_monitoring_never_allocate_on_the_audio_thread() {
    let mut r = rig(
        "rt",
        vec![
            track(Input::mono(0), true, true),
            track(Input::stereo(2), true, true),
        ],
    );
    r.send(Command::SetLoop(Some((Frames(0), Frames(20_000)))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    let rec = r.rec.spawn();
    let mut eng = r.eng;
    let audio = std::thread::spawn(move || {
        let inp = input(0, 256);
        let (mut l, mut ri) = (vec![0.0; 256], vec![0.0; 256]);
        let rt = rt_check::RtGuard::enter();
        for _ in 0..400 {
            eng.process_duplex(&inp, CH, &mut l, &mut ri, None);
        }
        drop(rt);
        rt_check::violations()
    });
    assert_eq!(audio.join().unwrap(), 0, "allocations on the audio thread");
    r.ctl.send(Command::SetRecording(false)).ok();
    drop(rec);
    assert_eq!(r.ctl.status().rec_dropped(), 0);
    let _ = std::fs::remove_dir_all(&r.dir);
}
