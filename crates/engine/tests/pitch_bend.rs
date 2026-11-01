// SPDX-License-Identifier: GPL-3.0-or-later
//! Pitch editing in the engine (plan B2): a clip's pitch bend moves
//! each part of the audio by its own amount as it plays, the formants
//! can be moved on their own, notes can be moved in time by composing
//! warps, and the bent path doesn't allocate on the audio thread.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::f32::consts::TAU;
use std::sync::Arc;

use daw_engine::bend::PitchBend;
use daw_engine::engine::{self, Command};
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::stretch::Stretcher;
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};
use daw_engine::tuner::{Detector, window};
use daw_engine::warp::{SliceFeed, Warp, WarpPoint};

const RATE: u32 = 48_000;

fn tone(hz: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|t| 0.5 * (TAU * hz * t as f32 / RATE as f32).sin())
        .collect()
}

fn pitch_at(l: &[f32], centre: usize) -> f32 {
    let w = window(RATE);
    Detector::new(RATE)
        .detect(&l[centre - w / 2..centre + w / 2])
        .expect("a pitch")
}

fn cents(got: f32, want: f32) -> f32 {
    1200.0 * (got / want).log2()
}

fn session(clip: ClipSpec) -> Session {
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    let mut t = TrackSpec::new(vec![clip]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    s
}

/// A bend of nothing for the first second, then `semis` for the next.
fn step_bend(semis: f32, formants: f32) -> PitchBend {
    let hop = 480;
    let n = 2 * RATE as usize / hop + 1;
    PitchBend {
        first: 0,
        hop: hop as u32,
        semitones: (0..n)
            .map(|i| if i * hop >= RATE as usize { semis } else { 0.0 })
            .collect(),
        formants: vec![formants; n],
    }
}

#[test]
fn a_bend_moves_one_part_of_a_clip_and_leaves_the_rest() {
    let n = 2 * RATE as usize;
    let data = Arc::new(AudioData {
        channels: vec![tone(220.0, n)],
        rate: RATE,
    });
    let mut clip = ClipSpec::new(Frames(0), data);
    clip.bend = Some(Arc::new(step_bend(2.0, 0.0)));
    clip.tag = 4;
    let [l, _] = engine::render_offline(session(clip).compile(512).unwrap(), Frames(0), n, 512);
    let first = pitch_at(&l, RATE as usize / 2);
    let second = pitch_at(&l, 3 * RATE as usize / 2);
    assert!(cents(first, 220.0).abs() < 4.0, "first half {first} Hz");
    let want = 220.0 * 2f32.powf(2.0 / 12.0);
    assert!(
        cents(second, want).abs() < 4.0,
        "second half {second} Hz, want {want}"
    );
}

#[test]
fn a_bend_adds_to_the_clips_own_transpose() {
    let n = 2 * RATE as usize;
    let data = Arc::new(AudioData {
        channels: vec![tone(220.0, n)],
        rate: RATE,
    });
    let mut clip = ClipSpec::new(Frames(0), data);
    clip.semitones = -3.0;
    clip.bend = Some(Arc::new(step_bend(1.0, 0.0)));
    clip.tag = 5;
    let [l, _] = engine::render_offline(session(clip).compile(512).unwrap(), Frames(0), n, 512);
    let second = pitch_at(&l, 3 * RATE as usize / 2);
    let want = 220.0 * 2f32.powf(-2.0 / 12.0);
    assert!(cents(second, want).abs() < 4.0, "{second} Hz, want {want}");
}

/// The level of `x` around `hz`, by correlation.
fn level_at(x: &[f32], hz: f32) -> f32 {
    let (mut re, mut im) = (0.0f32, 0.0f32);
    let n = x.len();
    for (i, &v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos();
        let p = TAU * hz * i as f32 / RATE as f32;
        re += v * w * p.cos();
        im += v * w * p.sin();
    }
    re.hypot(im)
}

#[test]
fn a_formant_shift_moves_the_resonance_and_not_the_pitch() {
    // Harmonics of 150 Hz shaped by a broad resonance at 1.2 kHz: the
    // cepstral envelope follows resonances about this wide.
    let (f0, formant) = (150.0f32, 1_200.0f32);
    let n = 72_000;
    let res = |f: f32| 1.0 / (1.0 + ((f - formant) / 500.0).powi(2));
    let src: Vec<f32> = (0..n)
        .map(|t| {
            let s = t as f32 / RATE as f32;
            (1..50)
                .map(|h| h as f32 * f0)
                .map(|f| 0.1 * res(f) * (TAU * f * s).sin())
                .sum()
        })
        .collect();
    let mut st = Stretcher::new(512);
    st.set_formants(true);
    st.set_formant_shift(5.0);
    st.set(1.0, 0.0);
    st.reset(0.0);
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let mut feed = SliceFeed {
        left: &src,
        right: &src,
    };
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        st.process(&mut feed, a, b);
    }
    let mid = &l[24_000..24_000 + 8_192];
    let loudest = (1..40)
        .map(|h| h as f32 * f0)
        .max_by(|a, b| level_at(mid, *a).total_cmp(&level_at(mid, *b)))
        .unwrap();
    let want = formant * 2f32.powf(5.0 / 12.0);
    assert!(
        (loudest - want).abs() <= f0,
        "the resonance should move up to about {want}: {loudest}"
    );
    let got = pitch_at(&l, 36_000);
    assert!(cents(got, f0).abs() < 10.0, "pitch {got}, want {f0}");
}

