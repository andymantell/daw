// SPDX-License-Identifier: GPL-3.0-or-later
//! Clip fades, crossfades, gain and gain envelopes, as played.

use std::sync::Arc;

use daw_engine::engine;
use daw_engine::fade::{Curve, Fade, GainPoint};
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};

const RATE: u32 = 48_000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

fn ones(n: usize) -> Arc<AudioData> {
    Arc::new(AudioData {
        channels: vec![vec![1.0; n]],
        rate: RATE,
    })
}

/// The left channel of `clips` on one track panned hard left.
fn play(clips: Vec<ClipSpec>, frames: usize) -> Vec<f32> {
    let mut s = session();
    let mut t = TrackSpec::new(clips);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let [l, _] = engine::render_offline(s.compile(128).unwrap(), Frames(0), frames, 128);
    l
}

#[test]
fn fades_shape_the_start_and_end() {
    let mut c = ClipSpec::new(Frames(100), ones(1000));
    c.fade_in = Fade::new(200, Curve::Linear);
    c.fade_out = Fade::new(100, Curve::Slow);
    let l = play(vec![c], 1200);
    assert_eq!(l[99], 0.0);
    assert_eq!(l[100], 0.0, "silent on its first frame");
    assert!((l[200] - 0.5).abs() < 1e-6, "half way in");
    assert_eq!(l[600], 1.0);
    // 50 frames from the end of a 100-frame slow fade: (49/100)².
    assert!((l[1050] - 0.49 * 0.49).abs() < 1e-5, "{}", l[1050]);
    assert_eq!(l[1099], 0.0, "silent on its last frame");
}

#[test]
fn fades_longer_than_the_clip_are_shortened_to_fit() {
    let mut c = ClipSpec::new(Frames(0), ones(100));
    c.fade_in = Fade::new(150, Curve::Linear);
    c.fade_out = Fade::new(50, Curve::Linear);
    let l = play(vec![c], 100);
    // 75 and 25 frames: the peak is where they meet.
    assert!((l[74] - 74.0 / 75.0).abs() < 1e-5);
    assert!((l[75] - 24.0 / 25.0).abs() < 1e-5);
    assert!(l.iter().all(|&x| x <= 1.0));
}

#[test]
fn an_equal_power_crossfade_keeps_the_power_steady() {
    // Two clips overlap by 1000 frames: the first fades out as the
    // second fades in. Different sounds (one positive, one negative
    // half the time) would add as powers, so check that.
    let mut a = ClipSpec::new(Frames(0), ones(3000));
    a.fade_out = Fade::new(1000, Curve::EqualPower);
    let mut b = ClipSpec::new(Frames(2000), ones(3000));
    b.fade_in = Fade::new(1000, Curve::EqualPower);
    let la = play(vec![a.clone()], 5000);
    let lb = play(vec![b.clone()], 5000);
    for i in 1990..3010 {
        let p = la[i] * la[i] + lb[i] * lb[i];
        assert!((p - 1.0).abs() < 2e-3, "frame {i}: {p}");
    }
    let both = play(vec![a, b], 5000);
    assert!((both[2500] - (la[2500] + lb[2500])).abs() < 1e-6);
}

#[test]
fn clip_gain_and_its_envelope_set_the_level() {
    let mut c = ClipSpec::new(Frames(0), ones(1000));
    c.offset = 100;
    c.gain_db = -6.0;
    // In source frames: from 0 dB at 300 to −20 dB at 500.
    c.envelope = Arc::new([
        GainPoint { at: 300, db: 0.0 },
        GainPoint { at: 500, db: -20.0 },
    ]);
    let l = play(vec![c], 900);
    let g6 = 10f32.powf(-6.0 / 20.0);
    assert!((l[0] - g6).abs() < 1e-6, "before the first point");
    // Clip frame 300 is source frame 400: −10 dB on the line.
    assert!((l[300] - g6 * 10f32.powf(-0.5)).abs() < 1e-5);
    assert!((l[800] - g6 * 0.1).abs() < 1e-6, "after the last point");
}
