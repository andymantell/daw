// SPDX-License-Identifier: GPL-3.0-or-later
//! Effect containers (roadmap 3.5): effects in chains that a container
//! splits the audio into and joins again, lined up in time inside the
//! container and with the other tracks, proved by null tests against
//! the same sound worked out without one.

use std::sync::Arc;

use daw_engine::engine;
use daw_engine::graph::{
    AudioData, ClipSpec, Insert, InsertSpec, Lanes, Place, Session, Split, TrackSpec,
};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const LEN: usize = 3000;
const BLOCK: usize = 128;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

/// Mono noise, the same every run, so a shift of one frame shows up.
fn noise(n: usize) -> Arc<AudioData> {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
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

/// A track of the noise, panned fully to one side.
fn track(pan: f32) -> TrackSpec {
    let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), noise(LEN))]);
    t.mix.pan = pan;
    t
}

fn fx(id: u64, latency: u32) -> InsertSpec {
    InsertSpec {
        id,
        latency,
        ..Default::default()
    }
}

fn inside(id: u64, latency: u32, container: u64, branch: u8) -> InsertSpec {
    InsertSpec {
        place: Some(Place { container, branch }),
        ..fx(id, latency)
    }
}

fn container(id: u64, branches: u8) -> InsertSpec {
    InsertSpec {
        branches,
        ..fx(id, 0)
    }
}

/// Delays by what it reports and scales, like a plugin with latency.
struct DelayGain {
    line: [Vec<f32>; 2],
    at: usize,
    gain: f32,
}

impl DelayGain {
    fn make(delay: usize, gain: f32) -> Box<dyn Insert> {
        Box::new(DelayGain {
            line: [vec![0.0; delay], vec![0.0; delay]],
            at: 0,
            gain,
        })
    }
}

impl Insert for DelayGain {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let d = self.line[0].len();
        for i in 0..left.len() {
            for (ch, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                let x = buf[i] * self.gain;
                buf[i] = if d == 0 {
                    x
                } else {
                    std::mem::replace(&mut self.line[ch][self.at], x)
                };
            }
            if d > 0 {
                self.at = (self.at + 1) % d;
            }
        }
    }
}

/// Copies the audio into every chain and joins them by adding them up
/// with the dry audio at `dry`.
struct Sum {
    dry: f32,
}

impl Insert for Sum {
    fn process(&mut self, _: &mut [f32], _: &mut [f32]) {}
    fn as_split(&mut self) -> Option<&mut dyn Split> {
        Some(self)
    }
}

impl Split for Sum {
    fn split(&mut self, left: &[f32], right: &[f32], lanes: &mut Lanes) {
        for b in 0..lanes.count() {
            let (l, r) = lanes.get_mut(b);
            l.copy_from_slice(left);
            r.copy_from_slice(right);
        }
    }
    fn join(&mut self, dry: (&[f32], &[f32]), lanes: &Lanes, left: &mut [f32], right: &mut [f32]) {
        for i in 0..left.len() {
            left[i] = dry.0[i] * self.dry;
            right[i] = dry.1[i] * self.dry;
        }
        for b in 0..lanes.count() {
            let (l, r) = lanes.get(b);
            for i in 0..left.len() {
                left[i] += l[i];
                right[i] += r[i];
            }
        }
    }
}

fn sum(dry: f32) -> Box<dyn Insert> {
    Box::new(Sum { dry })
}

/// Renders a reference track on the left, through one effect of
/// `gain`, and the track with `inserts` on the right; checks they match
/// from frame `from` on and that the right is silent before.
fn same_as_gain(
    gain: f32,
    inserts: Vec<InsertSpec>,
    mut effects: Vec<(u64, Box<dyn Insert>)>,
    from: usize,
) {
    let mut s = session();
    let mut reference = track(-1.0);
    reference.inserts = vec![fx(100, 0)];
    let mut t = track(1.0);
    t.inserts = inserts;
    s.tracks = vec![reference, t];
    effects.push((100, DelayGain::make(0, gain)));
    let g = s.compile_with(BLOCK, &mut effects).unwrap();
    assert!(effects.is_empty(), "the graph took every effect");
    let [l, r] = engine::render_offline(g, Frames(0), LEN, BLOCK);
    assert!(l.iter().any(|&x| x != 0.0));
    assert!(r[..from].iter().all(|&x| x == 0.0), "silent while filling");
    for i in from..LEN {
        assert!(
            (l[i] - r[i]).abs() < 1e-6,
            "frame {i}: {} vs {}",
            l[i],
            r[i]
        );
    }
}

