// SPDX-License-Identifier: GPL-3.0-or-later
//! Mixer routing (roadmap 2.3): buses, sends, sidechains, control groups
//! and delay compensation across them, proved by null tests: two ways
//! of routing the same sound must come out the same, sample for sample.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::sync::Arc;

use daw_engine::engine::{self, Command};
use daw_engine::graph::{
    AudioData, ClipSpec, CompileError, GroupSpec, Insert, InsertSpec, Output, SendSpec, Session,
    TrackSpec,
};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const LEN: usize = 4000;
const BLOCK: usize = 128;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

/// Mono noise from `seed`, the same every run, so a shift of even one
/// frame shows up.
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

/// A bus: a track with no clips.
fn bus() -> TrackSpec {
    TrackSpec::new(Vec::new())
}

fn insert(id: u64, latency: u32) -> InsertSpec {
    InsertSpec {
        id,
        bypass: false,
        latency,
        sidechain: None,
        ..Default::default()
    }
}

/// Delays by what it reports, like a plugin with latency.
struct Delay {
    line: [Vec<f32>; 2],
    at: usize,
}

impl Delay {
    fn make(delay: usize) -> Box<dyn Insert> {
        Box::new(Delay {
            line: [vec![0.0; delay], vec![0.0; delay]],
            at: 0,
        })
    }
}

impl Insert for Delay {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let d = self.line[0].len();
        if d == 0 {
            return;
        }
        for i in 0..left.len() {
            left[i] = std::mem::replace(&mut self.line[0][self.at], left[i]);
            right[i] = std::mem::replace(&mut self.line[1][self.at], right[i]);
            self.at = (self.at + 1) % d;
        }
    }
}

/// Plays its sidechain instead of its own audio, so a test can hear
/// exactly what a keyed effect is given.
struct KeyThrough;

impl Insert for KeyThrough {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        left.copy_from_slice(key.0);
        right.copy_from_slice(key.1);
    }
}

fn render(s: &Session, fx: Vec<(u64, Box<dyn Insert>)>) -> [Vec<f32>; 2] {
    let mut fx = fx;
    let g = s.compile_with(BLOCK, &mut fx).unwrap();
    assert!(fx.is_empty(), "the graph took every effect");
    engine::render_offline(g, Frames(0), LEN, BLOCK)
}

/// Frames to let every delay line fill before comparing. Playing from a
/// standstill, an effect's first frames come out of its empty delay line,
/// as they always have (see the inserts tests); by this far in, every
/// route carries sound.
const SETTLE: usize = 1000;

/// Asserts two renders are the same to within float rounding.
fn assert_null(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2], what: &str) {
    for ch in 0..2 {
        for i in SETTLE..LEN {
            let (x, y) = (a[ch][i], b[ch][i]);
            assert!(
                (x - y).abs() < 1e-6,
                "{what}: channel {ch} frame {i}: {x} vs {y}"
            );
        }
    }
    assert!(
        a[0].iter().any(|x| x.abs() > 0.01),
        "{what}: the render is not silent"
    );
}

#[test]
fn a_bus_at_unity_nulls_against_the_direct_path() {
    let mut direct = session();
    direct.tracks.push(track(1));

    let mut routed = session();
    let mut t = track(1);
    t.output = Output::Track(1);
    routed.tracks.push(t);
    routed.tracks.push(bus());

    assert_null(
        &render(&direct, vec![]),
        &render(&routed, vec![]),
        "through a bus",
    );
}

#[test]
fn latency_on_a_bus_is_compensated() {
    // Two tracks sharing a bus with a slow effect, and a third going
    // straight to the master: everything must still line up.
    let mut reference = session();
    reference.tracks.push(track(1));
    // With an effect (stereo, like the routed one), but no latency.
    let mut b = track(2);
    b.inserts = vec![insert(3, 0)];
    reference.tracks.push(b);
    reference.tracks.push(track(3));

    let mut routed = session();
    let mut a = track(1);
    a.output = Output::Track(3);
    let mut b = track(2);
    b.output = Output::Track(3);
    b.inserts = vec![insert(2, 37)];
    routed.tracks.push(a);
    routed.tracks.push(b);
    routed.tracks.push(track(3));
    let mut group = bus();
    group.inserts = vec![insert(1, 200)];
    routed.tracks.push(group);

    assert_null(
        &render(&reference, vec![(3, Delay::make(0))]),
        &render(&routed, vec![(1, Delay::make(200)), (2, Delay::make(37))]),
        "a bus with latency",
    );
}

