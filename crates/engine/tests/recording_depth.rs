// SPDX-License-Identifier: GPL-3.0-or-later
//! Recording depth (roadmap 2.2): punch in and out, pre-roll and
//! post-roll, many inputs at once, and retrospective capture.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::path::PathBuf;

use daw_engine::engine::{self, Command, Controller, Engine};
use daw_engine::graph::{Input, Session, TrackSpec};
use daw_engine::record::{Punch, Recorder, Take};
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};

const RATE: u32 = 48_000;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-depth-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A device input of `ch` channels where every sample says which frame
/// and channel it is.
fn input(ch: usize, from: usize, frames: usize) -> Vec<f32> {
    (from..from + frames)
        .flat_map(|f| (0..ch).map(move |c| value(f, c)))
        .collect()
}

fn value(frame: usize, ch: usize) -> f32 {
    (frame as f32 + ch as f32 / 100.0) / 1e6
}

fn map() -> TempoMap {
    TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON)
}

/// Frames in one bar: 4/4 at 120 bpm.
const BAR: i64 = 2 * RATE as i64;

struct Rig {
    ctl: Controller,
    eng: Engine,
    rec: Recorder,
    ch: usize,
    fed: usize,
    dir: PathBuf,
}

fn rig(name: &str, ch: usize, tracks: Vec<TrackSpec>) -> Rig {
    let mut s = Session::new(map());
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
        ch,
        fed: 0,
        dir,
    }
}

fn armed(input: Input) -> TrackSpec {
    let mut t = TrackSpec::new(vec![]);
    t.input = Some(input);
    t.arm = true;
    t
}

