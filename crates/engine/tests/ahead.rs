// SPDX-License-Identifier: GPL-3.0-or-later
//! Low-latency monitoring with a big mix buffer: the engine split between
//! a worker that renders the mix ahead in big blocks and a callback that
//! plays the live tracks in small ones. Null tests: the split engine must
//! sound like the whole one, frame for frame, while live input comes
//! straight through in the callback it arrived in.

use std::sync::Arc;

use daw_engine::ahead::{self, Front, Worker};
use daw_engine::engine::{self, Command, Controller};
use daw_engine::graph::{
    AudioData, ClipSpec, Input, Insert, InsertSpec, Output, SendSpec, Session, TrackSpec,
};
use daw_engine::metronome::Metronome;
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
const LEN: usize = 24_000;
/// The device's buffer, and the worker's.
const SMALL: usize = 64;
const BIG: usize = 1024;
const MAX: usize = 4096;
const CH: usize = 2;

type Effects = Vec<(u64, Box<dyn Insert>)>;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

/// Noise from `seed`, the same every run, so a shift of even one frame
/// shows up.
fn noise(seed: u64, n: usize) -> Vec<f32> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        })
        .collect()
}

fn clip(seed: u64) -> ClipSpec {
    ClipSpec::new(
        Ticks(0),
        Arc::new(AudioData {
            channels: vec![noise(seed, LEN)],
            rate: RATE,
        }),
    )
}

/// The device input: two channels of noise.
fn input(from: usize, frames: usize) -> Vec<f32> {
    let all = noise(99, (from + frames) * CH);
    all[from * CH..].to_vec()
}

fn insert(id: u64, latency: u32) -> InsertSpec {
    InsertSpec {
        id,
        latency,
        ..Default::default()
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

fn delay(d: usize) -> Box<dyn Insert> {
    Box::new(Delay {
        line: [vec![0.0; d], vec![0.0; d]],
        at: 0,
    })
}

/// A one-pole low-pass: it remembers, so losing it or running it twice
/// shows.
struct Smooth([f32; 2]);

impl Insert for Smooth {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (k, buf) in [left, right].into_iter().enumerate() {
            for x in buf {
                self.0[k] += 0.3 * (*x - self.0[k]);
                *x = self.0[k];
            }
        }
    }
}

/// Adds a steady level, so a test can hear whether the effect is there.
struct Offset(f32);

impl Insert for Offset {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x += self.0;
        }
    }
}

/// Plays its sidechain in place of its own audio.
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

/// A mix with every kind of route between the worker's tracks and the
/// live ones:
///
/// 0. guitar: monitors input 0-1, through an amp (with latency), to bus 3
/// 1. backing: clips, with latency, to bus 3 and sent to bus 4
/// 2. drums: clips, to the master
/// 3. guitar bus: smoothing
/// 4. reverb bus: smoothing (the worker's alone)
fn mix() -> (Session, Effects) {
    let mut s = session();
    let mut guitar = TrackSpec::new(vec![clip(1)]);
    guitar.input = Some(Input::stereo(0));
    guitar.monitor = true;
    guitar.arm = true;
    guitar.inserts = vec![insert(10, 7)];
    guitar.output = Output::Track(3);
    guitar.mix.gain_db = -3.0;
    let mut backing = TrackSpec::new(vec![clip(2)]);
    backing.inserts = vec![insert(11, 33)];
    backing.output = Output::Track(3);
    backing.sends = vec![SendSpec::to(4)];
    backing.mix.pan = 0.4;
    let mut drums = TrackSpec::new(vec![clip(3)]);
    drums.mix.gain_db = -6.0;
    let mut gbus = TrackSpec::new(vec![]);
    gbus.inserts = vec![insert(12, 0)];
    let mut verb = TrackSpec::new(vec![]);
    verb.inserts = vec![insert(14, 0)];
    s.tracks = vec![guitar, backing, drums, gbus, verb];
    s.master_db = -1.5;
    let fx: Vec<(u64, Box<dyn Insert>)> = vec![
        (10, delay(7)),
        (11, delay(33)),
        (12, Box::new(Smooth([0.0; 2]))),
        (14, Box::new(Smooth([0.0; 2]))),
    ];
    (s, fx)
}

/// An engine split by hand, for stepping the worker and the callback in
/// turn.
struct Split {
    ctl: Controller,
    front: Front,
    worker: Worker,
}

fn split() -> Split {
    let (ctl, eng) = engine::new(MAX);
    let (front, worker) = ahead::split_by_hand(eng, BIG);
    Split { ctl, front, worker }
}

fn whole() -> (Controller, engine::Engine) {
    engine::new(MAX)
}

