// SPDX-License-Identifier: GPL-3.0-or-later
//! Every candidate holds pitch, keeps time and never allocates while
//! processing.

use spike_engine::rt_check::{self, RtGuard};
use stretch_bench::signals::{cents, estimate_f0, harmonic_tone};
use stretch_bench::{CANDIDATES, Feed, RATE, build};

const BLOCK: usize = 256;

#[test]
fn pitch_rate_and_no_allocation() {
    let tone = harmonic_tone(6.0);
    for name in CANDIDATES {
        for (speed, semis) in [(0.5, 0.0), (1.5, 0.0), (1.0, 12.0), (0.75, -1.0)] {
            let mut s = build(name, BLOCK);
            if !s.set(speed, semis) {
                continue;
            }
            let mut feed = Feed::new(&tone);
            let mut out = vec![0.0f32; 3 * RATE as usize];
            let (first, rest) = out.split_at_mut(RATE as usize);
            for c in first.chunks_mut(BLOCK) {
                s.process(&mut feed, c);
            }
            let c0 = feed.consumed;
            let before = rt_check::violations();
            {
                let _rt = RtGuard::enter();
                for c in rest.chunks_mut(BLOCK) {
                    s.process(&mut feed, c);
                }
            }
            assert_eq!(
                rt_check::violations(),
                before,
                "{name} allocated at {speed}x {semis}st"
            );
            let rate = (feed.consumed - c0) / rest.len() as f64;
            assert!(
                (rate / speed - 1.0).abs() < 0.005,
                "{name} rate {rate} at {speed}x"
            );
            let want = 220.0 * 2f32.powf(semis as f32 / 12.0);
            let got = estimate_f0(&out[RATE as usize * 3 / 2..RATE as usize * 5 / 2]);
            let off = cents(got, want);
            assert!(
                off.abs() < 5.0,
                "{name} {off:+.1} cents at {speed}x {semis}st"
            );
        }
    }
}

#[test]
fn own_vocoder_passes_audio_through_at_unity() {
    let tone = harmonic_tone(2.0);
    let mut s = build("own-2048", BLOCK);
    s.set(1.0, 0.0);
    let mut feed = Feed::new(&tone);
    let mut out = vec![0.0f32; RATE as usize];
    for c in out.chunks_mut(BLOCK) {
        s.process(&mut feed, c);
    }
    // Find the delay, then compare.
    let seg = &out[RATE as usize / 2..RATE as usize / 2 + 4800];
    let (delay, err) = (0..256)
        .map(|d| {
            let e: f32 = seg
                .iter()
                .enumerate()
                .map(|(i, y)| (y - tone[RATE as usize / 2 + i - d]).powi(2))
                .sum();
            (d, e / seg.len() as f32)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let power: f32 = seg.iter().map(|y| y * y).sum::<f32>() / seg.len() as f32;
    assert!(
        err < power * 1e-3,
        "delay {delay}: error {err} vs power {power}"
    );
}
