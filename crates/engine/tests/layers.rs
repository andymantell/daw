// SPDX-License-Identifier: GPL-3.0-or-later
//! Layered instruments (roadmap 3.5): several instruments sharing one
//! track's notes, each playing the notes in its ranges, moved by its
//! transpose, through its own effects, at its own level, lined up in
//! time, and carried over to a new graph with the notes they hold.

use std::sync::{Arc, Mutex};

use daw_engine::engine::{self, Command};
use daw_engine::graph::{Insert, InsertSpec, InstrumentSpec, NoteSpec, Place, Session, TrackSpec};
use daw_engine::layers::LayerSpec;
use daw_engine::midi::{Event, Instrument, Kind};
use daw_engine::time::{Frames, PPQ, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
/// Frames per beat at 120 bpm.
const BEAT: i64 = 24_000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

type Log = Arc<Mutex<Vec<Kind>>>;

/// Writes down every note it is given, and plays a click on each
/// note-on's frame.
struct Clicker {
    log: Log,
}

impl Instrument for Clicker {
    fn process(&mut self, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        for e in events {
            if matches!(e.kind, Kind::NoteOn { .. } | Kind::NoteOff { .. }) {
                self.log.lock().unwrap().push(e.kind);
            }
            if let Kind::NoteOn { .. } = e.kind
                && let Some(l) = left.get_mut(e.frame as usize)
            {
                *l += 1.0;
                right[e.frame as usize] += 1.0;
            }
        }
    }
}

/// Plays a steady level, notes or not.
struct Steady(f32);

impl Instrument for Steady {
    fn process(&mut self, _events: &[Event], left: &mut [f32], right: &mut [f32]) {
        for s in left.iter_mut().chain(right.iter_mut()) {
            *s += self.0;
        }
    }
}

/// Delays by `delay` frames and scales by `gain`.
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

fn note(beat: f64, beats: f64, pitch: u8, velocity: u8) -> NoteSpec {
    NoteSpec {
        start: Ticks((beat * PPQ as f64) as i64),
        len: Ticks((beats * PPQ as f64) as i64),
        pitch,
        velocity,
        channel: 0,
        expr: (0, 0),
    }
}

/// A layer, by id, whose instrument has the same id plus 100.
fn layer(id: u64) -> LayerSpec {
    LayerSpec::new(
        id,
        Some(InstrumentSpec {
            id: id + 100,
            latency: 0,
        }),
    )
}

fn layered(notes: Vec<NoteSpec>, layers: Vec<LayerSpec>) -> TrackSpec {
    let mut t = TrackSpec::new(Vec::new());
    t.notes = notes.into();
    t.layers = layers;
    t
}

fn clicker(id: u64) -> (Log, (u64, Box<dyn Instrument>)) {
    let log = Log::default();
    (log.clone(), (id + 100, Box::new(Clicker { log }) as _))
}

/// The notes in `log` as (on or off, pitch).
fn notes(log: &Log) -> Vec<(bool, u8)> {
    (log.lock().unwrap().iter())
        .filter_map(|k| match *k {
            Kind::NoteOn { pitch, .. } => Some((true, pitch)),
            Kind::NoteOff { pitch, .. } => Some((false, pitch)),
            _ => None,
        })
        .collect()
}

#[test]
fn each_layer_plays_the_notes_in_its_ranges_moved_by_its_transpose() {
    let mut s = session();
    let (low, high, soft) = (layer(1), layer(2), layer(3));
    low.controls.set_keys(0, 59);
    high.controls.set_keys(60, 127);
    high.controls.set_transpose(12);
    soft.controls.set_velocities(1, 63);
    s.tracks.push(layered(
        vec![
            note(0.0, 0.5, 48, 100),
            note(1.0, 0.5, 64, 100),
            note(2.0, 0.5, 72, 40),
        ],
        vec![low, high, soft],
    ));
    let (a, ia) = clicker(1);
    let (b, ib) = clicker(2);
    let (c, ic) = clicker(3);
    let g = s
        .compile_all(BLOCK, &mut Vec::new(), &mut vec![ia, ib, ic])
        .unwrap();
    engine::render_offline(g, Frames(0), 3 * BEAT as usize, BLOCK);
    assert_eq!(notes(&a), vec![(true, 48), (false, 48)]);
    assert_eq!(
        notes(&b),
        vec![(true, 76), (false, 76), (true, 84), (false, 84)]
    );
    assert_eq!(notes(&c), vec![(true, 72), (false, 72)]);
}

#[test]
fn layers_mix_at_their_levels_through_their_own_effects() {
    let mut s = session();
    let (quiet, doubled) = (layer(1), layer(2));
    quiet.controls.set_gain_db(-6.0206);
    let mut t = layered(Vec::new(), vec![quiet, doubled.clone()]);
    // Doubled by an effect in the second layer only.
    t.inserts.push(InsertSpec {
        id: 7,
        place: Some(Place {
            container: 2,
            branch: 0,
        }),
        ..Default::default()
    });
    s.tracks.push(t);
    let g = s
        .compile_all(
            BLOCK,
            &mut vec![(7, DelayGain::make(0, 2.0))],
            &mut vec![
                (101, Box::new(Steady(0.5)) as _),
                (102, Box::new(Steady(0.25)) as _),
            ],
        )
        .unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let _ = ctl.set_graph(g);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    eng.process(&mut l, &mut r);
    // 0.5 at half level, and 0.25 doubled.
    assert!((l[BLOCK - 1] - 0.75).abs() < 1e-3, "{}", l[BLOCK - 1]);
    assert!((r[BLOCK - 1] - 0.75).abs() < 1e-3);
    // Muting a layer takes effect without a new graph, smoothly.
    doubled.controls.set_mute(true);
    eng.process(&mut l, &mut r);
    assert!(l[0] > 0.25 && l[0] < 0.75, "ramps: {}", l[0]);
    eng.process(&mut l, &mut r);
    assert!((l[BLOCK - 1] - 0.25).abs() < 1e-3);
    // Panned left, the right side loses it.
    doubled.controls.set_mute(false);
    doubled.controls.set_pan(-1.0);
    eng.process(&mut l, &mut r);
    eng.process(&mut l, &mut r);
    assert!((l[BLOCK - 1] - 0.75).abs() < 1e-3);
    assert!((r[BLOCK - 1] - 0.25).abs() < 1e-3);
}

#[test]
fn a_layer_with_latency_is_lined_up_with_the_others() {
    let mut s = session();
    let mut t = layered(vec![note(0.5, 0.25, 60, 100)], vec![layer(1), layer(2)]);
    // 37 frames of delay in the first layer's chain.
    t.inserts.push(InsertSpec {
        id: 7,
        latency: 37,
        place: Some(Place {
            container: 1,
            branch: 0,
        }),
        ..Default::default()
    });
    s.tracks.push(t);
    let (_, ia) = clicker(1);
    let (_, ib) = clicker(2);
    let g = s
        .compile_all(
            BLOCK,
            &mut vec![(7, DelayGain::make(37, 1.0))],
            &mut vec![ia, ib],
        )
        .unwrap();
    let [l, _] = engine::render_offline(g, Frames(0), BEAT as usize, BLOCK);
    let clicks: Vec<(usize, f32)> = (l.iter().enumerate())
        .filter(|(_, v)| v.abs() > 1e-6)
        .map(|(i, v)| (i, *v))
        .collect();
    // Both clicks land together, on the note's own frame.
    assert_eq!(clicks, vec![(BEAT as usize / 2, 2.0)]);
}

#[test]
fn a_new_graph_keeps_the_layers_and_the_notes_they_hold() {
    let mut s = session();
    let up = layer(1);
    up.controls.set_transpose(12);
    s.tracks
        .push(layered(vec![note(0.0, 1.0, 60, 100)], vec![up.clone()]));
    let (log, i) = clicker(1);
    let g = s.compile_all(BLOCK, &mut Vec::new(), &mut vec![i]).unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let _ = ctl.set_graph(g);
    let _ = ctl.send(Command::Play);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    eng.process(&mut l, &mut r);
    // While the note sounds: the transpose changes, and a second layer
    // comes in, with no instrument handed in for the first.
    up.controls.set_transpose(0);
    s.tracks[0].layers.push(layer(2));
    let (log2, i2) = clicker(2);
    let g = s
        .compile_all(BLOCK, &mut Vec::new(), &mut vec![i2])
        .unwrap();
    let _ = ctl.set_graph(g);
    for _ in 0..(BEAT as usize / BLOCK + 2) {
        eng.process(&mut l, &mut r);
    }
    // The note stopped where it started, once.
    assert_eq!(notes(&log), vec![(true, 72), (false, 72)]);
    // The new layer didn't start it, so isn't sent its note-off.
    assert_eq!(notes(&log2), vec![]);
}

#[test]
fn a_tracks_instrument_turned_into_a_layer_carries_on() {
    let mut s = session();
    let mut t = TrackSpec::new(Vec::new());
    t.notes = vec![note(0.0, 0.5, 60, 100), note(1.0, 0.5, 62, 100)].into();
    t.instrument = Some(InstrumentSpec {
        id: 101,
        latency: 0,
    });
    s.tracks.push(t);
    let (log, i) = clicker(1);
    let g = s.compile_all(BLOCK, &mut Vec::new(), &mut vec![i]).unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let _ = ctl.set_graph(g);
    let _ = ctl.send(Command::Play);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    eng.process(&mut l, &mut r);
    // The same instrument now the first layer: it is taken over, its
    // note is stopped, and the next note plays through the layer.
    s.tracks[0].instrument = None;
    s.tracks[0].layers = vec![layer(1)];
    let _ = ctl.set_graph(s.compile(BLOCK).unwrap());
    for _ in 0..(2 * BEAT as usize / BLOCK) {
        eng.process(&mut l, &mut r);
    }
    let got = notes(&log);
    assert_eq!(got.first(), Some(&(true, 60)));
    assert!(
        got.contains(&(true, 62)) && got.contains(&(false, 62)),
        "{got:?}"
    );
}