#[test]
fn notes_move_in_time_through_a_composed_warp() {
    // A 220 Hz tone that turns to 330 Hz at 1 s; the change is moved
    // 0.25 s later by an inner warp, inside a clip played at its own
    // speed.
    let n = 2 * RATE as usize;
    let mut src = tone(220.0, RATE as usize);
    src.extend(tone(330.0, RATE as usize));
    let data = Arc::new(AudioData {
        channels: vec![src],
        rate: RATE,
    });
    let outer = Warp::even(0..n as i64, 0, 1.0).unwrap();
    let r = RATE as i64;
    let inner = Warp::new(vec![
        WarpPoint { src: 0, out: 0 },
        WarpPoint {
            src: r,
            out: r + r / 4,
        },
        WarpPoint {
            src: 2 * r,
            out: 2 * r,
        },
    ])
    .unwrap();
    let warp = outer.then(&inner).unwrap();
    let mut clip = ClipSpec::new(Frames(0), data);
    clip.warp = Some(Arc::new(warp));
    clip.tag = 6;
    let [l, _] = engine::render_offline(session(clip).compile(512).unwrap(), Frames(0), n, 512);
    // At 1.1 s it is still the first note; by 1.5 s the second.
    let early = pitch_at(&l, RATE as usize * 11 / 10);
    let late = pitch_at(&l, RATE as usize * 3 / 2);
    assert!(cents(early, 220.0).abs() < 10.0, "{early} Hz at 1.1 s");
    assert!(cents(late, 330.0).abs() < 10.0, "{late} Hz at 1.5 s");
}

#[test]
fn a_bent_clip_does_not_allocate_as_it_plays() {
    let n = RATE as usize;
    let data = Arc::new(AudioData {
        channels: vec![tone(220.0, 2 * n)],
        rate: RATE,
    });
    let mut clip = ClipSpec::new(Frames(0), data);
    clip.bend = Some(Arc::new(step_bend(3.0, -2.0)));
    clip.tag = 7;
    let (mut ctl, mut eng) = engine::new(512);
    ctl.set_graph(session(clip).compile(512).unwrap())
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
    // Warm up, so first-time set-up is not counted.
    eng.process(&mut l, &mut r);
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        for _ in 0..(2 * n / 512) {
            eng.process(&mut l, &mut r);
        }
    }
    assert_eq!(rt_check::violations(), before, "the bent path allocated");
}
