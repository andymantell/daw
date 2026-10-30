// SPDX-License-Identifier: GPL-3.0-or-later
//! The monitor section (roadmap 2.3): headphone mixes, talkback and the
//! control room, proved against the main mix by null tests.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::sync::Arc;

use daw_engine::engine::{self, Command};
use daw_engine::graph::{
    AudioData, ClipSpec, Insert, InsertSpec, Mix, Output, Session, TrackSpec, db_to_gain,
};
use daw_engine::metronome::Metronome;
use daw_engine::monitor::{ControlRoom, CueBus, CueSend, Listen, MAX_CUES, Talkback};
use daw_engine::time::{Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const LEN: usize = 4000;
const BLOCK: usize = 128;
const SETTLE: usize = 1000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

fn noise(seed: u64, n: usize) -> Arc<AudioData> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let samples = (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        })
        .collect();
    Arc::new(AudioData {
        channels: vec![samples],
        rate: RATE,
    })
}

fn track(seed: u64) -> TrackSpec {
    TrackSpec::new(vec![ClipSpec::new(Ticks(0), noise(seed, LEN))])
}

/// Delays by what it reports, like a plugin with latency.
struct Delay {
    line: [Vec<f32>; 2],
    at: usize,
}

impl Insert for Delay {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let d = self.line[0].len();
        for i in 0..left.len() {
            left[i] = std::mem::replace(&mut self.line[0][self.at], left[i]);
            right[i] = std::mem::replace(&mut self.line[1][self.at], right[i]);
            self.at = (self.at + 1) % d;
        }
    }
}

fn delay(id: u64, d: usize) -> (InsertSpec, (u64, Box<dyn Insert>)) {
    (
        InsertSpec {
            id,
            bypass: false,
            latency: d as u32,
            sidechain: None,
        },
        (
            id,
            Box::new(Delay {
                line: [vec![0.0; d], vec![0.0; d]],
                at: 0,
            }),
        ),
    )
}

/// A headphone mix on outputs `2 + 2k`, `3 + 2k`.
fn cue(k: usize) -> CueBus {
    CueBus {
        on: true,
        click_db: None,
        out: Some((2 + 2 * k as u16, 3 + 2 * k as u16)),
        ..CueBus::default()
    }
}

/// What came out of a run: the main pair, and each headphone mix.
struct Heard {
    main: [Vec<f32>; 2],
    cues: Vec<[Vec<f32>; 2]>,
}

/// Plays `s` for [`LEN`] frames with `cmds` sent first, `input` (two
/// channels, interleaved, repeated) as the device input.
fn play(s: &Session, fx: Vec<(u64, Box<dyn Insert>)>, cmds: Vec<Command>, input: &[f32]) -> Heard {
    let mut fx = fx;
    let g = s.compile_with(BLOCK, &mut fx).unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    ctl.set_graph(g).ok().unwrap();
    for c in cmds {
        ctl.send(c).ok().unwrap();
    }
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let inp: Vec<f32> = input.iter().copied().cycle().take(BLOCK * 2).collect();
    // A block stopped first, so the mixes have faded in from silence.
    eng.process_duplex(&inp, 2, &mut l, &mut r, None);
    ctl.send(Command::Play).ok().unwrap();
    let mut heard = Heard {
        main: [Vec::new(), Vec::new()],
        cues: (0..MAX_CUES).map(|_| [Vec::new(), Vec::new()]).collect(),
    };
    for _ in 0..LEN / BLOCK {
        eng.process_duplex(&inp, 2, &mut l, &mut r, None);
        heard.main[0].extend_from_slice(&l);
        heard.main[1].extend_from_slice(&r);
        let mut got = [false; 2 * MAX_CUES];
        eng.each_cue_out(&mut |ch, x| {
            let (k, side) = ((ch - 2) / 2, (ch - 2) % 2);
            heard.cues[k][side].extend_from_slice(x);
            got[ch - 2] = true;
        });
        for (k, c) in heard.cues.iter_mut().enumerate() {
            for side in 0..2 {
                if !got[2 * k + side] {
                    c[side].extend(std::iter::repeat_n(0.0, BLOCK));
                }
            }
        }
    }
    heard
}

fn assert_null(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2], what: &str) {
    for ch in 0..2 {
        for i in SETTLE..a[ch].len() {
            let (x, y) = (a[ch][i], b[ch][i]);
            assert!(
                (x - y).abs() < 1e-6,
                "{what}: channel {ch} frame {i}: {x} vs {y}"
            );
        }
    }
    assert!(
        a[0][SETTLE..].iter().any(|x| x.abs() > 0.01),
        "{what}: not silent"
    );
}

