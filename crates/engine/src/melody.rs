// SPDX-License-Identifier: GPL-3.0-or-later
//! The notes of one voice or instrument, read from its audio: each note
//! sung or played, with how long it lasts and how far off the note it
//! is, and how long each of the twelve notes sounds in all. Made so an
//! assistant that can't hear can tell which key a vocal is in, and how
//! far to move it, from more than a key name.
//!
//! The notes come from the pitch line pitch editing uses
//! ([`crate::pitch_line`]): the tuner's detector (YIN) every 10 ms, on
//! the audio taken down to about 16 kHz, cut into notes where the pitch
//! moves and stays, where the sound stops, or where the level dips
//! between two of the same note. It reads one note at a time: chords
//! and whole mixes read poorly.

use crate::analysis::{KeyGuess, key_of};
use crate::pitch_line;
use crate::tuner::Note;

/// One note heard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    /// Where it starts, in seconds from the start of the audio.
    pub start: f64,
    pub seconds: f64,
    /// The nearest note (MIDI number, 60 is middle C).
    pub midi: i32,
    /// How far the note is from that, −50 to +50 cents, by the middle
    /// of its readings.
    pub cents: f32,
}

/// What the notes of a stretch of audio are.
#[derive(Clone, Debug, PartialEq)]
pub struct Melody {
    pub notes: Vec<Heard>,
    /// Seconds of each of the twelve notes, C first, across octaves.
    pub pitch_classes: [f64; 12],
    /// How far off the notes are as a whole, in cents: the middle of
    /// every note's `cents`, weighted by how long it lasts. A singer a
    /// steady 30 cents flat reads about −30.
    pub tuning_cents: f32,
    /// The key the notes fit best, by how long each sounds.
    pub key: Option<KeyGuess>,
}

/// Reads the notes of `left` and `right`, the same length, at `rate`.
pub fn melody(left: &[f32], right: &[f32], rate: u32) -> Melody {
    let mono: Vec<f32> = (left.iter().zip(right))
        .map(|(l, r)| 0.5 * (l + r))
        .collect();
    let line = pitch_line::line(&mono, rate);
    let notes: Vec<Heard> = pitch_line::notes(&line, &mono)
        .into_iter()
        .map(|n| {
            let note = Note::from_hz(440.0 * 2f32.powf((n.midi - 69.0) / 12.0), 440.0);
            Heard {
                start: n.start as f64 / rate as f64,
                seconds: (n.end - n.start) as f64 / rate as f64,
                midi: note.midi,
                cents: note.cents,
            }
        })
        .collect();
    let mut pitch_classes = [0.0f64; 12];
    for n in &notes {
        pitch_classes[n.midi.rem_euclid(12) as usize] += n.seconds;
    }
    let tuning_cents = {
        let mut by: Vec<(f32, f64)> = notes.iter().map(|n| (n.cents, n.seconds)).collect();
        by.sort_by(|a, b| a.0.total_cmp(&b.0));
        let half = by.iter().map(|x| x.1).sum::<f64>() / 2.0;
        let mut sum = 0.0;
        by.iter()
            .find(|x| {
                sum += x.1;
                sum >= half
            })
            .map_or(0.0, |x| x.0)
    };
    let c: [f32; 12] = pitch_classes.map(|s| s as f32);
    Melody {
        key: key_of(&c),
        notes,
        pitch_classes,
        tuning_cents,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice-like tone: a fundamental and a few harmonics.
    fn tone(midi: f32, seconds: f64, rate: u32) -> Vec<f32> {
        let hz = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
        (0..(seconds * rate as f64) as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (1..4)
                    .map(|h| 0.3 / h as f32 * (std::f32::consts::TAU * hz * h as f32 * t).sin())
                    .sum()
            })
            .collect()
    }

    #[test]
    fn a_tune_reads_note_by_note() {
        let rate = 48_000;
        // C4 D4 E4 F4 G4, the E 30 cents flat, with a rest before G.
        let mut x = Vec::new();
        for (m, s) in [(60.0, 0.4), (62.0, 0.4), (63.7, 0.4), (65.0, 0.4)] {
            x.extend(tone(m, s, rate));
        }
        x.extend(vec![0.0; rate as usize / 5]);
        x.extend(tone(67.0, 0.6, rate));
        let m = melody(&x, &x, rate);
        let names: Vec<i32> = m.notes.iter().map(|n| n.midi).collect();
        assert_eq!(names, [60, 62, 64, 65, 67], "{m:?}");
        let e = m.notes[2];
        assert!((e.cents + 30.0).abs() < 8.0, "{e:?}");
        assert!((m.notes[4].start - 1.8).abs() < 0.05, "{m:?}");
        assert!((m.notes[4].seconds - 0.6).abs() < 0.06, "{m:?}");
        assert!((m.pitch_classes[7] - 0.6).abs() < 0.06, "{m:?}");
        assert!(m.tuning_cents.abs() < 10.0, "{m:?}");
    }

    #[test]
    fn a_singer_flat_throughout_reads_as_flat() {
        let rate = 44_100;
        let mut x = Vec::new();
        for m in [57.0, 59.0, 60.0, 64.0, 62.0, 60.0] {
            x.extend(tone(m - 0.35, 0.3, rate));
        }
        let m = melody(&x, &x, rate);
        assert_eq!(m.notes.len(), 6, "{m:?}");
        assert!((m.tuning_cents + 35.0).abs() < 8.0, "{m:?}");
    }

    #[test]
    fn silence_has_no_notes() {
        let x = vec![0.0; 48_000];
        let m = melody(&x, &x, 48_000);
        assert!(m.notes.is_empty());
        assert!(m.key.is_none());
    }
}
