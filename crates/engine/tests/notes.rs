// SPDX-License-Identifier: GPL-3.0-or-later
//! Notes on instrument tracks (roadmap 3.1): notes from clips reach the
//! instrument on the right frame, round the loop, at practice speed and
//! transposed; live notes reach it as they arrive and are reported for
//! recording; and a MIDI output gets the track's notes.

use std::sync::{Arc, Mutex};

use daw_engine::engine::{self, Command};
use daw_engine::graph::{Control, ControlSpec, ExprSpec, InstrumentSpec, NoteSpec, Session, TrackSpec};
use daw_engine::midi::{Captured, Event, Expression, Instrument, Kind, LiveEvent};
use daw_engine::synth::{SimpleSynth, SynthParams};
use daw_engine::time::{Frames, PPQ, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
/// Frames per beat at 120 bpm.
const BEAT: i64 = 24_000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

/// An instrument that writes down every event with the frame it was
/// given at, counted from when it started.
struct Logger {
    log: Arc<Mutex<Vec<(i64, Event)>>>,
    at: i64,
}

impl Instrument for Logger {
    fn process(&mut self, events: &[Event], left: &mut [f32], _right: &mut [f32]) {
        let mut log = self.log.lock().unwrap();
        for e in events {
            log.push((self.at + e.frame as i64, *e));
        }
        self.at += left.len() as i64;
    }
}

fn note(beat: f64, beats: f64, pitch: u8) -> NoteSpec {
    NoteSpec {
        start: Ticks((beat * PPQ as f64) as i64),
        len: Ticks((beats * PPQ as f64) as i64),
        pitch,
        velocity: 100,
        channel: 0,
        expr: (0, 0),
    }
}

fn track(notes: Vec<NoteSpec>, latency: u32) -> TrackSpec {
    let mut t = TrackSpec::new(Vec::new());
    t.notes = notes.into();
    t.instrument = Some(InstrumentSpec { id: 1, latency });
    t
}

/// Plays `s` through a logging instrument for `frames` frames from
/// `from`, with the loop and practice settings given, and returns what
/// the instrument saw: (frame, kind) for note-ons and note-offs.
fn play(
    s: &Session,
    from: i64,
    frames: usize,
    looping: Option<(i64, i64)>,
    practice: Option<(f64, f64)>,
) -> Vec<(i64, Kind)> {
    play_all(s, from, frames, looping, practice)
        .into_iter()
        .filter(|(_, k)| matches!(k, Kind::NoteOn { .. } | Kind::NoteOff { .. }))
        .collect()
}

/// As [`play`], with every event the instrument saw.
fn play_all(
    s: &Session,
    from: i64,
    frames: usize,
    looping: Option<(i64, i64)>,
    practice: Option<(f64, f64)>,
) -> Vec<(i64, Kind)> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let logger = Logger {
        log: log.clone(),
        at: 0,
    };
    let g = s
        .compile_all(
            BLOCK,
            &mut Vec::new(),
            &mut vec![(1, Box::new(logger) as _)],
        )
        .unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let _ = ctl.set_graph(g);
    let _ = ctl.send(Command::Seek(Frames(from)));
    if let Some((a, b)) = looping {
        let _ = ctl.send(Command::SetLoop(Some((Frames(a), Frames(b)))));
    }
    if let Some((speed, semitones)) = practice {
        let _ = ctl.send(Command::SetPractice { speed, semitones });
    }
    let _ = ctl.send(Command::Play);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let mut done = 0;
    while done < frames {
        let n = BLOCK.min(frames - done);
        eng.process(&mut l[..n], &mut r[..n]);
        done += n;
    }
    log.lock()
        .unwrap()
        .iter()
        .map(|(f, e)| (*f, e.kind))
        .collect()
}

fn on(pitch: u8) -> impl Fn(&Kind) -> bool {
    move |k| matches!(k, Kind::NoteOn { pitch: p, .. } if *p == pitch)
}

fn off(pitch: u8) -> impl Fn(&Kind) -> bool {
    move |k| matches!(k, Kind::NoteOff { pitch: p, .. } if *p == pitch)
}