#[test]
fn nested_buses_are_compensated() {
    let mut reference = session();
    reference.tracks.push(track(5));
    reference.tracks.push(track(6));

    // Track → bus with latency → bus with latency → master, beside a
    // track going straight to the master.
    let mut routed = session();
    let mut a = track(5);
    a.output = Output::Track(2);
    routed.tracks.push(a);
    routed.tracks.push(track(6));
    let mut inner = bus();
    inner.inserts = vec![insert(1, 64)];
    inner.output = Output::Track(3);
    routed.tracks.push(inner);
    let mut outer = bus();
    outer.inserts = vec![insert(2, 300)];
    routed.tracks.push(outer);

    assert_null(
        &render(&reference, vec![]),
        &render(&routed, vec![(1, Delay::make(64)), (2, Delay::make(300))]),
        "nested buses",
    );
}

#[test]
fn a_send_adds_a_copy_lined_up_with_the_output() {
    // A send at 0 dB to a return bus with a slow effect, beside the
    // track's own output, sounds like the track twice.
    let mut reference = session();
    reference.tracks.push(track(7));
    reference.tracks.push(track(7));

    let mut routed = session();
    let mut a = track(7);
    a.sends = vec![SendSpec::to(1)];
    routed.tracks.push(a);
    let mut ret = bus();
    ret.inserts = vec![insert(1, 150)];
    routed.tracks.push(ret);

    assert_null(
        &render(&reference, vec![]),
        &render(&routed, vec![(1, Delay::make(150))]),
        "a send through a return bus",
    );
}

#[test]
fn a_send_level_scales_the_copy() {
    let mut reference = session();
    let mut half = track(8);
    half.mix.gain_db = -6.0;
    reference.tracks.push(half);

    let mut routed = session();
    let mut a = track(8);
    a.mix.gain_db = -96.0; // Nothing by its own output.
    a.sends = vec![SendSpec {
        gain_db: -6.0,
        pre_fader: true,
        ..SendSpec::to(1)
    }];
    routed.tracks.push(a);
    routed.tracks.push(bus());

    assert_null(
        &render(&reference, vec![]),
        &render(&routed, vec![]),
        "a pre-fader send at -6 dB",
    );
}

#[test]
fn a_post_fader_send_follows_the_fader_and_a_pre_fader_one_does_not() {
    let mut s = session();
    let mut a = track(9);
    a.mix.gain_db = -96.0;
    a.sends = vec![SendSpec::to(1)];
    s.tracks.push(a);
    s.tracks.push(bus());
    let [l, _] = render(&s, vec![]);
    assert!(l.iter().all(|x| x.abs() < 1e-9), "post-fader: silent");

    s.tracks[0].sends[0].pre_fader = true;
    let [l, _] = render(&s, vec![]);
    assert!(l.iter().any(|x| x.abs() > 0.01), "pre-fader: heard");

    s.tracks[0].sends[0].mute = true;
    let [l, _] = render(&s, vec![]);
    assert!(l.iter().all(|x| x.abs() < 1e-9), "muted send: silent");
}

