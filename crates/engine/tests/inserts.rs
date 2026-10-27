// SPDX-License-Identifier: GPL-3.0-or-later
//! Track insert effects (roadmap 1.3): order, bypass, latency
//! compensation and effects living on across graph changes.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use daw_engine::engine::{self, Command};
use daw_engine::graph::{AudioData, ClipSpec, Insert, InsertSpec, Session, TrackSpec};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

/// Mono audio whose sample `i` is `(i + 1) / 1e6`.
fn counting(n: usize) -> Arc<AudioData> {
    Arc::new(AudioData {
        channels: vec![(0..n).map(|i| (i + 1) as f32 / 1e6).collect()],
        rate: RATE,
    })
}

/// Delays by `delay` frames and multiplies by `gain`, like a plugin that
/// reports latency. Counts the frames it has processed.
struct DelayGain {
    line: [Vec<f32>; 2],
    at: usize,
    gain: f32,
    processed: Arc<AtomicUsize>,
}

impl DelayGain {
    fn make(delay: usize, gain: f32) -> (Box<dyn Insert>, Arc<AtomicUsize>) {
        let processed = Arc::new(AtomicUsize::new(0));
        let fx = DelayGain {
            line: [vec![0.0; delay], vec![0.0; delay]],
            at: 0,
            gain,
            processed: processed.clone(),
        };
        (Box::new(fx), processed)
    }
}

impl Insert for DelayGain {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let d = self.line[0].len();
        for i in 0..left.len() {
            for (ch, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                let x = buf[i] * self.gain;
                if d == 0 {
                    buf[i] = x;
                } else {
                    buf[i] = std::mem::replace(&mut self.line[ch][self.at], x);
                }
            }
            if d > 0 {
                self.at = (self.at + 1) % d;
            }
        }
        self.processed.fetch_add(left.len(), Ordering::Relaxed);
    }
}

fn hard_left(clips: Vec<ClipSpec>) -> TrackSpec {
    let mut t = TrackSpec::new(clips);
    t.mix.pan = -1.0;
    t
}

#[test]
fn effects_run_in_order_and_bypass_skips_them() {
    let mut s = session();
    let mut t = hard_left(vec![ClipSpec::new(Ticks(0), counting(1000))]);
    t.inserts = vec![
        InsertSpec {
            id: 1,
            bypass: false,
            latency: 0,
        },
        InsertSpec {
            id: 2,
            bypass: true,
            latency: 0,
        },
    ];
    s.tracks.push(t);
    let mut fx = vec![
        (1, DelayGain::make(0, 0.5).0),
        (2, DelayGain::make(0, 0.0).0),
    ];
    let g = s.compile_with(64, &mut fx).unwrap();
    assert!(fx.is_empty(), "the graph took both effects");
    let [l, _] = engine::render_offline(g, Frames(0), 1000, 64);
    for (i, x) in l.iter().enumerate() {
        // Pan law aside, only the first effect's half gain applies.
        let want = (i + 1) as f32 / 1e6 * 0.5;
        assert!((x - want).abs() < 1e-9, "frame {i}: {x} vs {want}");
    }
}

#[test]
fn latency_is_compensated_so_tracks_stay_in_time() {
    let mut s = session();
    // Two identical tracks, one through a 100-frame effect.
    let mut dry = hard_left(vec![ClipSpec::new(Ticks(0), counting(2000))]);
    dry.mix.pan = -1.0;
    let mut wet = TrackSpec::new(vec![ClipSpec::new(Ticks(0), counting(2000))]);
    wet.mix.pan = 1.0;
    wet.inserts = vec![InsertSpec {
        id: 7,
        bypass: false,
        latency: 100,
    }];
    s.tracks = vec![dry, wet];
    let mut fx = vec![(7, DelayGain::make(100, 1.0).0)];
    let g = s.compile_with(256, &mut fx).unwrap();
    let [l, r] = engine::render_offline(g, Frames(0), 1500, 256);
    // After the effect has filled, right lines up with left exactly.
    for i in 100..1500 {
        assert!(
            (l[i] - r[i]).abs() < 1e-9,
            "frame {i}: {} vs {}",
            l[i],
            r[i]
        );
    }
    // The effect's first 100 frames are its empty delay line.
    assert!(r[..100].iter().all(|&x| x == 0.0));
}

#[test]
fn a_mono_track_is_widened_before_its_effects() {
    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), counting(100))]);
    t.inserts = vec![InsertSpec {
        id: 3,
        bypass: false,
        latency: 0,
    }];
    s.tracks.push(t);
    let mut fx = vec![(3, DelayGain::make(0, 1.0).0)];
    let [l, r] = engine::render_offline(s.compile_with(64, &mut fx).unwrap(), Frames(0), 100, 64);
    // Stereo now, so centre pan is equal on both sides.
    assert_eq!(l, r);
    assert!(l[50] > 0.0);
}

#[test]
fn effects_carry_over_when_the_graph_changes() {
    let mut s = session();
    let mut t = hard_left(vec![ClipSpec::new(Ticks(0), counting(10_000))]);
    t.inserts = vec![InsertSpec {
        id: 9,
        bypass: false,
        latency: 0,
    }];
    s.tracks.push(t);
    let (fx, processed) = DelayGain::make(0, 1.0);
    let (mut ctl, mut eng) = engine::new(128);
    ctl.set_graph(s.compile_with(128, &mut vec![(9, fx)]).unwrap())
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let mut buf = [vec![0.0; 128], vec![0.0; 128]];
    let [l, r] = &mut buf;
    eng.process(l, r);
    assert_eq!(processed.load(Ordering::Relaxed), 128);

    // A new graph (volume changed) with no new effects: the old one is
    // taken over, not lost, and nothing is allocated doing it.
    s.tracks[0].mix.gain_db = -6.0;
    ctl.set_graph(s.compile(128).unwrap()).ok().unwrap();
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        eng.process(l, r);
    }
    assert_eq!(rt_check::violations(), before);
    assert_eq!(processed.load(Ordering::Relaxed), 256);

    // Removing the effect leaves it in the retired graph, dropped on the
    // controller's side.
    s.tracks[0].inserts.clear();
    ctl.set_graph(s.compile(128).unwrap()).ok().unwrap();
    eng.process(l, r);
    assert_eq!(processed.load(Ordering::Relaxed), 256);
    ctl.collect_garbage();
    assert_eq!(Arc::strong_count(&processed), 1, "the effect was dropped");
}