fn graph_for(
    ctl: &Controller,
    s: &Session,
    fx: &mut Vec<(u64, Box<dyn Insert>)>,
) -> daw_engine::graph::Graph {
    let g = s.compile_with(MAX, fx).unwrap();
    if ctl.wants_twin() {
        g.with_twin(s.compile(MAX).unwrap())
    } else {
        g
    }
}

/// Plays `frames` frames in callbacks of `SMALL`, giving the worker its
/// turn before each when `pump` says so.
fn run_split(sp: &mut Split, from: usize, frames: usize, pump: bool) -> [Vec<f32>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    let mut at = from;
    while at < from + frames {
        if pump {
            while sp.worker.fill() {}
        }
        let inp = input(at, SMALL);
        let (mut l, mut r) = (vec![0.0; SMALL], vec![0.0; SMALL]);
        sp.front.process_duplex(&inp, CH, &mut l, &mut r, None);
        out[0].extend(l);
        out[1].extend(r);
        at += SMALL;
    }
    out
}

fn run_whole(eng: &mut engine::Engine, from: usize, frames: usize) -> [Vec<f32>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    let mut at = from;
    while at < from + frames {
        let inp = input(at, SMALL);
        let (mut l, mut r) = (vec![0.0; SMALL], vec![0.0; SMALL]);
        eng.process_duplex(&inp, CH, &mut l, &mut r, None);
        out[0].extend(l);
        out[1].extend(r);
        at += SMALL;
    }
    out
}

fn assert_same(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2], what: &str) {
    assert_eq!(a[0].len(), b[0].len());
    let mut loud = 0.0f32;
    for ch in 0..2 {
        for (i, (x, y)) in a[ch].iter().zip(&b[ch]).enumerate() {
            assert!(
                (x - y).abs() < 1e-5,
                "{what}: channel {ch} frame {i}: {x} against {y}"
            );
            loud = loud.max(x.abs());
        }
    }
    assert!(loud > 0.05, "{what}: there was something to hear");
}

#[test]
fn live_tracks_and_what_they_feed_play_in_the_callback() {
    let (s, mut fx) = mix();
    let g = s.compile_with(MAX, &mut fx).unwrap();
    // The guitar is live, and so is its bus; the rest are not.
    assert_eq!(g.live_tracks(), vec![true, false, false, true, false]);

    // A live track's effect keyed from another track brings that one
    // into the callback too.
    let mut s2 = s.clone();
    s2.tracks[3].inserts.push(InsertSpec {
        sidechain: Some(2),
        ..insert(13, 0)
    });
    let g = s2.compile(MAX).unwrap();
    assert_eq!(g.live_tracks(), vec![true, false, true, true, false]);
}

/// Plays `make`'s mix, after `cmds`, whole and split, and returns both.
fn both(
    make: fn() -> (Session, Effects),
    cmds: fn() -> Vec<Command>,
    frames: usize,
) -> ([Vec<f32>; 2], [Vec<f32>; 2], Split) {
    let (mut ctl, mut eng) = whole();
    let (s, mut fx) = make();
    ctl.set_graph(graph_for(&ctl, &s, &mut fx)).ok().unwrap();
    for c in cmds() {
        ctl.send(c).ok().unwrap();
    }
    let a = run_whole(&mut eng, 0, frames);

    let mut sp = split();
    let (s, mut fx) = make();
    let g = graph_for(&sp.ctl, &s, &mut fx);
    sp.ctl.set_graph(g).ok().unwrap();
    for c in cmds() {
        sp.ctl.send(c).ok().unwrap();
    }
    let b = run_split(&mut sp, 0, frames, true);
    (a, b, sp)
}

fn looping_with_clicks() -> Vec<Command> {
    let metronome = Metronome {
        enabled: true,
        level_db: -12.0,
        ..Metronome::default()
    };
    vec![
        Command::SetMetronome(metronome),
        Command::SetLoop(Some((Frames(3_000), Frames(11_000)))),
        Command::Play,
    ]
}

#[test]
fn the_split_mix_sounds_the_same_as_the_whole_one() {
    let (a, b, sp) = both(mix, looping_with_clicks, 20_000);
    assert_same(&a, &b, "split");
    let status = sp.ctl.status();
    assert!(status.loops() >= 2, "went round the loop");
    assert_eq!(status.ahead.late(), 0);
    assert_eq!(status.ahead.block(), Some(BIG as u32));
}

/// [`mix`], with the guitar bus playing its sidechain from the drums in
/// place of its own sound: the drums must come into the callback too.
fn keyed() -> (Session, Effects) {
    let (mut s, mut fx) = mix();
    s.tracks[3].inserts.push(InsertSpec {
        sidechain: Some(2),
        ..insert(13, 0)
    });
    s.tracks[0].mix.gain_db = 0.0;
    fx.push((13, Box::new(KeyThrough)));
    (s, fx)
}