#[test]
fn a_sidechain_arrives_lined_up_whichever_side_is_slower() {
    // What the keyed effect hears must be the other track as it would
    // sound at the master, whether the key's own track or the listening
    // track has the latency.
    // The key's track as it sounds alone (stereo, as effects make it).
    let mut reference = session();
    let mut plain = track(11);
    plain.inserts = vec![insert(9, 0)];
    reference.tracks.push(plain);
    let reference = render(&reference, vec![(9, Delay::make(0))]);

    for (key_latency, before, after) in
        [(0, 0, 0), (50, 0, 0), (0, 30, 0), (0, 0, 30), (80, 20, 90)]
    {
        let mut s = session();
        // The listener plays its key, after effects before and after it.
        let mut a = track(10);
        a.inserts = vec![
            insert(1, before),
            InsertSpec {
                sidechain: Some(1),
                ..insert(2, 0)
            },
            insert(3, after),
        ];
        s.tracks.push(a);
        // The key's track, with its own latency, and heard nowhere: it
        // goes to a muted bus.
        let mut key = track(11);
        key.inserts = vec![insert(4, key_latency)];
        key.output = Output::Track(2);
        s.tracks.push(key);
        let mut dump = bus();
        dump.mix.mute = true;
        s.tracks.push(dump);
        let fx: Vec<(u64, Box<dyn Insert>)> = vec![
            (1, Delay::make(before as usize)),
            (2, Box::new(KeyThrough)),
            (3, Delay::make(after as usize)),
            (4, Delay::make(key_latency as usize)),
        ];
        assert_null(
            &reference,
            &render(&s, fx),
            &format!("sidechain with key latency {key_latency}, {before} before, {after} after"),
        );
    }
}

#[test]
fn a_control_group_moves_its_members_faders_together() {
    let mut reference = session();
    let mut a = track(12);
    a.mix.gain_db = -9.0;
    reference.tracks.push(a);
    let mut b = track(13);
    b.mix.gain_db = -3.0;
    reference.tracks.push(b);

    let mut grouped = session();
    grouped.groups.push(GroupSpec {
        gain_db: -5.0,
        mute: false,
    });
    let mut a = track(12);
    a.mix.gain_db = -4.0;
    a.group = Some(0);
    grouped.tracks.push(a);
    let mut b = track(13);
    b.mix.gain_db = 2.0;
    b.group = Some(0);
    grouped.tracks.push(b);
    assert_null(
        &render(&reference, vec![]),
        &render(&grouped, vec![]),
        "a control group at -5 dB",
    );

    grouped.groups[0].mute = true;
    let [l, r] = render(&grouped, vec![]);
    assert!(l.iter().chain(&r).all(|x| x.abs() < 1e-9), "group muted");
}

#[test]
fn group_and_send_changes_apply_without_a_new_graph() {
    let mut s = session();
    s.groups.push(GroupSpec::default());
    let mut a = track(14);
    a.group = Some(0);
    a.mix.gain_db = -96.0;
    a.sends = vec![SendSpec {
        pre_fader: true,
        ..SendSpec::to(1)
    }];
    s.tracks.push(a);
    s.tracks.push(bus());
    let (mut ctl, mut eng) = engine::new(BLOCK);
    ctl.set_graph(s.compile(BLOCK).unwrap()).ok().unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let peak = |l: &[f32]| l.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    eng.process(&mut l, &mut r);
    let full = peak(&l);
    assert!(full > 0.1);

    ctl.send(Command::SetSend {
        track: 0,
        send: 0,
        gain_db: -96.0,
        mute: false,
    })
    .ok()
    .unwrap();
    // The change ramps over a block.
    eng.process(&mut l, &mut r);
    eng.process(&mut l, &mut r);
    assert!(peak(&l) < 1e-6, "send turned down");

    ctl.send(Command::SetSend {
        track: 0,
        send: 0,
        gain_db: 0.0,
        mute: false,
    })
    .ok()
    .unwrap();
    eng.process(&mut l, &mut r);
    eng.process(&mut l, &mut r);
    assert!(peak(&l) > 0.1, "send back up");

    // The group mutes the member, and so its pre-fader send.
    ctl.send(Command::SetGroup {
        group: 0,
        spec: GroupSpec {
            gain_db: 0.0,
            mute: true,
        },
    })
    .ok()
    .unwrap();
    eng.process(&mut l, &mut r);
    eng.process(&mut l, &mut r);
    assert!(peak(&l) < 1e-6, "group muted");
}