#[test]
fn notes_from_a_clip_start_and_stop_on_their_frames() {
    let mut s = session();
    s.tracks
        .push(track(vec![note(1.0, 0.5, 60), note(1.0, 1.0, 64)], 0));
    let got = play(&s, 0, 3 * BEAT as usize, None, None);
    assert_eq!(
        got,
        vec![
            (
                BEAT,
                Kind::NoteOn {
                    pitch: 60,
                    velocity: 100,
                    id: 0
                }
            ),
            (
                BEAT,
                Kind::NoteOn {
                    pitch: 64,
                    velocity: 100,
                    id: 1
                }
            ),
            (
                BEAT + BEAT / 2,
                Kind::NoteOff {
                    pitch: 60,
                    velocity: 0,
                    id: 0
                }
            ),
            (
                2 * BEAT,
                Kind::NoteOff {
                    pitch: 64,
                    velocity: 0,
                    id: 1
                }
            ),
        ]
    );
}

#[test]
fn playing_from_the_middle_of_a_note_leaves_it_out_and_a_jump_stops_notes() {
    let mut s = session();
    s.tracks
        .push(track(vec![note(0.0, 2.0, 60), note(1.0, 4.0, 62)], 0));
    // From halfway through the first note: only the second starts.
    let got = play(&s, BEAT / 2, BEAT as usize, None, None);
    assert_eq!(got.len(), 1);
    assert!(on(62)(&got[0].1));
    assert_eq!(got[0].0, BEAT / 2);
}

#[test]
fn the_loop_stops_sounding_notes_and_plays_them_again() {
    let mut s = session();
    s.tracks.push(track(vec![note(1.0, 2.0, 60)], 0));
    // Loop one and a half beats: the note is cut at the loop end and
    // starts again a beat after the loop goes round.
    let end = BEAT + BEAT / 2;
    let got = play(&s, 0, (3 * BEAT) as usize, Some((0, end)), None);
    let ons: Vec<i64> = got
        .iter()
        .filter(|(_, k)| on(60)(k))
        .map(|(f, _)| *f)
        .collect();
    let offs: Vec<i64> = got
        .iter()
        .filter(|(_, k)| off(60)(k))
        .map(|(f, _)| *f)
        .collect();
    assert_eq!(ons, vec![BEAT, end + BEAT]);
    assert_eq!(offs.len(), 1);
    // The off comes at the wrap, within a block of it.
    assert!((offs[0] - end).abs() <= BLOCK as i64, "{}", offs[0]);
    assert!(offs[0] < ons[1]);
}

#[test]
fn practice_speed_and_transpose_move_notes_in_time_and_pitch() {
    let mut s = session();
    s.tracks.push(track(vec![note(1.0, 1.0, 60)], 0));
    let got = play(&s, 0, 5 * BEAT as usize, None, Some((0.5, 12.0)));
    let ons: Vec<(i64, Kind)> = got.iter().filter(|(_, k)| on(72)(k)).cloned().collect();
    let offs: Vec<i64> = got
        .iter()
        .filter(|(_, k)| off(72)(k))
        .map(|(f, _)| *f)
        .collect();
    assert_eq!(ons.len(), 1, "{got:?}");
    // At half speed a beat takes two beats' worth of frames.
    assert!((ons[0].0 - 2 * BEAT).abs() <= 1, "{}", ons[0].0);
    assert!((offs[0] - 4 * BEAT).abs() <= 1, "{}", offs[0]);
}

#[test]
fn an_instruments_latency_reads_notes_early() {
    let mut s = session();
    s.tracks.push(track(vec![note(1.0, 1.0, 60)], 100));
    let got = play(&s, 0, 3 * BEAT as usize, None, None);
    assert_eq!(got[0].0, BEAT - 100);
}

#[test]
fn stopping_sends_every_note_off() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let logger = Logger {
        log: log.clone(),
        at: 0,
    };
    let mut s = session();
    s.tracks.push(track(vec![note(0.0, 8.0, 60)], 0));
    let g = s
        .compile_all(
            BLOCK,
            &mut Vec::new(),
            &mut vec![(1, Box::new(logger) as _)],
        )
        .unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let _ = ctl.set_graph(g);
    let _ = ctl.send(Command::Play);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    eng.process(&mut l, &mut r);
    let _ = ctl.send(Command::Stop);
    eng.process(&mut l, &mut r);
    let kinds: Vec<Kind> = log.lock().unwrap().iter().map(|(_, e)| e.kind).collect();
    assert!(matches!(kinds[0], Kind::NoteOn { pitch: 60, .. }));
    assert!(kinds.contains(&Kind::AllNotesOff), "{kinds:?}");
}

