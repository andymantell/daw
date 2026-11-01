// SPDX-License-Identifier: GPL-3.0-or-later
//! A pitch and formant curve over a clip's source (plan B2, pitch
//! editing): how far each moment of the audio is moved in pitch, and
//! how far its formants are moved on top, sampled every few
//! milliseconds of the source. The app builds one from the notes the
//! user has edited; the engine reads it as it plays the clip through
//! the stretcher, so edits are heard at once and nothing is rendered
//! to a file.

/// Pitch and formant moves, in semitones, every `hop` source frames
/// from source frame `first`. Between two samples the move runs evenly;
/// outside them it is nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PitchBend {
    pub first: i64,
    pub hop: u32,
    pub semitones: Vec<f32>,
    pub formants: Vec<f32>,
}

impl PitchBend {
    /// The pitch move and formant move at source frame `src`.
    pub fn at(&self, src: f64) -> (f64, f64) {
        let n = self.semitones.len().min(self.formants.len());
        if n == 0 || self.hop == 0 {
            return (0.0, 0.0);
        }
        let x = (src - self.first as f64) / self.hop as f64;
        if x < 0.0 || x > (n - 1) as f64 {
            return (0.0, 0.0);
        }
        let i = x as usize;
        let f = x - i as f64;
        let j = (i + 1).min(n - 1);
        let lerp = |v: &[f32]| v[i] as f64 + f * (v[j] as f64 - v[i] as f64);
        (lerp(&self.semitones), lerp(&self.formants))
    }

    /// Whether it moves anything at all.
    pub fn is_flat(&self) -> bool {
        self.semitones
            .iter()
            .chain(&self.formants)
            .all(|&v| v == 0.0)
    }
}

/// A pitch line: the pitch of a single voice every `hop` frames from
/// frame 0 of its audio, as a MIDI number with cents as the fraction
/// (69.0 is A4), or NaN where there is no clear pitch. What pitch
/// editing draws and works the drift and vibrato from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PitchLine {
    pub rate: u32,
    pub hop: u32,
    pub midi: Vec<f32>,
}

impl PitchLine {
    /// The pitch at hop `i`, if there is one.
    pub fn get(&self, i: usize) -> Option<f32> {
        self.midi.get(i).copied().filter(|v| v.is_finite())
    }

    /// The hop nearest frame `frame`.
    pub fn hop_of(&self, frame: i64) -> usize {
        (frame.max(0) as f64 / self.hop.max(1) as f64).round() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bend_runs_evenly_between_samples_and_is_nothing_outside() {
        let b = PitchBend {
            first: 100,
            hop: 10,
            semitones: vec![0.0, 1.0, 1.0],
            formants: vec![0.0, 0.0, -2.0],
        };
        assert_eq!(b.at(50.0), (0.0, 0.0));
        assert_eq!(b.at(105.0), (0.5, 0.0));
        assert_eq!(b.at(115.0), (1.0, -1.0));
        assert_eq!(b.at(131.0), (0.0, 0.0));
        assert!(!b.is_flat());
        assert!(PitchBend::default().is_flat());
    }
}
