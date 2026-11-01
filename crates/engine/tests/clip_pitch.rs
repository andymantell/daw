// SPDX-License-Identifier: GPL-3.0-or-later
//! A clip's own transpose (plan B2's first step): the pitch moves by
//! semitones and cents, the formants stay where they were when asked,
//! it plays through the graph whether or not the clip follows tempo,
//! and the real-time path doesn't allocate.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::f32::consts::TAU;
use std::sync::Arc;

use daw_engine::engine;
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};
use daw_engine::tuner::{Detector, window};
use daw_engine::warp::{SliceFeed, Warp, Warper, render_with};

const RATE: u32 = 48_000;

fn tone(hz: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|t| 0.5 * (TAU * hz * t as f32 / RATE as f32).sin())
        .collect()
}

fn pitch_of(l: &[f32]) -> f32 {
    let mid = l.len() / 2;
    Detector::new(RATE)
        .detect(&l[mid..mid + window(RATE)])
        .expect("a pitch")
}

fn cents(got: f32, want: f32) -> f32 {
    1200.0 * (got / want).log2()
}

/// A vowel-like buzz: harmonics of `f0` shaped by one resonance at
/// `formant` Hz.
fn buzz(f0: f32, formant: f32, n: usize) -> Vec<f32> {
    let res = |f: f32| 1.0 / (1.0 + ((f - formant) / 250.0).powi(2));
    let harmonics: Vec<(f32, f32)> = (1..)
        .map(|h| h as f32 * f0)
        .take_while(|&f| f < 8_000.0)
        .map(|f| (f, res(f)))
        .collect();
    (0..n)
        .map(|t| {
            let s = t as f32 / RATE as f32;
            0.1 * harmonics
                .iter()
                .map(|&(f, a)| a * (TAU * f * s).sin())
                .sum::<f32>()
        })
        .collect()
}

/// The loudest harmonic of `f0` in `x`, in Hz: where the resonance is.
fn loudest_harmonic(x: &[f32], f0: f32) -> f32 {
    // A Hann-windowed correlation at each harmonic.
    let n = x.len();
    let w = |i: usize| 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos();
    (1..)
        .map(|h| h as f32 * f0)
        .take_while(|&f| f < 6_000.0)
        .map(|f| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, &v) in x.iter().enumerate() {
                let p = TAU * f * i as f32 / RATE as f32;
                re += v * w(i) * p.cos();
                im += v * w(i) * p.sin();
            }
            (f, re.hypot(im))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0
}

fn transpose(src: &[f32], semis: f64, formants: bool) -> Vec<f32> {
    let n = src.len();
    let warp = Warp::even(0..n as i64, 0, 1.0).unwrap();
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let mut feed = SliceFeed {
        left: src,
        right: src,
    };
    render_with(&mut feed, &warp, semis, formants, 0, &mut l, &mut r);
    l
}

#[test]
fn semitones_and_cents_move_the_pitch_with_or_without_formants() {
    let src = tone(220.0, 96_000);
    for semis in [7.0, -6.0, 0.35, -11.5] {
        for formants in [false, true] {
            let got = pitch_of(&transpose(&src, semis, formants));
            let want = 220.0 * 2f32.powf(semis as f32 / 12.0);
            assert!(
                cents(got, want).abs() < 4.0,
                "{semis} semitones, formants {formants}: {got} Hz, want {want}"
            );
        }
    }
}

#[test]
fn keeping_formants_leaves_the_resonance_where_it_was() {
    let (f0, formant) = (150.0, 1_200.0);
    let src = buzz(f0, formant, 72_000);
    let semis = 5.0;
    let ratio = 2f32.powf(semis / 12.0);
    let new_f0 = f0 * ratio;
    let mid = |x: &[f32]| x[24_000..24_000 + 8_192].to_vec();
    // As recorded, the loudest harmonic is the one nearest the
    // resonance.
    assert!((loudest_harmonic(&mid(&src), f0) - formant).abs() <= f0);
    // Moved with the pitch, like tape sped up.
    let moved = loudest_harmonic(&mid(&transpose(&src, semis as f64, false)), new_f0);
    assert!(
        (moved - formant * ratio).abs() <= new_f0,
        "without formants the resonance should move to {}: {moved}",
        formant * ratio
    );
    // Kept: still at about 1.2 kHz, while the pitch has gone up.
    let kept_out = transpose(&src, semis as f64, true);
    let kept = loudest_harmonic(&mid(&kept_out), new_f0);
    assert!(
        (kept - formant).abs() <= new_f0,
        "with formants kept the resonance should stay near {formant}: {kept}"
    );
    let got = pitch_of(&kept_out);
    assert!(
        cents(got, new_f0).abs() < 10.0,
        "pitch {got}, want {new_f0}"
    );
}

#[test]
fn a_clip_plays_transposed_through_the_graph_in_its_own_time() {
    let n = 96_000;
    let data = Arc::new(AudioData {
        channels: vec![tone(220.0, n)],
        rate: RATE,
    });
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    let mut clip = ClipSpec::new(Frames(24_000), data);
    clip.semitones = 3.0;
    clip.tag = 9;
    let mut t = TrackSpec::new(vec![clip]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let total = 24_000 + n + 2_000;
    let [l, _] = engine::render_offline(s.compile(512).unwrap(), Frames(0), total, 512);
    // Silent before it, and it lasts as long as it did.
    assert!(l[..23_000].iter().all(|x| x.abs() < 1e-4));
    assert!(l[24_000 + n + 1_000..].iter().all(|x| x.abs() < 1e-3));
    let want = 220.0 * 2f32.powf(3.0 / 12.0);
    let got = pitch_of(&l[24_000..24_000 + n]);
    assert!(cents(got, want).abs() < 4.0, "{got} Hz, want {want}");
}

#[test]
fn a_transpose_adds_to_follow_tempo_stretching() {
    let n = 96_000;
    let data = Arc::new(AudioData {
        channels: vec![tone(220.0, n)],
        rate: RATE,
    });
    let mut s = Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON));
    let mut clip = ClipSpec::new(Frames(0), data);
    // Slowed to 80% and down two semitones.
    let warp = Arc::new(Warp::even(0..n as i64, 0, 0.8).unwrap());
    clip.warp = Some(warp.clone());
    clip.semitones = -2.0;
    clip.tag = 3;
    let mut t = TrackSpec::new(vec![clip]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let out = warp.out_range().end as usize;
    let [l, _] = engine::render_offline(s.compile(512).unwrap(), Frames(0), out, 512);
    let want = 220.0 * 2f32.powf(-2.0 / 12.0);
    let got = pitch_of(&l);
    assert!(cents(got, want).abs() < 4.0, "{got} Hz, want {want}");
}

#[test]
fn keeping_formants_in_real_time_does_not_allocate() {
    let n = 48_000;
    let src = buzz(150.0, 1_200.0, n);
    let warp = Warp::even(0..n as i64, 0, 1.0).unwrap();
    let mut w = Warper::new(512);
    w.set_pitch(-4.0);
    w.set_formants(true);
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let mut feed = SliceFeed {
        left: &src,
        right: &src,
    };
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        let mut done = 0;
        while done < n {
            let run = 437.min(n - done);
            w.process(
                &mut feed,
                &warp,
                done as f64,
                1.0,
                &mut l[done..done + run],
                &mut r[done..done + run],
            );
            done += run;
        }
    }
    assert_eq!(
        rt_check::violations(),
        before,
        "the real-time path allocated"
    );
    assert!(l.iter().any(|x| x.abs() > 0.01));
}
