// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruments with more than one output (roadmap 3.4): a drum machine's
//! pad on a bus of its own leaves the instrument before the track's
//! effects, and still lines up in time with the rest of the instrument
//! once both reach the master.

use daw_engine::engine;
use daw_engine::graph::{
    AuxSpec, CompileError, Insert, InsertSpec, InstrumentSpec, NoteSpec, Output, Session, TrackSpec,
};
use daw_engine::midi::{Event, Instrument, Kind, Outputs};
use daw_engine::time::{Frames, PPQ, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const LEN: usize = 48_000;
/// Frames per beat at 120 bpm.
const BEAT: usize = 24_000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

/// Clicks on each note: 1.0 on the track, and 0.5 on output 1, or on
/// the track too when output 1 goes nowhere.
struct Clicker;

impl Instrument for Clicker {
    fn process(&mut self, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        self.process_outputs(events, left, right, &mut Outputs::none());
    }

    fn process_outputs(
        &mut self,
        events: &[Event],
        left: &mut [f32],
        right: &mut [f32],
        outputs: &mut Outputs,
    ) {
        for e in events {
            if let Kind::NoteOn { .. } = e.kind {
                let f = e.frame as usize;
                left[f] += 1.0;
                right[f] += 1.0;
                match outputs.get(1) {
                    Some((l, r)) => {
                        l[f] += 0.5;
                        r[f] += 0.5;
                    }
                    None => {
                        left[f] += 0.5;
                        right[f] += 0.5;
                    }
                }
            }
        }
    }
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

fn delay(id: u64, frames: usize) -> (InsertSpec, (u64, Box<dyn Insert>)) {
    (
        InsertSpec {
            id,
            latency: frames as u32,
            ..Default::default()
        },
        (
            id,
            Box::new(Delay {
                line: [vec![0.0; frames], vec![0.0; frames]],
                at: 0,
            }),
        ),
    )
}

/// An instrument track with one note on beat 1 (the second beat).
fn drums() -> TrackSpec {
    let mut t = TrackSpec::new(Vec::new());
    t.notes = vec![NoteSpec::new(Ticks(PPQ), Ticks(PPQ / 2), 36, 100, 0)].into();
    t.instrument = Some(InstrumentSpec { id: 1, latency: 0 });
    t
}

fn render(s: &Session, mut fx: Vec<(u64, Box<dyn Insert>)>) -> Vec<f32> {
    let mut inst = vec![(1, Box::new(Clicker) as Box<dyn Instrument>)];
    let g = s.compile_all(BLOCK, &mut fx, &mut inst).unwrap();
    let [l, _] = engine::render_offline(g, Frames(0), LEN, BLOCK);
    l
}

/// Where the output is not silent, and how loud.
fn clicks(x: &[f32]) -> Vec<(usize, f32)> {
    (x.iter().enumerate())
        .filter(|(_, v)| v.abs() > 1e-6)
        .map(|(i, v)| (i, *v))
        .collect()
}

#[test]
fn an_output_with_nowhere_to_go_plays_through_the_track() {
    let mut s = session();
    s.tracks.push(drums());
    assert_eq!(clicks(&render(&s, Vec::new())), [(BEAT, 1.5)]);
}

#[test]
fn an_extra_output_reaches_its_bus_lined_up_with_the_track() {
    // The track's effects wait 100 frames and the bus's 37: the output
    // to the bus skips the track's, so it must be held back to match.
    let mut s = session();
    let (track_fx, a) = delay(10, 100);
    let (bus_fx, b) = delay(11, 37);
    let mut t = drums();
    t.inserts = vec![track_fx];
    t.instrument_outputs = vec![AuxSpec { output: 1, to: 1 }];
    s.tracks.push(t);
    let mut bus = TrackSpec::new(Vec::new());
    bus.inserts = vec![bus_fx];
    s.tracks.push(bus);
    assert_eq!(clicks(&render(&s, vec![a, b])), [(BEAT, 1.5)]);

    // Through a bus with more latency than the track's effects, the
    // track's own audio is the one that waits.
    let mut s = session();
    let (track_fx, a) = delay(10, 20);
    let (bus_fx, b) = delay(11, 300);
    let mut t = drums();
    t.inserts = vec![track_fx];
    t.instrument_outputs = vec![AuxSpec { output: 1, to: 1 }];
    s.tracks.push(t);
    let mut bus = TrackSpec::new(Vec::new());
    bus.inserts = vec![bus_fx];
    s.tracks.push(bus);
    assert_eq!(clicks(&render(&s, vec![a, b])), [(BEAT, 1.5)]);
}

#[test]
fn an_extra_output_skips_the_tracks_fader_but_not_the_buss() {
    let mut s = session();
    let mut t = drums();
    t.mix.mute = true;
    t.instrument_outputs = vec![AuxSpec { output: 1, to: 1 }];
    s.tracks.push(t);
    s.tracks.push(TrackSpec::new(Vec::new()));
    assert_eq!(clicks(&render(&s, Vec::new())), [(BEAT, 0.5)]);
    s.tracks[0].mix.mute = false;
    s.tracks[1].mix.mute = true;
    assert_eq!(clicks(&render(&s, Vec::new())), [(BEAT, 1.0)]);
}

#[test]
fn bad_output_routes_are_refused() {
    let mut s = session();
    let mut t = drums();
    t.instrument_outputs = vec![AuxSpec { output: 1, to: 5 }];
    s.tracks.push(t);
    assert!(matches!(
        s.compile(BLOCK),
        Err(CompileError::BadRoute { track: 0 })
    ));
    s.tracks[0].instrument_outputs = vec![AuxSpec { output: 0, to: 0 }];
    assert!(s.compile(BLOCK).is_err());
    // Round in a circle: the bus goes back into the drums.
    let mut bus = TrackSpec::new(Vec::new());
    bus.output = Output::Track(0);
    s.tracks[0].instrument_outputs = vec![AuxSpec { output: 1, to: 1 }];
    s.tracks.push(bus);
    assert!(matches!(s.compile(BLOCK), Err(CompileError::Cycle { .. })));
}