#[test]
fn live_notes_reach_the_instrument_the_midi_output_and_the_recorder() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let logger = Logger {
        log: log.clone(),
        at: 0,
    };
    let mut s = session();
    let mut t = track(Vec::new(), 0);
    t.notes_live = true;
    t.midi_out = Some(3);
    s.tracks.push(t);
    let g = s
        .compile_all(
            BLOCK,
            &mut Vec::new(),
            &mut vec![(1, Box::new(logger) as _)],
        )
        .unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let mut midi_in = ctl.take_midi_input().unwrap();
    let mut midi_out = ctl.take_midi_output().unwrap();
    let _ = ctl.set_graph(g);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    // While stopped: heard, not recorded.
    let _ = midi_in.push(LiveEvent {
        port: 0,
        event: Event::note_on(0, 0, 60, 90, 0),
    });
    eng.process(&mut l, &mut r);
    let _ = ctl.send(Command::Seek(Frames(1000)));
    let _ = ctl.send(Command::SetRecording(true));
    let _ = ctl.send(Command::Play);
    let _ = midi_in.push(LiveEvent {
        port: 0,
        event: Event::note_off(0, 0, 60, 0),
    });
    eng.process(&mut l, &mut r);
    let seen: Vec<Kind> = log.lock().unwrap().iter().map(|(_, e)| e.kind).collect();
    assert_eq!(
        seen,
        vec![
            Kind::NoteOn {
                pitch: 60,
                velocity: 90,
                id: 0
            },
            Kind::NoteOff {
                pitch: 60,
                velocity: 0,
                id: 0
            }
        ]
    );
    let mut out = Vec::new();
    while let Ok(e) = midi_out.pop() {
        out.push(e);
    }
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|e| e.port == 3));
    let mut cap: Vec<Captured> = Vec::new();
    ctl.take_captured(&mut cap);
    assert_eq!(cap.len(), 2);
    assert!(!cap[0].recording && !cap[0].playing);
    assert!(cap[1].recording && cap[1].playing);
    assert_eq!(cap[1].at, 1000);
}

#[test]
fn a_new_graph_keeps_the_instrument_and_its_notes_playing() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let logger = Logger {
        log: log.clone(),
        at: 0,
    };
    let mut s = session();
    s.tracks.push(track(vec![note(0.0, 1.0, 60)], 0));
    let g = s
        .compile_all(
            BLOCK,
            &mut Vec::new(),
            &mut vec![(1, Box::new(logger) as _)],
        )
        .unwrap();
    let (mut ctl, mut eng) = engine::new(BLOCK);
    let _ = ctl.set_graph(g);
    let _ = ctl.send(Command::Play);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    eng.process(&mut l, &mut r);
    // The same notes again, with a second note added later: no new
    // instrument is handed in, the old one carries on.
    s.tracks[0].notes = vec![note(0.0, 1.0, 60), note(2.0, 1.0, 64)].into();
    let g = s.compile(BLOCK).unwrap();
    let _ = ctl.set_graph(g);
    for _ in 0..(BEAT as usize / BLOCK + 2) {
        eng.process(&mut l, &mut r);
    }
    let kinds: Vec<Kind> = log.lock().unwrap().iter().map(|(_, e)| e.kind).collect();
    let ons = kinds.iter().filter(|k| on(60)(k)).count();
    let offs = kinds.iter().filter(|k| off(60)(k)).count();
    assert_eq!((ons, offs), (1, 1), "{kinds:?}");
}

