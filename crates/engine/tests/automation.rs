// SPDX-License-Identifier: GPL-3.0-or-later
//! Automation (roadmap 2.6): lanes on the volume, pan, a send and an
//! effect's setting are followed by playback and export alike, a lane
//! being written leaves its control in charge, and reading lanes costs
//! the audio thread no allocation.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::sync::Arc;

use daw_engine::automation::{Curve, LaneSpec, Target};
use daw_engine::engine::{self, Command};
use daw_engine::graph::{
    AudioData, ClipSpec, Insert, InsertSpec, Mix, SendSpec, Session, TrackSpec, db_to_gain,
};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const LEN: usize = 4800;
const BLOCK: usize = 128;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

/// A steady mono signal, so a level is easy to read off.
fn steady(n: usize) -> Arc<AudioData> {
    Arc::new(AudioData {
        channels: vec![vec![0.5; n]],
        rate: RATE,
    })
}

fn track() -> TrackSpec {
    TrackSpec::new(vec![ClipSpec::new(Ticks(0), steady(LEN))])
}

fn lane(target: Target, points: Vec<(i64, f32)>) -> LaneSpec {
    LaneSpec::new(target, Arc::new(Curve::new(points)))
}

fn render(s: &Session, fx: Vec<(u64, Box<dyn Insert>)>) -> [Vec<f32>; 2] {
    let mut fx = fx;
    let g = s.compile_with(BLOCK, &mut fx).unwrap();
    engine::render_offline(g, Frames(0), LEN, BLOCK)
}

/// The level at frame `i` relative to the source, as a gain, allowing
/// for the centre pan of a mono source (−3 dB each side).
fn gain_at(out: &[f32], i: usize) -> f32 {
    out[i] / (0.5 * std::f32::consts::FRAC_1_SQRT_2)
}

/// An insert that multiplies by its setting 0, so what it was set to
/// shows in the audio.
struct Scaler {
    gain: f32,
    set: Option<f32>,
}

impl Insert for Scaler {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let g = self.set.take().unwrap_or(self.gain);
        for s in left.iter_mut().chain(right.iter_mut()) {
            *s *= g;
        }
    }

    fn set_param(&mut self, param: u32, value: f64) {
        if param == 0 {
            self.set = Some(value as f32);
        }
    }
}

#[test]
fn a_volume_lane_moves_the_fader_along_its_line() {
    let mut s = session();
    let mut t = track();
    // Silence to unity over the first 2400 frames, then held.
    t.automation = vec![lane(Target::Gain, vec![(0, -60.0), (2400, 0.0)])];
    s.tracks.push(t);
    let [l, _] = render(&s, vec![]);
    // Near the start the fader is far down; at the end it is at unity.
    assert!(
        gain_at(&l, 200) < 0.1,
        "quiet at the start: {}",
        gain_at(&l, 200)
    );
    let mid = gain_at(&l, 1200);
    let want = db_to_gain(-30.0);
    assert!((mid - want).abs() < 0.1, "half way: {mid} vs about {want}");
    assert!(
        (gain_at(&l, LEN - 1) - 1.0).abs() < 1e-3,
        "unity at the end: {}",
        gain_at(&l, LEN - 1)
    );
    // Rising all the way: no step back.
    for i in 1..2400 {
        assert!(l[i] >= l[i - 1] - 1e-6, "a dip at frame {i}");
    }
}

#[test]
fn a_pan_lane_moves_the_sound_across() {
    let mut s = session();
    let mut t = track();
    t.automation = vec![lane(Target::Pan, vec![(0, -1.0), (4800, 1.0)])];
    s.tracks.push(t);
    let [l, r] = render(&s, vec![]);
    assert!(
        l[200] > r[200] * 10.0,
        "left at the start: {} {}",
        l[200],
        r[200]
    );
    assert!(r[LEN - 1] > l[LEN - 1] * 10.0, "right at the end");
    assert!((l[2400] - r[2400]).abs() < 0.02, "centred half way");
}

#[test]
fn a_send_lane_sets_the_send_and_a_muted_send_stays_silent() {
    let mut s = session();
    let mut t = track();
    t.mix.gain_db = -96.0; // Heard through the send only.
    t.sends = vec![SendSpec {
        pre_fader: true,
        ..SendSpec::to(1)
    }];
    t.automation = vec![lane(Target::Send(0), vec![(0, -6.0)])];
    s.tracks.push(t);
    s.tracks.push(TrackSpec::new(Vec::new()));
    let [l, _] = render(&s, vec![]);
    let want = db_to_gain(-6.0);
    assert!(
        (gain_at(&l, LEN - 1) - want).abs() < 1e-3,
        "send at -6 dB: {} vs {want}",
        gain_at(&l, LEN - 1)
    );

    s.tracks[0].sends[0].mute = true;
    let [l, _] = render(&s, vec![]);
    assert!(
        l.iter().all(|x| x.abs() < 1e-9),
        "a muted send stays silent"
    );
}