#[test]
fn a_sidechain_from_the_rest_of_the_mix_stays_in_time() {
    let (a, b, _) = both(keyed, looping_with_clicks, 16_000);
    assert_same(&a, &b, "keyed");
}

#[test]
fn practice_speed_and_transpose_sound_the_same_split() {
    let (a, b, _) = both(
        mix,
        || {
            vec![
                Command::SetPractice {
                    speed: 0.75,
                    semitones: -2.0,
                },
                Command::Play,
            ]
        },
        16_000,
    );
    assert_same(&a, &b, "practice");
}

#[test]
fn live_input_comes_out_in_the_callback_it_went_in() {
    // Only the guitar, monitored straight to the master: its input must
    // sound in the same callback, though the worker runs big blocks.
    let mut sp = split();
    let mut s = session();
    let mut guitar = TrackSpec::new(vec![]);
    guitar.input = Some(Input::stereo(0));
    guitar.monitor = true;
    let mut backing = TrackSpec::new(vec![clip(5)]);
    backing.mix.mute = true;
    s.tracks = vec![guitar, backing];
    let g = graph_for(&sp.ctl, &s, &mut Vec::new());
    sp.ctl.set_graph(g).ok().unwrap();
    sp.ctl.send(Command::Play).ok().unwrap();
    // Let the worker get going; from then on, every callback.
    run_split(&mut sp, 0, BIG, true);
    let out = run_split(&mut sp, BIG, 4 * BIG, true);
    let inp = input(BIG, 4 * BIG);
    for i in 0..4 * BIG {
        // Stereo input, panned centre: each side as it came in.
        assert!((out[0][i] - inp[i * CH]).abs() < 1e-6, "frame {i}");
        assert!((out[1][i] - inp[i * CH + 1]).abs() < 1e-6, "frame {i}");
    }
}

#[test]
fn a_late_worker_pauses_the_mix_but_not_the_live_tracks() {
    let mut sp = split();
    let mut s = session();
    let mut guitar = TrackSpec::new(vec![]);
    guitar.input = Some(Input::stereo(0));
    guitar.monitor = true;
    s.tracks = vec![guitar, TrackSpec::new(vec![clip(6)])];
    let g = graph_for(&sp.ctl, &s, &mut Vec::new());
    sp.ctl.set_graph(g).ok().unwrap();
    sp.ctl.send(Command::Play).ok().unwrap();
    run_split(&mut sp, 0, BIG, true);
    let before = sp.ctl.status().position().0;
    // The worker stops: what it made plays out, then only the guitar.
    let out = run_split(&mut sp, BIG, 4 * BIG, false);
    let status = sp.ctl.status();
    assert!(
        status.ahead.late() >= 2 * BIG as u64,
        "{}",
        status.ahead.late()
    );
    let end = 4 * BIG - SMALL;
    let inp = input(BIG + end, SMALL);
    for i in 0..SMALL {
        assert!((out[0][end + i] - inp[i * CH]).abs() < 1e-6, "frame {i}");
    }
    // The play position waits for the worker.
    let after = status.position().0;
    assert!(after - before <= 2 * BIG as i64, "{before} to {after}");
    // Once it catches up, the mix carries on from where it paused.
    run_split(&mut sp, 5 * BIG, 2 * BIG, true);
    assert!(sp.ctl.status().position().0 > after);
}

/// The left output of a split engine given a new graph every few
/// blocks, the track monitoring its input or not as `monitor` says.
fn offsets(monitor: &[bool]) -> Vec<f32> {
    let mut sp = split();
    let mut out = Vec::new();
    let mut first = true;
    for &live in monitor {
        let mut s = session();
        let mut t = TrackSpec::new(vec![]);
        t.monitor = live;
        // An input the device lacks: only the effect is heard.
        t.input = Some(Input::stereo(4));
        t.inserts = vec![insert(20, 0)];
        s.tracks = vec![t, TrackSpec::new(vec![])];
        let mut fx: Vec<(u64, Box<dyn Insert>)> = if first {
            vec![(20, Box::new(Offset(0.25)))]
        } else {
            Vec::new()
        };
        first = false;
        let g = graph_for(&sp.ctl, &s, &mut fx);
        sp.ctl.set_graph(g).ok().unwrap();
        let [l, _] = run_split(&mut sp, 0, 4 * BIG, true);
        out.extend(l);
        sp.ctl.collect_garbage();
    }
    out
}