fn silent(a: &[Vec<f32>; 2]) -> bool {
    a.iter().all(|c| c.iter().all(|&x| x == 0.0))
}

#[test]
fn a_headphone_mix_has_its_own_levels() {
    // The main mix has both tracks; the headphones only the first, at
    // −6 dB.
    let mut s = session();
    let mut a = track(1);
    a.cues[0] = CueSend::at(-6.0);
    s.tracks.push(a);
    s.tracks.push(track(2));
    let heard = play(
        &s,
        vec![],
        vec![Command::SetCueBus {
            cue: 0,
            bus: cue(0),
        }],
        &[],
    );

    let mut alone = session();
    let mut a = track(1);
    a.mix.gain_db = -6.0;
    alone.tracks.push(a);
    let reference = play(&alone, vec![], vec![], &[]);

    assert_null(&heard.cues[0], &reference.main, "own levels");
    assert!(silent(&heard.cues[1]), "a mix that is off is silent");
}

#[test]
fn the_main_mix_leaves_the_headphones_alone() {
    // Turning a track down, muting it, or soloing another in the main
    // mix doesn't change what the performer hears.
    let mut plain = session();
    let mut a = track(3);
    a.cues[0] = CueSend::at(0.0);
    let mut b = track(4);
    b.cues[0] = CueSend::at(-3.0);
    plain.tracks.push(a.clone());
    plain.tracks.push(b.clone());

    let mut changed = session();
    a.mix = Mix {
        gain_db: -20.0,
        mute: true,
        ..Mix::default()
    };
    b.mix.solo = true;
    changed.tracks.push(a);
    changed.tracks.push(b);

    let cmds = || {
        vec![Command::SetCueBus {
            cue: 0,
            bus: cue(0),
        }]
    };
    let x = play(&plain, vec![], cmds(), &[]);
    let y = play(&changed, vec![], cmds(), &[]);
    assert_null(&x.cues[0], &y.cues[0], "main mix changes");
    assert!(!silent(&y.main));
}

#[test]
fn headphones_stay_in_time_with_the_speakers() {
    // A track through a bus with a slow effect: the headphone mix,
    // taking the bus, must be in time with the main mix.
    let mut s = session();
    let (ins, fx) = delay(1, 300);
    let mut a = track(5);
    a.output = Output::Track(2);
    s.tracks.push(a);
    let mut b = track(6);
    b.cues[0] = CueSend::at(0.0);
    s.tracks.push(b);
    let mut bus = TrackSpec::new(Vec::new());
    bus.inserts = vec![ins];
    bus.cues[0] = CueSend::at(0.0);
    s.tracks.push(bus);
    let heard = play(
        &s,
        vec![fx],
        vec![Command::SetCueBus {
            cue: 0,
            bus: cue(0),
        }],
        &[],
    );
    // Same tracks at the same levels: the headphones are the main mix.
    assert_null(&heard.cues[0], &heard.main, "lined up");
}

#[test]
fn a_mix_can_follow_the_main_mix() {
    let mut s = session();
    s.tracks.push(track(7));
    s.tracks.push(track(8));
    let bus = CueBus {
        follow_main: true,
        gain_db: -6.0,
        ..cue(1)
    };
    let heard = play(&s, vec![], vec![Command::SetCueBus { cue: 1, bus }], &[]);
    let g = db_to_gain(-6.0);
    let scaled = heard
        .main
        .clone()
        .map(|c| c.iter().map(|x| x * g).collect());
    assert_null(&heard.cues[1], &scaled, "follows the main mix");
}

#[test]
fn the_click_goes_to_the_headphones_at_its_own_level() {
    let mut s = session();
    s.tracks.push(TrackSpec::new(Vec::new()));
    let m = Metronome {
        enabled: true,
        ..Metronome::default()
    };
    let loud = CueBus {
        click_db: Some(0.0),
        ..cue(0)
    };
    let quiet = CueBus {
        click_db: Some(-12.0),
        ..cue(1)
    };
    let heard = play(
        &s,
        vec![],
        vec![
            Command::SetMetronome(m),
            Command::SetCueBus { cue: 0, bus: loud },
            Command::SetCueBus { cue: 1, bus: quiet },
        ],
        &[],
    );
    // The first click is on frame 0, in the main mix too.
    let peak = |c: &[f32]| c.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    let main = peak(&heard.main[0]);
    assert!(main > 0.1);
    assert!((peak(&heard.cues[0][0]) - main).abs() < 1e-6);
    assert!((peak(&heard.cues[1][0]) - main * db_to_gain(-12.0)).abs() < 1e-6);
}

