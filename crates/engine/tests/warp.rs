// SPDX-License-Identifier: GPL-3.0-or-later
//! Stretching a clip between markers (plan A7): tied frames land where
//! the warp says, the audio between keeps its pitch, and the real-time
//! path neither allocates nor drifts.

#[path = "common/rt_check.rs"]
mod rt_check;

use daw_engine::stretch::Feed;
use daw_engine::tuner::{Detector, window};
use daw_engine::warp::{SliceFeed, Warp, WarpPoint, Warper, render};

const RATE: u32 = 48_000;

fn pt(src: i64, out: i64) -> WarpPoint {
    WarpPoint { src, out }
}

/// Where the loudest sample is within `half` frames of `at`.
fn peak_near(l: &[f32], at: usize, half: usize) -> usize {
    let (lo, hi) = (at.saturating_sub(half), (at + half).min(l.len()));
    (lo..hi)
        .max_by(|&a, &b| l[a].abs().total_cmp(&l[b].abs()))
        .unwrap()
}

/// Clicks at the given source frames, and silence otherwise.
fn clicks(frames: usize, at: &[i64]) -> Vec<f32> {
    let mut v = vec![0.0; frames];
    for &a in at {
        v[a as usize] = 1.0;
    }
    v
}

#[test]
fn tied_frames_land_where_the_warp_says() {
    // Clicks every 0.4 s; the warp moves every other one by up to 30%
    // of the gap, as quantising a loose take would.
    let gap = 19_200i64;
    let at: Vec<i64> = (1..12).map(|k| k * gap).collect();
    let src = clicks(gap as usize * 13, &at);
    let mut points = vec![pt(0, 0)];
    for (k, &a) in at.iter().enumerate() {
        let shift = match k % 4 {
            1 => 5_000,
            3 => -5_000,
            _ => 0,
        };
        points.push(pt(a, a + shift));
    }
    points.push(pt(gap * 13, gap * 13));
    let warp = Warp::new(points).unwrap();
    let n = (gap * 13) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let mut feed = SliceFeed {
        left: &src,
        right: &src,
    };
    render(&mut feed, &warp, 0.0, 0, &mut l, &mut r);
    for &a in &at {
        let want = warp.out_at(a as f64) as usize;
        let got = peak_near(&l, want, 4_000);
        let ms = (got as f64 - want as f64) / RATE as f64 * 1000.0;
        // A phase vocoder smears an attack over part of a frame; the
        // loudest point stays within that.
        assert!(ms.abs() < 12.0, "click at {a}: {ms:.1} ms off {want}");
    }
}

#[test]
fn the_audio_between_markers_keeps_its_pitch() {
    let n = 96_000;
    let tone: Vec<f32> = (0..n)
        .map(|t| 0.5 * (std::f32::consts::TAU * 220.0 * t as f32 / RATE as f32).sin())
        .collect();
    for (speed, semis) in [(0.8, 0.0), (1.25, 0.0), (1.0, 2.0)] {
        let warp = Warp::even(0..n as i64, 0, speed).unwrap();
        let out = warp.out_range().end as usize;
        let (mut l, mut r) = (vec![0.0; out], vec![0.0; out]);
        let mut feed = SliceFeed {
            left: &tone,
            right: &tone,
        };
        render(&mut feed, &warp, semis, 0, &mut l, &mut r);
        let want = 220.0 * 2f32.powf(semis as f32 / 12.0);
        let mut d = Detector::new(RATE);
        let got = d
            .detect(&l[out / 2..out / 2 + window(RATE)])
            .expect("a pitch");
        let cents = 1200.0 * (got / want).log2();
        assert!(cents.abs() < 3.0, "speed {speed} semis {semis}: {got} Hz");
    }
}

#[test]
fn real_time_blocks_follow_the_warp_without_allocating_and_restart_on_a_jump() {
    let gap = 19_200i64;
    let at: Vec<i64> = (1..6).map(|k| k * gap).collect();
    let src = clicks(gap as usize * 7, &at);
    let warp = Warp::new(vec![
        pt(0, 0),
        pt(3 * gap, 3 * gap + 6_000),
        pt(7 * gap, 7 * gap),
    ])
    .unwrap();
    let n = (7 * gap) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let mut w = Warper::new(512);
    let mut feed = SliceFeed {
        left: &src,
        right: &src,
    };
    let before = rt_check::violations();
    {
        let _rt = rt_check::RtGuard::enter();
        // Odd block lengths, so runs and blocks do not line up.
        let mut done = 0;
        while done < n {
            let run = 437.min(n - done);
            w.process(
                &mut feed,
                &warp,
                done as f64,
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
    for &a in &at {
        let want = warp.out_at(a as f64) as usize;
        let got = peak_near(&l, want, 4_000);
        let ms = (got as f64 - want as f64) / RATE as f64 * 1000.0;
        assert!(ms.abs() < 12.0, "click at {a}: {ms:.1} ms off");
    }
    // The last block followed on; a jump back starts afresh there.
    assert!(w.continues_at(n as f64));
    assert!(!w.continues_at(1000.0));
    let (mut bl, mut br) = (vec![0.0; 512], vec![0.0; 512]);
    w.process(&mut feed, &warp, (gap - 256) as f64, &mut bl, &mut br);
    let got = peak_near(&bl, 256, 200);
    assert!(
        (got as i64 - 256).abs() < 200,
        "after a jump, the click at {got}"
    );
}

/// Reading the source at a position: the feed's contract past the ends.
#[test]
fn a_slice_feed_is_silent_outside_the_audio() {
    let a = [1.0, 2.0, 3.0];
    let mut f = SliceFeed {
        left: &a,
        right: &a,
    };
    let (mut l, mut r) = ([9.0; 6], [9.0; 6]);
    f.read(-2, &mut l, &mut r);
    assert_eq!(l, [0.0, 0.0, 1.0, 2.0, 3.0, 0.0]);
    assert_eq!(r, l);
}
