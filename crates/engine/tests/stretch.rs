// SPDX-License-Identifier: GPL-3.0-or-later
//! The practice stretcher (roadmap 1.4): pitch held at any speed, pitch
//! moved by transpose, output in time with the input, no allocation.

#[path = "common/rt_check.rs"]
mod rt_check;

use daw_engine::stretch::{Feed, Stretcher};
use daw_engine::tuner::{Detector, window};

const RATE: u32 = 48_000;

/// A steady tone, or silence with clicks, readable anywhere.
struct Source {
    hz: f32,
    clicks_every: Option<i64>,
}

impl Feed for Source {
    fn read(&mut self, start: i64, l: &mut [f32], r: &mut [f32]) {
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let t = start + i as i64;
            let v = match self.clicks_every {
                Some(p) => {
                    if t >= 0 && t % p == 0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                None => 0.5 * (std::f32::consts::TAU * self.hz * t as f32 / RATE as f32).sin(),
            };
            *a = v;
            *b = -v;
        }
    }
}

fn run(s: &mut Stretcher, src: &mut Source, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
    for (bl, br) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        s.process(src, bl, br);
    }
    (l, r)
}

fn cents(a: f32, b: f32) -> f32 {
    1200.0 * (a / b).log2()
}

#[test]
fn speed_keeps_pitch_and_transpose_moves_it() {
    for (speed, semis) in [
        (0.5, 0.0),
        (0.75, 0.0),
        (1.0, 0.0),
        (1.5, 0.0),
        (1.0, -1.0),
        (0.75, 2.0),
        (1.0, 12.0),
        (1.0, -12.0),
    ] {
        let mut s = Stretcher::new(256);
        s.set(speed, semis);
        s.reset(0.0);
        let mut src = Source {
            hz: 220.0,
            clicks_every: None,
        };
        let (l, r) = run(&mut s, &mut src, 48_000);
        let want = 220.0 * 2f32.powf(semis as f32 / 12.0);
        let mut d = Detector::new(RATE);
        let got = d.detect(&l[l.len() - window(RATE)..]).expect("a pitch");
        assert!(
            cents(got, want).abs() < 3.0,
            "speed {speed} semis {semis}: {got} Hz, want {want}"
        );
        // Channels stay opposite, as the input was.
        let err = l
            .iter()
            .zip(&r)
            .skip(4096)
            .map(|(a, b)| (a + b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-3, "channels drifted: {err}");
    }
}

#[test]
fn output_stays_in_time_with_the_input() {
    // A click every half second; at speed s, output frame o plays input
    // o × s, so click k (input k × 24000) lands near output k × 24000 / s.
    for speed in [0.5, 0.8, 1.0, 1.25, 1.5] {
        let mut s = Stretcher::new(256);
        s.set(speed, 0.0);
        s.reset(0.0);
        let mut src = Source {
            hz: 0.0,
            clicks_every: Some(24_000),
        };
        let (l, _) = run(&mut s, &mut src, 160_000);
        for k in 1..4 {
            let at = (k as f64 * 24_000.0 / speed) as usize;
            let (lo, hi) = (at.saturating_sub(2400), (at + 2400).min(l.len()));
            let peak = (lo..hi)
                .max_by(|&a, &b| l[a].abs().total_cmp(&l[b].abs()))
                .unwrap();
            let ms = (peak as f64 - at as f64) / RATE as f64 * 1000.0;
            // A phase vocoder smears an attack over part of a frame, more
            // when slowed (spike 0.6 measured 37 ms of smear at 50%), so
            // the loudest point can sit a little early.
            assert!(ms.abs() < 20.0, "speed {speed}, click {k}: {ms:.1} ms off");
        }
    }
}

#[test]
fn reset_lines_up_and_processing_does_not_allocate() {
    let mut s = Stretcher::new(512);
    s.set(0.7, 3.0);
    s.reset(10_000.0);
    assert!((s.position() - 10_000.0).abs() < 1.0, "{}", s.position());
    let mut src = Source {
        hz: 330.0,
        clicks_every: None,
    };
    let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
    s.process(&mut src, &mut l, &mut r);
    assert!(
        (s.position() - (10_000.0 + 512.0 * 0.7)).abs() < 2.0,
        "{}",
        s.position()
    );
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        for i in 0..200 {
            s.set(0.5 + (i % 10) as f64 * 0.1, (i % 25) as f64 - 12.0);
            s.process(&mut src, &mut l, &mut r);
        }
    }
    assert_eq!(rt_check::violations(), before);
}