#[test]
fn talkback_goes_to_the_headphones_and_dims_the_speakers() {
    let mut s = session();
    s.tracks.push(track(9));
    let room = ControlRoom {
        talkback: Talkback {
            input: Some(1),
            on: true,
            gain_db: 0.0,
            dim_db: 20.0,
        },
        ..ControlRoom::default()
    };
    let deaf = CueBus {
        talkback: false,
        ..cue(1)
    };
    let heard = play(
        &s,
        vec![],
        vec![
            Command::SetCueBus {
                cue: 0,
                bus: cue(0),
            },
            Command::SetCueBus { cue: 1, bus: deaf },
            Command::SetControlRoom(room),
        ],
        // Input 1, the microphone, holds a steady 0.25.
        &[0.0, 0.25],
    );
    // Neither mix has the track; one hears the microphone.
    assert!(heard.cues[0][0][SETTLE..].iter().all(|&x| x == 0.25));
    assert!(silent(&heard.cues[1]));
    let reference = play(&s, vec![], vec![], &[]);
    let g = db_to_gain(-20.0);
    let dimmed = reference
        .main
        .clone()
        .map(|c| c.iter().map(|x| x * g).collect());
    assert_null(&heard.main, &dimmed, "speakers dimmed, no microphone");
}

#[test]
fn the_speakers_can_listen_to_a_headphone_mix() {
    let mut s = session();
    let mut a = track(10);
    a.cues[2] = CueSend {
        pan: -1.0,
        ..CueSend::at(0.0)
    };
    s.tracks.push(a);
    s.tracks.push(track(11));
    let room = ControlRoom {
        listen: Listen::Cue(2),
        ..ControlRoom::default()
    };
    let heard = play(
        &s,
        vec![],
        vec![
            Command::SetCueBus {
                cue: 2,
                bus: cue(2),
            },
            Command::SetControlRoom(room),
        ],
        &[],
    );
    assert_null(&heard.main, &heard.cues[2], "listening to a mix");
    // Panned hard left in that mix.
    assert!(heard.main[1].iter().all(|&x| x == 0.0));
}

#[test]
fn changing_a_level_in_a_mix_needs_no_new_graph() {
    let mut s = session();
    s.tracks.push(track(12));
    let heard = play(
        &s,
        vec![],
        vec![
            Command::SetCueBus {
                cue: 0,
                bus: cue(0),
            },
            Command::SetCueSend {
                track: 0,
                cue: 0,
                send: CueSend::at(0.0),
            },
        ],
        &[],
    );
    assert_null(&heard.cues[0], &heard.main, "set live");
}

#[test]
fn the_monitor_section_does_not_allocate() {
    let mut s = session();
    let (ins, fx) = delay(1, 100);
    let mut a = track(13);
    a.output = Output::Track(1);
    a.cues = [CueSend::at(0.0); MAX_CUES];
    s.tracks.push(a);
    let mut bus = TrackSpec::new(Vec::new());
    bus.inserts = vec![ins];
    s.tracks.push(bus);
    let mut fx = vec![fx];
    let (mut ctl, mut eng) = engine::new(BLOCK);
    ctl.set_graph(s.compile_with(BLOCK, &mut fx).unwrap())
        .ok()
        .unwrap();
    for k in 0..MAX_CUES {
        let bus = CueBus {
            follow_main: k % 2 == 1,
            click_db: Some(-6.0),
            ..cue(k)
        };
        ctl.send(Command::SetCueBus { cue: k, bus }).ok().unwrap();
    }
    let mut room = ControlRoom {
        mono: true,
        dim: true,
        listen: Listen::Cue(1),
        ..ControlRoom::default()
    };
    room.talkback.input = Some(0);
    room.talkback.on = true;
    room.speakers[1].out = Some((8, 9));
    room.active = 1;
    ctl.send(Command::SetControlRoom(room)).ok().unwrap();
    ctl.send(Command::SetMetronome(Metronome {
        enabled: true,
        ..Metronome::default()
    }))
    .ok()
    .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let inp = vec![0.1; BLOCK * 2];
    eng.process_duplex(&inp, 2, &mut l, &mut r, None);
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        for _ in 0..20 {
            eng.process_duplex(&inp, 2, &mut l, &mut r, None);
            let mut sum = 0.0;
            eng.each_cue_out(&mut |_, x| sum += x[0]);
            assert!(sum.is_finite());
        }
    }
    assert_eq!(rt_check::violations(), before);
    assert_eq!(eng.main_out(), Some((8, 9)));
}
