// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice speed and transpose through the engine (roadmap 1.4).

#[path = "common/rt_check.rs"]
mod rt_check;

use std::sync::Arc;

use daw_engine::engine::{self, Command};
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::metronome::Metronome;
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};
use daw_engine::tuner::{Detector, window};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

fn tone(hz: f32, seconds: f32) -> Arc<AudioData> {
    let n = (RATE as f32 * seconds) as usize;
    Arc::new(AudioData {
        channels: vec![
            (0..n)
                .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin())
                .collect(),
        ],
        rate: RATE,
    })
}

fn session(clip: Arc<AudioData>) -> Session {
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    s.tracks
        .push(TrackSpec::new(vec![ClipSpec::new(Ticks(0), clip)]));
    s
}

struct Rig {
    ctl: engine::Controller,
    eng: engine::Engine,
}

impl Rig {
    fn new(s: &Session) -> Rig {
        let (mut ctl, eng) = engine::new(BLOCK);
        ctl.set_graph(s.compile(BLOCK).unwrap()).ok().unwrap();
        Rig { ctl, eng }
    }
    fn send(&mut self, c: Command) {
        self.ctl.send(c).ok().unwrap();
    }
    fn run(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
        for _ in 0..frames / BLOCK {
            self.eng.process(&mut l, &mut r);
            out.extend_from_slice(&l);
        }
        out
    }
    fn pos(&self) -> i64 {
        self.ctl.status().position().0
    }
}

fn pitch(x: &[f32]) -> f32 {
    Detector::new(RATE)
        .detect(&x[x.len() - window(RATE)..])
        .expect("a pitch")
}

fn cents(a: f32, b: f32) -> f32 {
    1200.0 * (a / b).log2()
}

#[test]
fn half_speed_plays_half_as_far_at_the_same_pitch() {
    let mut rig = Rig::new(&session(tone(220.0, 4.0)));
    rig.send(Command::SetPractice {
        speed: 0.5,
        semitones: 0.0,
    });
    rig.send(Command::Play);
    let out = rig.run(BLOCK * 200);
    assert_eq!(rig.pos(), (BLOCK * 100) as i64);
    assert!(cents(pitch(&out), 220.0).abs() < 3.0);
}

#[test]
fn transpose_moves_pitch_and_keeps_time() {
    let mut rig = Rig::new(&session(tone(220.0, 4.0)));
    rig.send(Command::SetPractice {
        speed: 1.0,
        semitones: -1.0,
    });
    rig.send(Command::Play);
    let out = rig.run(BLOCK * 200);
    assert_eq!(rig.pos(), (BLOCK * 200) as i64);
    let want = 220.0 * 2f32.powf(-1.0 / 12.0);
    assert!(cents(pitch(&out), want).abs() < 3.0, "{}", pitch(&out));
}

#[test]
fn back_to_normal_is_exact_again() {
    let s = session(tone(220.0, 4.0));
    let mut rig = Rig::new(&s);
    rig.send(Command::SetPractice {
        speed: 0.75,
        semitones: 2.0,
    });
    rig.send(Command::Play);
    rig.run(BLOCK * 40);
    rig.send(Command::SetPractice {
        speed: 1.0,
        semitones: 0.0,
    });
    rig.send(Command::Seek(Frames(48_000)));
    let out = rig.run(BLOCK * 4);
    let mono = tone(220.0, 4.0);
    // Centre pan on a mono track: −3 dB.
    let g = std::f32::consts::FRAC_1_SQRT_2;
    for (i, x) in out.iter().enumerate() {
        assert!(
            (x - mono.channels[0][48_000 + i] * g).abs() < 1e-6,
            "frame {i}"
        );
    }
}

#[test]
fn loops_wrap_and_clicks_follow_the_slower_beat() {
    let mut s = session(tone(220.0, 4.0));
    s.tracks[0].mix.mute = true;
    let mut rig = Rig::new(&s);
    // Loop the first bar (2 s at 120 BPM); clicks every beat (0.5 s).
    rig.send(Command::SetLoop(Some((Frames(0), Frames(96_000)))));
    rig.send(Command::SetMetronome(Metronome {
        enabled: true,
        ..Metronome::default()
    }));
    rig.send(Command::SetPractice {
        speed: 0.8,
        semitones: 0.0,
    });
    rig.send(Command::Play);
    let out = rig.run(BLOCK * 1000);
    // Click onsets: a beat is 24,000 timeline frames, 30,000 output frames.
    let onsets: Vec<usize> = (1..out.len())
        .filter(|&i| {
            out[i] != 0.0
                && out[i - 1] == 0.0
                && (i < 200 || out[i - 200..i - 1].iter().all(|&x| x == 0.0))
        })
        .collect();
    assert!(onsets.len() >= 8, "{onsets:?}");
    for w in onsets.windows(2) {
        let gap = w[1] - w[0];
        assert!(gap.abs_diff(30_000) <= 1, "{onsets:?}");
    }
    // 256,000 output frames at 80% is 204,800 of timeline: round the
    // 96,000-frame loop twice, and still inside it.
    assert!(rig.pos() < 96_000);
    assert_eq!(rig.ctl.status().loops(), 2);
}

#[test]
fn stretching_does_not_allocate() {
    let mut rig = Rig::new(&session(tone(220.0, 4.0)));
    rig.send(Command::SetPractice {
        speed: 0.6,
        semitones: 3.5,
    });
    rig.send(Command::Play);
    rig.run(BLOCK * 4);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        for _ in 0..100 {
            rig.eng.process(&mut l, &mut r);
        }
    }
    assert_eq!(rt_check::violations(), before);
}