#[test]
fn the_simple_synth_plays_a_clips_note_through_the_engine() {
    let mut s = session();
    let params = Arc::new(SynthParams::default());
    params.set("release", 0.01);
    s.tracks.push(track(vec![note(0.5, 0.5, 69)], 0));
    let synth = SimpleSynth::new(RATE, params);
    let g = s
        .compile_all(BLOCK, &mut Vec::new(), &mut vec![(1, Box::new(synth) as _)])
        .unwrap();
    let [l, _] = engine::render_offline(g, Frames(0), 2 * BEAT as usize, BLOCK);
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert_eq!(peak(&l[..BEAT as usize / 2]), 0.0);
    assert!(peak(&l[BEAT as usize / 2..BEAT as usize]) > 0.05);
    assert_eq!(peak(&l[BEAT as usize + 2000..]), 0.0);
}

#[test]
fn controller_lanes_play_and_catch_up_after_a_jump() {
    let mut s = session();
    let mut t = track(vec![note(0.0, 4.0, 60)], 0);
    t.controls = vec![ControlSpec {
        channel: 1,
        control: Control::Cc(1),
        points: vec![(Ticks(0), 10), (Ticks(PPQ), 64), (Ticks(PPQ * 2), 127)].into(),
    }];
    s.tracks.push(t);
    let cc = |v: u8| Kind::Control {
        number: 1,
        value: v,
    };
    // From the start: each point on its beat.
    let got = play_all(&s, 0, (BEAT * 3) as usize, None, None);
    let ctl: Vec<(i64, Kind)> = got
        .iter()
        .copied()
        .filter(|(_, k)| matches!(k, Kind::Control { .. }))
        .collect();
    assert_eq!(ctl, vec![(0, cc(10)), (BEAT, cc(64)), (2 * BEAT, cc(127))]);
    // From half way through the second beat: the value in force first.
    let got = play_all(&s, BEAT + BEAT / 2, BEAT as usize, None, None);
    let ctl: Vec<(i64, Kind)> = got
        .iter()
        .copied()
        .filter(|(_, k)| matches!(k, Kind::Control { .. }))
        .collect();
    assert_eq!(ctl, vec![(0, cc(64)), (BEAT / 2, cc(127))]);
    // Round a loop over beats 2 to 3: caught up again each time round.
    let got = play_all(&s, BEAT, (BEAT * 2) as usize, Some((BEAT, 2 * BEAT)), None);
    let firsts = got
        .iter()
        .filter(|(_, k)| *k == cc(64))
        .count();
    assert_eq!(firsts, 2, "{got:?}");
}

#[test]
fn a_notes_expression_reaches_the_instrument_while_it_sounds() {
    let mut s = session();
    let mut n = note(1.0, 1.0, 60);
    n.expr = (0, 3);
    let mut t = track(vec![note(0.0, 0.5, 48), n], 0);
    t.expression = vec![
        ExprSpec {
            at: Ticks(PPQ),
            kind: Expression::Tuning,
            value: 0.0,
        },
        ExprSpec {
            at: Ticks(PPQ + PPQ / 2),
            kind: Expression::Tuning,
            value: 1.0,
        },
        // After the note ends: never sent.
        ExprSpec {
            at: Ticks(PPQ * 3),
            kind: Expression::Pressure,
            value: 1.0,
        },
    ]
    .into();
    s.tracks.push(t);
    let got = play_all(&s, 0, (BEAT * 4) as usize, None, None);
    let on_frame = got.iter().find(|(_, k)| on(60)(k)).unwrap().0;
    let id = match got.iter().find(|(_, k)| on(60)(k)).unwrap().1 {
        Kind::NoteOn { id, .. } => id,
        _ => unreachable!(),
    };
    let exprs: Vec<(i64, Kind)> = got
        .iter()
        .copied()
        .filter(|(_, k)| matches!(k, Kind::NoteExpression { .. }))
        .collect();
    let at = |value: f32| Kind::NoteExpression {
        id,
        pitch: 60,
        kind: Expression::Tuning,
        value,
    };
    assert_eq!(
        exprs,
        vec![(on_frame, at(0.0)), (BEAT + BEAT / 2, at(1.0))],
        "{got:?}"
    );
    // The note-on comes before its first expression on the same frame.
    let i_on = got.iter().position(|(_, k)| on(60)(k)).unwrap();
    let i_x = got.iter().position(|(_, k)| *k == at(0.0)).unwrap();
    assert!(i_on < i_x);
}