#[test]
fn an_effect_setting_lane_reaches_the_effect_each_block() {
    let mut s = session();
    let mut t = track();
    t.inserts = vec![InsertSpec {
        id: 1,
        ..InsertSpec::default()
    }];
    t.automation = vec![lane(
        Target::Insert { id: 1, param: 0 },
        vec![(0, 0.25), (4800, 0.25)],
    )];
    s.tracks.push(t);
    let scaler: Box<dyn Insert> = Box::new(Scaler {
        gain: 1.0,
        set: None,
    });
    // With an effect the track is stereo, so the centre pan loses nothing.
    let [l, _] = render(&s, vec![(1, scaler)]);
    assert!(
        (l[LEN - 1] / 0.5 - 0.25).abs() < 1e-3,
        "the effect took the lane's value: {}",
        l[LEN - 1] / 0.5
    );

    // Another setting of the same effect is left to its control.
    s.tracks[0].automation = vec![lane(Target::Insert { id: 1, param: 3 }, vec![(0, 0.25)])];
    let scaler: Box<dyn Insert> = Box::new(Scaler {
        gain: 1.0,
        set: None,
    });
    let [l, _] = render(&s, vec![(1, scaler)]);
    assert!((l[LEN - 1] / 0.5 - 1.0).abs() < 1e-3);
}

#[test]
fn a_lane_being_written_leaves_the_control_in_charge() {
    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), steady(LEN * 4))]);
    t.mix.gain_db = -6.0;
    t.automation = vec![lane(Target::Gain, vec![(0, -20.0)])];
    s.tracks.push(t);
    let g = s.compile(BLOCK).unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    ctl.set_graph(g).ok().unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    for _ in 0..10 {
        eng.process(&mut l, &mut r);
    }
    let want = db_to_gain(-20.0);
    assert!(
        (gain_at(&l, BLOCK - 1) - want).abs() < 1e-3,
        "the line plays"
    );

    // Touched: the fader (at -6 dB) plays instead.
    ctl.send(Command::SetAutoLive {
        track: 0,
        target: Target::Gain,
        live: true,
    })
    .ok()
    .unwrap();
    for _ in 0..10 {
        eng.process(&mut l, &mut r);
    }
    let want = db_to_gain(-6.0);
    assert!(
        (gain_at(&l, BLOCK - 1) - want).abs() < 1e-3,
        "the fader plays while written: {}",
        gain_at(&l, BLOCK - 1)
    );
    // The fader still moves while written.
    ctl.send(Command::SetMix {
        track: 0,
        mix: Mix {
            gain_db: 0.0,
            ..Mix::default()
        },
    })
    .ok()
    .unwrap();
    for _ in 0..10 {
        eng.process(&mut l, &mut r);
    }
    assert!((gain_at(&l, BLOCK - 1) - 1.0).abs() < 1e-3);

    // Let go: back to the line.
    ctl.send(Command::SetAutoLive {
        track: 0,
        target: Target::Gain,
        live: false,
    })
    .ok()
    .unwrap();
    for _ in 0..10 {
        eng.process(&mut l, &mut r);
    }
    let want = db_to_gain(-20.0);
    assert!(
        (gain_at(&l, BLOCK - 1) - want).abs() < 1e-3,
        "back to the line: {} vs {want}",
        gain_at(&l, BLOCK - 1)
    );
}

#[test]
fn an_empty_lane_changes_nothing() {
    let mut s = session();
    let mut t = track();
    t.mix.gain_db = -6.0;
    t.automation = vec![lane(Target::Gain, vec![])];
    s.tracks.push(t);
    let [l, _] = render(&s, vec![]);
    let want = db_to_gain(-6.0);
    assert!((gain_at(&l, LEN - 1) - want).abs() < 1e-3);
}

#[test]
fn reading_lanes_does_not_allocate() {
    let mut s = session();
    let mut t = track();
    t.sends = vec![SendSpec::to(1)];
    t.inserts = vec![InsertSpec {
        id: 1,
        ..InsertSpec::default()
    }];
    t.automation = vec![
        lane(Target::Gain, vec![(0, -6.0), (1000, 0.0), (3000, -12.0)]),
        lane(Target::Pan, vec![(0, -1.0), (4800, 1.0)]),
        lane(Target::Send(0), vec![(0, -3.0), (2000, -9.0)]),
        lane(
            Target::Insert { id: 1, param: 0 },
            vec![(0, 0.5), (4000, 1.0)],
        ),
    ];
    s.tracks.push(t);
    s.tracks.push(TrackSpec::new(Vec::new()));
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let mut fx: Vec<(u64, Box<dyn Insert>)> = vec![(
        1,
        Box::new(Scaler {
            gain: 1.0,
            set: None,
        }),
    )];
    ctl.set_graph(s.compile_with(BLOCK, &mut fx).unwrap())
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    eng.process(&mut l, &mut r);
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        for _ in 0..40 {
            eng.process(&mut l, &mut r);
        }
    }
    assert_eq!(rt_check::violations(), before);
}