#[test]
fn parallel_chains_and_the_dry_audio_are_lined_up_in_time() {
    // Dry at full, a chain at half 50 frames late, and one at a quarter
    // 120 frames late: all lined up, the container is 120 frames late,
    // and the track reads that far ahead.
    same_as_gain(
        1.75,
        vec![container(1, 2), inside(2, 50, 1, 0), inside(3, 120, 1, 1)],
        vec![
            (1, sum(1.0)),
            (2, DelayGain::make(50, 0.5)),
            (3, DelayGain::make(120, 0.25)),
        ],
        120,
    );
}

#[test]
fn effects_in_a_chain_run_in_order_and_add_their_latency() {
    // One chain of two effects (30 + 40 frames) against an empty one.
    same_as_gain(
        1.0 + 0.5 * 0.5,
        vec![container(1, 2), inside(2, 30, 1, 1), inside(3, 40, 1, 1)],
        vec![
            (1, sum(0.0)),
            (2, DelayGain::make(30, 0.5)),
            (3, DelayGain::make(40, 0.5)),
        ],
        70,
    );
}

#[test]
fn a_bypassed_effect_in_a_chain_adds_no_latency() {
    let mut slow = inside(3, 120, 1, 1);
    slow.bypass = true;
    same_as_gain(
        1.0 + 0.5 + 1.0,
        vec![container(1, 2), inside(2, 50, 1, 0), slow],
        vec![
            (1, sum(1.0)),
            (2, DelayGain::make(50, 0.5)),
            (3, DelayGain::make(120, 0.25)),
        ],
        50,
    );
}

#[test]
fn a_bypassed_container_passes_the_audio_and_adds_no_latency() {
    let mut c = container(1, 2);
    c.bypass = true;
    same_as_gain(
        0.5,
        vec![c, inside(2, 50, 1, 0), fx(4, 0)],
        vec![
            (1, sum(1.0)),
            (2, DelayGain::make(50, 0.0)),
            (4, DelayGain::make(0, 0.5)),
        ],
        0,
    );
}

#[test]
fn containers_hold_containers() {
    // The outer one's first chain holds a container of two chains (10
    // and 30 frames late, at a half and a quarter); its second chain is
    // empty. Effects after the outer container still run.
    same_as_gain(
        (0.5 + 0.25 + 1.0) * 2.0,
        vec![
            container(1, 2),
            InsertSpec {
                place: Some(Place {
                    container: 1,
                    branch: 0,
                }),
                ..container(5, 2)
            },
            inside(6, 10, 5, 0),
            inside(7, 30, 5, 1),
            fx(8, 0),
        ],
        vec![
            (1, sum(0.0)),
            (5, sum(0.0)),
            (6, DelayGain::make(10, 0.5)),
            (7, DelayGain::make(30, 0.25)),
            (8, DelayGain::make(0, 2.0)),
        ],
        30,
    );
}

#[test]
fn effects_placed_where_nothing_can_hold_them() {
    // One names a container the track doesn't have: it runs in the
    // track's own chain. One is in a chain past the container's last:
    // it doesn't run.
    same_as_gain(
        0.5 * 2.0,
        vec![inside(2, 0, 99, 0), container(1, 1), inside(3, 0, 1, 3)],
        vec![
            (1, sum(1.0)),
            (2, DelayGain::make(0, 0.5)),
            (3, DelayGain::make(0, 0.0)),
        ],
        0,
    );
}

#[test]
fn other_tracks_wait_for_a_container_on_a_bus() {
    // Two tracks each through a bus, one bus with a container 80 frames
    // late: the other bus's sound stays in time with it.
    use daw_engine::graph::Output;
    let mut s = session();
    let mut a = track(-1.0);
    a.output = Output::Track(2);
    let mut b = track(1.0);
    b.output = Output::Track(3);
    let mut plain = TrackSpec::new(Vec::new());
    plain.mix.pan = -1.0;
    plain.inserts = vec![fx(100, 0)];
    let mut held = TrackSpec::new(Vec::new());
    held.mix.pan = 1.0;
    held.inserts = vec![container(1, 1), inside(2, 80, 1, 0)];
    s.tracks = vec![a, b, plain, held];
    let mut effects = vec![
        (100, DelayGain::make(0, 2.0)),
        (1, sum(1.0)),
        (2, DelayGain::make(80, 1.0)),
    ];
    let g = s.compile_with(BLOCK, &mut effects).unwrap();
    let [l, r] = engine::render_offline(g, Frames(0), LEN, BLOCK);
    assert!(l.iter().any(|&x| x != 0.0));
    for i in 80..LEN {
        assert!(
            (l[i] - r[i]).abs() < 1e-6,
            "frame {i}: {} vs {}",
            l[i],
            r[i]
        );
    }
}