#[test]
fn arming_and_disarming_move_the_effects_across_and_back() {
    // The track's effect adds a level; it is made once, so if it were
    // lost on the way across, the level would go.
    // A stereo track, panned centre: full level on each side.
    let want = 0.25;
    let out = offsets(&[false, true, false, true, true, false]);
    let n = 4 * BIG;
    for (k, part) in out.chunks(n).enumerate() {
        // Each graph is heard a block or two after it is sent; by the end
        // of each stretch, the effect plays.
        let tail = &part[n - BIG..];
        assert!(
            tail.iter().all(|x| (x - want).abs() < 1e-6),
            "stretch {k}: {:?}",
            &tail[..4]
        );
        // Changing over, the track is silent for a moment at most, never
        // heard without its effect.
        assert!(
            part.iter()
                .all(|x| (x - want).abs() < 1e-6 || x.abs() < 1e-9)
        );
        let gap = part.iter().filter(|x| x.abs() < 1e-9).count();
        assert!(gap <= 3 * BIG, "stretch {k}: silent for {gap} frames");
    }
}

#[test]
fn a_whole_engine_ignores_the_twin() {
    let (mut ctl, mut eng) = whole();
    let (s, mut fx) = mix();
    let g = s.compile_with(MAX, &mut fx).unwrap();
    let g = g.with_twin(s.compile(MAX).unwrap());
    assert!(!ctl.wants_twin());
    ctl.set_graph(g).ok().unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let a = run_whole(&mut eng, 0, 4_000);

    let (mut ctl, mut eng) = whole();
    let (s, mut fx) = mix();
    ctl.set_graph(s.compile_with(MAX, &mut fx).unwrap())
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let b = run_whole(&mut eng, 0, 4_000);
    assert_eq!(a, b);
}

/// Records `blocks` callbacks at half speed from frame 4800, whole or
/// split, and returns the take's start, length and audio as played.
fn record_slowed(split_it: bool, blocks: usize) -> (Frames, usize, Vec<f32>) {
    let dir =
        std::env::temp_dir().join(format!("daw-ahead-slow-{}-{split_it}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = session();
    let mut t = TrackSpec::new(vec![]);
    t.input = Some(Input::stereo(0));
    t.arm = true;
    s.tracks = vec![t, TrackSpec::new(vec![clip(8)])];
    let (mut ctl, mut eng, mut sp) = if split_it {
        let sp = split();
        (None, None, Some(sp))
    } else {
        let (c, e) = whole();
        (Some(c), Some(e), None)
    };
    let c = ctl.as_mut().or(sp.as_mut().map(|s| &mut s.ctl)).unwrap();
    c.set_record_dir(&dir);
    let g = graph_for(c, &s, &mut Vec::new());
    c.set_graph(g).ok().unwrap();
    let mut rec = c.take_recorder().unwrap();
    for cmd in [
        Command::SetPractice {
            speed: 0.5,
            semitones: 0.0,
        },
        Command::Seek(Frames(4800)),
        Command::SetRecording(true),
        Command::Play,
    ] {
        c.send(cmd).ok().unwrap();
    }
    let mut play = |sp: &mut Option<Split>, eng: &mut Option<engine::Engine>, at, n| match (
        sp.as_mut(),
        eng.as_mut(),
    ) {
        (Some(sp), _) => {
            run_split(sp, at, n, true);
        }
        (_, Some(e)) => {
            run_whole(e, at, n);
        }
        _ => unreachable!(),
    };
    play(&mut sp, &mut eng, 0, blocks * SMALL);
    rec.service();
    let c = ctl.as_mut().or(sp.as_mut().map(|s| &mut s.ctl)).unwrap();
    c.send(Command::Stop).ok().unwrap();
    play(&mut sp, &mut eng, blocks * SMALL, 2 * BIG);
    rec.service();
    let c = ctl.as_mut().or(sp.as_mut().map(|s| &mut s.ctl)).unwrap();
    let takes = c.finished_takes();
    assert_eq!(takes.len(), 1, "split {split_it}: {takes:?}");
    let take = &takes[0];
    let slowed = take.slowed.as_ref().expect("played slowed");
    let played: Vec<f32> = hound::WavReader::open(&slowed.as_played)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    (take.start, take.frames, played)
}

#[test]
fn recording_at_a_practice_speed_works_split() {
    let (start, frames, played) = record_slowed(false, 200);
    let (s2, f2, p2) = record_slowed(true, 200);
    assert_eq!(start, Frames(4800));
    assert_eq!(s2, start);
    assert!(played.len() >= 150 * SMALL * CH, "{}", played.len());
    // The same capture, frame for frame. The split take runs on until
    // the stop the worker made reaches the speakers, up to a mix buffer
    // or two later, as heard.
    assert!(p2.len() >= played.len() && p2.len() <= played.len() + 2 * BIG * CH);
    assert_eq!(&p2[..played.len()], &played[..]);
    // Sped back up to the song's speed.
    assert_eq!(frames, (played.len() / CH) / 2);
    assert_eq!(f2, (p2.len() / CH) / 2);
}
