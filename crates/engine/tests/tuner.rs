// SPDX-License-Identifier: GPL-3.0-or-later
//! The chromatic tuner (roadmap 1.4): pitch detection across guitar and
//! bass range, note naming, and input reaching it from the engine.

use daw_engine::engine::{self, Command};
use daw_engine::tuner::{Detector, Note, window};

const RATE: u32 = 48_000;

/// A plucked-string-like tone: harmonics falling off, with the
/// fundamental weaker than the second harmonic, as on a low string.
fn string(hz: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let w = std::f32::consts::TAU * hz * t;
            0.3 * w.sin() + 0.5 * (2.0 * w).sin() + 0.2 * (3.0 * w).sin() + 0.1 * (5.0 * w).sin()
        })
        .collect()
}

fn cents(a: f32, b: f32) -> f32 {
    1200.0 * (a / b).log2()
}

#[test]
fn finds_guitar_and_bass_strings_within_a_cent() {
    let mut d = Detector::new(RATE);
    // Five-string bass low B to the guitar's top E at the 12th fret.
    for hz in [
        30.87, 41.20, 55.0, 82.41, 110.0, 146.83, 196.0, 246.94, 329.63, 659.26,
    ] {
        let got = d.detect(&string(hz, window(RATE))).expect("a pitch");
        assert!(cents(got, hz).abs() < 1.0, "{hz} Hz read as {got}");
    }
}

#[test]
fn silence_has_no_pitch() {
    let mut d = Detector::new(RATE);
    assert_eq!(d.detect(&vec![0.0; window(RATE)]), None);
    let hiss: Vec<f32> = (0..window(RATE))
        .map(|i| ((i * 7919) % 101) as f32 / 101.0 * 1e-4)
        .collect();
    assert_eq!(d.detect(&hiss), None);
}

#[test]
fn notes_are_named_against_the_reference_pitch() {
    let n = Note::from_hz(440.0, 440.0);
    assert_eq!((n.name(), n.octave(), n.midi), ("A", 4, 69));
    assert!(n.cents.abs() < 1e-3);
    let e = Note::from_hz(82.41, 440.0);
    assert_eq!((e.name(), e.octave()), ("E", 2));
    // Tuned to A = 432, a 440 A reads about 32 cents sharp.
    let sharp = Note::from_hz(440.0, 432.0);
    assert_eq!(sharp.name(), "A");
    assert!((sharp.cents - 31.77).abs() < 0.1, "{}", sharp.cents);
    // A quarter-tone up rounds to the next note, flat.
    let q = Note::from_hz(440.0 * 2f32.powf(0.6 / 12.0), 440.0);
    assert_eq!(q.name(), "A♯");
    assert!((q.cents + 40.0).abs() < 0.1);
}

#[test]
fn the_engine_sends_the_chosen_input_channel() {
    let (mut ctl, mut eng) = engine::new(256);
    ctl.send(Command::SetTunerInput(Some(1))).ok().unwrap();
    // Two input channels; the tuner wants the second.
    let input: Vec<f32> = (0..256).flat_map(|i| [-1.0, i as f32]).collect();
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    eng.process_duplex(&input, 2, &mut l, &mut r, None);
    let mut got = Vec::new();
    ctl.take_tuner_samples(&mut got, 100);
    // Only the newest 100 are kept.
    let want: Vec<f32> = (156..256).map(|i| i as f32).collect();
    assert_eq!(got, want);

    ctl.send(Command::SetTunerInput(None)).ok().unwrap();
    eng.process_duplex(&input, 2, &mut l, &mut r, None);
    got.clear();
    ctl.take_tuner_samples(&mut got, 1000);
    assert!(got.is_empty());
}