#[test]
fn soloing_a_track_is_heard_through_its_bus() {
    let mut reference = session();
    reference.tracks.push(track(15));

    let mut s = session();
    let mut a = track(15);
    a.output = Output::Track(2);
    a.mix.solo = true;
    s.tracks.push(a);
    let mut b = track(16);
    b.output = Output::Track(2);
    s.tracks.push(b);
    s.tracks.push(bus());
    s.tracks.push(track(17));
    assert_null(
        &render(&reference, vec![]),
        &render(&s, vec![]),
        "solo through a bus",
    );
}

#[test]
fn routes_that_go_round_or_nowhere_are_refused() {
    let mut s = session();
    let mut a = bus();
    a.output = Output::Track(1);
    let mut b = bus();
    b.output = Output::Track(0);
    s.tracks.push(a);
    s.tracks.push(b);
    assert!(matches!(s.compile(BLOCK), Err(CompileError::Cycle { .. })));

    let mut s = session();
    let mut a = track(1);
    a.sends = vec![SendSpec::to(0)];
    s.tracks.push(a);
    assert_eq!(
        s.compile(BLOCK).err(),
        Some(CompileError::BadRoute { track: 0 })
    );

    let mut s = session();
    let mut a = track(1);
    a.output = Output::Track(5);
    s.tracks.push(a);
    assert_eq!(
        s.compile(BLOCK).err(),
        Some(CompileError::BadRoute { track: 0 })
    );

    let mut s = session();
    let mut a = track(1);
    a.group = Some(0);
    s.tracks.push(a);
    assert_eq!(
        s.compile(BLOCK).err(),
        Some(CompileError::BadRoute { track: 0 })
    );
}

#[test]
fn delay_lines_carry_over_when_the_graph_changes() {
    // A track going to the master and, by a send, through a slow return
    // bus, with the graph replaced in the middle of playing: the direct
    // route's delay line (holding 500 frames) must carry over, so there
    // is no gap.
    let mut s = session();
    let mut a = track(18);
    a.sends = vec![SendSpec::to(1)];
    s.tracks.push(a);
    let mut ret = bus();
    ret.inserts = vec![insert(1, 500)];
    s.tracks.push(ret);
    // The track twice: what the two routes together should sound like.
    let mut reference = session();
    reference.tracks.push(track(18));
    reference.tracks.push(track(18));
    let want = render(&reference, vec![]);

    let (mut ctl, mut eng) = engine::new(BLOCK);
    let mut fx: Vec<(u64, Box<dyn Insert>)> = vec![(1, Delay::make(500))];
    ctl.set_graph(s.compile_with(BLOCK, &mut fx).unwrap())
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let mut got = Vec::new();
    for block in 0..LEN / BLOCK {
        if block == 9 {
            // The same routing, compiled again (as any edit does).
            ctl.set_graph(s.compile(BLOCK).unwrap()).ok().unwrap();
        }
        eng.process(&mut l, &mut r);
        got.extend_from_slice(&l);
    }
    for (i, (x, y)) in got.iter().zip(&want[0]).enumerate().skip(SETTLE) {
        assert!((x - y).abs() < 1e-6, "frame {i}: {x} vs {y}");
    }
}

#[test]
fn rendering_routes_does_not_allocate() {
    let mut s = session();
    s.groups.push(GroupSpec::default());
    let mut a = track(19);
    a.output = Output::Track(2);
    a.sends = vec![SendSpec::to(3)];
    a.group = Some(0);
    a.inserts = vec![InsertSpec {
        sidechain: Some(1),
        ..insert(1, 0)
    }];
    s.tracks.push(a);
    s.tracks.push(track(20));
    let mut group = bus();
    group.inserts = vec![insert(2, 100)];
    s.tracks.push(group);
    s.tracks.push(bus());
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let mut fx: Vec<(u64, Box<dyn Insert>)> =
        vec![(1, Box::new(KeyThrough)), (2, Delay::make(100))];
    ctl.set_graph(s.compile_with(BLOCK, &mut fx).unwrap())
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    eng.process(&mut l, &mut r);
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        for _ in 0..20 {
            eng.process(&mut l, &mut r);
        }
    }
    assert_eq!(rt_check::violations(), before);
}