impl Rig {
    fn run(&mut self, blocks: usize, n: usize) -> [Vec<f32>; 2] {
        let mut out = [Vec::new(), Vec::new()];
        for _ in 0..blocks {
            let inp = input(self.ch, self.fed, n);
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            self.eng.process_duplex(&inp, self.ch, &mut l, &mut r, None);
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

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn read(path: &PathBuf) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_punch_range_records_only_inside_it_and_ends_the_take_there() {
    let mut r = rig("punch", 2, vec![armed(Input::mono(1))]);
    r.send(Command::SetPunch(Some((Frames(1000), Frames(3000)))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    // Blocks of 700 so both ends fall mid-block.
    r.run(5, 700);
    // Past the punch-out and still playing: the take is already done.
    assert!(r.ctl.status().playing());
    let takes = r.takes();
    assert_eq!(takes.len(), 1, "{takes:?}");
    let t = &takes[0];
    assert_eq!((t.start, t.frames), (Frames(1000), 2000));
    let want: Vec<f32> = (1000..3000).map(|f| value(f, 1)).collect();
    assert_eq!(read(&t.path), want);
    // Nothing more while it plays on.
    r.run(3, 700);
    assert!(r.takes().is_empty());
}

#[test]
fn punching_into_a_loop_makes_one_take_per_pass() {
    let mut r = rig("punch-loop", 1, vec![armed(Input::mono(0))]);
    r.send(Command::SetLoop(Some((Frames(0), Frames(2000)))));
    r.send(Command::SetPunch(Some((Frames(500), Frames(1500)))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run(20, 300); // 6000 frames: three passes
    let takes = r.takes();
    let got: Vec<_> = takes.iter().map(|t| (t.start, t.frames)).collect();
    assert_eq!(got, [(Frames(500), 1000); 3]);
    assert_eq!(read(&takes[2].path)[0], value(4500, 0));
}

#[test]
fn punching_in_and_out_by_hand_while_playing() {
    let mut r = rig("manual", 1, vec![armed(Input::mono(0))]);
    r.send(Command::Play);
    r.run(4, 256);
    r.send(Command::SetRecording(true));
    r.run(4, 256);
    r.send(Command::SetRecording(false));
    r.run(4, 256);
    let takes = r.takes();
    assert_eq!(takes.len(), 1);
    assert_eq!((takes[0].start, takes[0].frames), (Frames(1024), 1024));
    assert_eq!(read(&takes[0].path)[0], value(1024, 0));
}

#[test]
fn playback_stops_by_itself_on_the_exact_frame() {
    let mut r = rig("stop-at", 1, vec![]);
    r.send(Command::Seek(Frames(100)));
    r.send(Command::StopAt(Some(Frames(1000))));
    r.send(Command::Play);
    r.run(3, 512);
    assert!(!r.ctl.status().playing());
    assert_eq!(r.ctl.status().position(), Frames(1000));
    // Playing again from there plays on: the stop was used up.
    r.send(Command::Play);
    r.run(2, 512);
    assert!(r.ctl.status().playing());
    assert_eq!(r.ctl.status().position(), Frames(2024));
}

#[test]
fn pre_roll_and_post_roll_are_whole_bars() {
    let m = map();
    let p = Punch {
        start: Frames(3 * BAR),
        end: Frames(5 * BAR),
        pre_roll: 2,
        post_roll: 1,
    };
    assert_eq!(p.play_from(&m), Frames(BAR));
    assert_eq!(p.stop_at(&m, Frames(250)), Frames(6 * BAR));
    // No post-roll still waits for the last input to arrive.
    let tight = Punch { post_roll: 0, ..p };
    assert_eq!(tight.stop_at(&m, Frames(250)), Frames(5 * BAR + 250));
    // Pre-roll stops at the beginning of the song.
    let early = Punch { pre_roll: 8, ..p };
    assert_eq!(early.play_from(&m), Frames(0));
    assert_eq!(
        p.capture_range(Frames(250)),
        (Frames(3 * BAR + 250), Frames(5 * BAR + 250))
    );
}

#[test]
fn a_punched_take_with_pre_and_post_roll_lines_up() {
    // The whole run as the app does it: seek back for the pre-roll, punch
    // later by the round trip, stop after the post-roll.
    let offset = Frames(250);
    let p = Punch {
        start: Frames(BAR),
        end: Frames(2 * BAR),
        pre_roll: 1,
        post_roll: 1,
    };
    let mut r = rig("roll", 1, vec![armed(Input::mono(0))]);
    r.send(Command::SetPunch(Some(p.capture_range(offset))));
    r.send(Command::StopAt(Some(p.stop_at(&map(), offset))));
    r.send(Command::Seek(p.play_from(&map())));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run((3 * BAR as usize).div_ceil(512) + 1, 512);
    assert!(!r.ctl.status().playing());
    assert_eq!(r.ctl.status().position(), Frames(3 * BAR));
    let takes = r.takes();
    assert_eq!(takes.len(), 1);
    // Placed by the round trip, it covers the punch range exactly.
    assert_eq!(takes[0].clip_start(offset), p.start);
    assert_eq!(takes[0].frames as i64, p.end.0 - p.start.0);
}

#[test]
fn sixteen_inputs_record_at_once() {
    // Sixteen mono tracks and a stereo one, from an 18-channel interface.
    let mut tracks: Vec<_> = (0..16).map(|c| armed(Input::mono(c))).collect();
    tracks.push(armed(Input::stereo(16)));
    let mut r = rig("many", 18, tracks);
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    // Two seconds, in blocks of 480.
    r.run(200, 480);
    r.send(Command::Stop);
    r.run(1, 480);
    assert_eq!(r.ctl.status().rec_dropped(), 0);
    let takes = r.takes();
    assert_eq!(takes.len(), 17);
    for t in &takes[..16] {
        assert_eq!((t.start, t.frames, t.channels), (Frames(0), 96_000, 1));
        let want: Vec<f32> = (0..96_000).map(|f| value(f, t.track)).collect();
        assert_eq!(read(&t.path), want, "track {}", t.track);
    }
    let st = &takes[16];
    assert_eq!((st.frames, st.channels), (96_000, 2));
    let want: Vec<f32> = (0..96_000)
        .flat_map(|f| [value(f, 16), value(f, 17)])
        .collect();
    assert_eq!(read(&st.path), want);
}

#[test]
fn retrospective_capture_keeps_what_was_played_but_not_recorded() {
    let mut r = rig(
        "retro",
        2,
        vec![armed(Input::mono(0)), armed(Input::mono(1))],
    );
    r.ctl.set_capture(60).ok().unwrap();
    r.send(Command::Seek(Frames(5000)));
    r.send(Command::Play);
    r.run(10, 300);
    assert_eq!(r.ctl.captured_frames(), 3000);
    // Nothing is written until asked.
    assert!(r.takes().is_empty());
    r.ctl.keep_capture();
    let takes = r.takes();
    assert_eq!(takes.len(), 2);
    for t in &takes {
        assert_eq!((t.start, t.frames), (Frames(5000), 3000));
        let want: Vec<f32> = (0..3000).map(|f| value(f, t.track)).collect();
        assert_eq!(read(&t.path), want);
        assert!(
            t.path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("kept")
        );
    }
    // Kept once: it is gone from memory.
    assert_eq!(r.ctl.captured_frames(), 0);
    r.ctl.keep_capture();
    assert!(r.takes().is_empty());
}

#[test]
fn capture_keeps_each_pass_separately_and_nothing_while_stopped() {
    let mut r = rig("passes", 1, vec![armed(Input::mono(0))]);
    r.ctl.set_capture(60).ok().unwrap();
    r.run(2, 500);
    r.send(Command::SetLoop(Some((Frames(0), Frames(1000)))));
    r.send(Command::Play);
    r.run(5, 500);
    r.send(Command::Stop);
    r.run(2, 500);
    r.ctl.keep_capture();
    let takes = r.takes();
    let got: Vec<_> = takes.iter().map(|t| (t.start, t.frames)).collect();
    assert_eq!(
        got,
        [(Frames(0), 1000), (Frames(0), 1000), (Frames(0), 500)]
    );
    assert_eq!(read(&takes[1].path)[0], value(2000, 0));
}

#[test]
fn capture_keeps_only_the_last_so_many_seconds() {
    let mut r = rig("window", 1, vec![armed(Input::mono(0))]);
    r.ctl.set_capture(1).ok().unwrap();
    r.send(Command::Play);
    r.run(150, 480); // 1.5 seconds
    assert_eq!(r.ctl.captured_frames(), RATE as usize);
    r.ctl.keep_capture();
    let t = &r.takes()[0];
    assert_eq!((t.start, t.frames), (Frames(24_000), 48_000));
    assert_eq!(read(&t.path)[0], value(24_000, 0));
}

#[test]
fn capture_skips_what_was_recorded_and_keeps_a_missed_punch() {
    let mut r = rig("missed", 1, vec![armed(Input::mono(0))]);
    r.ctl.set_capture(60).ok().unwrap();
    r.send(Command::SetPunch(Some((Frames(1000), Frames(2000)))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    r.run(6, 500);
    let recorded = r.takes();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        (recorded[0].start, recorded[0].frames),
        (Frames(1000), 1000)
    );
    r.ctl.keep_capture();
    let kept: Vec<_> = r.takes().iter().map(|t| (t.start, t.frames)).collect();
    assert_eq!(kept, [(Frames(0), 1000), (Frames(2000), 1000)]);
}

#[test]
fn capture_off_keeps_nothing_and_forgets() {
    let mut r = rig("off", 1, vec![armed(Input::mono(0))]);
    r.ctl.set_capture(60).ok().unwrap();
    r.send(Command::Play);
    r.run(4, 500);
    r.ctl.set_capture(0).ok().unwrap();
    r.run(4, 500);
    assert_eq!(r.ctl.captured_frames(), 0);
    r.ctl.keep_capture();
    assert!(r.takes().is_empty());
}

#[test]
fn punch_post_roll_and_capture_never_allocate_on_the_audio_thread() {
    let mut r = rig(
        "rt",
        4,
        vec![armed(Input::mono(0)), armed(Input::stereo(2))],
    );
    r.ctl.set_capture(30).ok().unwrap();
    r.send(Command::SetLoop(Some((Frames(0), Frames(20_000)))));
    r.send(Command::SetPunch(Some((Frames(3000), Frames(9000)))));
    r.send(Command::StopAt(Some(Frames(60_000))));
    r.send(Command::SetRecording(true));
    r.send(Command::Play);
    let Rig { ctl, eng, rec, .. } = &mut r;
    let rec = std::mem::replace(rec, ctl_recorder_stub());
    let rec = rec.spawn();
    let mut eng = std::mem::replace(eng, engine::new(512).1);
    let audio = std::thread::spawn(move || {
        let inp = input(4, 0, 256);
        let (mut l, mut ri) = (vec![0.0; 256], vec![0.0; 256]);
        let rt = rt_check::RtGuard::enter();
        for _ in 0..400 {
            eng.process_duplex(&inp, 4, &mut l, &mut ri, None);
        }
        drop(rt);
        rt_check::violations()
    });
    assert_eq!(audio.join().unwrap(), 0, "allocations on the audio thread");
    drop(rec);
    assert_eq!(ctl.status().rec_dropped(), 0);
    assert_eq!(ctl.status().capture_dropped(), 0);
}

/// A recorder to leave in the rig once the real one has moved to its own
/// thread.
fn ctl_recorder_stub() -> Recorder {
    engine::new(16).0.take_recorder().unwrap()
}
