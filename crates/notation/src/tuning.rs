// SPDX-License-Identifier: GPL-3.0-or-later
//! String tunings for tab: any string count, any tuning.

/// Open-string pitches as MIDI note numbers, from string 1 (the
/// highest-pitched string) down to the lowest.
#[derive(Clone, Debug, PartialEq)]
pub struct Tuning {
    pub name: String,
    pub strings: Vec<u8>,
}

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

impl Tuning {
    pub fn new(name: &str, strings_high_to_low: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            strings: strings_high_to_low.to_vec(),
        }
    }

    pub fn guitar_standard() -> Self {
        Self::new("Standard", &[64, 59, 55, 50, 45, 40])
    }

    pub fn guitar_drop_d() -> Self {
        Self::new("Drop D", &[64, 59, 55, 50, 45, 38])
    }

    pub fn guitar_open_g() -> Self {
        Self::new("Open G", &[62, 59, 55, 50, 43, 38])
    }

    pub fn guitar_open_d() -> Self {
        Self::new("Open D", &[62, 57, 54, 50, 45, 38])
    }

    pub fn guitar_dadgad() -> Self {
        Self::new("DADGAD", &[62, 57, 55, 50, 45, 38])
    }

    pub fn guitar_7_string() -> Self {
        Self::new("7-string standard", &[64, 59, 55, 50, 45, 40, 35])
    }

    pub fn guitar_8_string() -> Self {
        Self::new("8-string standard", &[64, 59, 55, 50, 45, 40, 35, 30])
    }

    pub fn bass_4() -> Self {
        Self::new("Bass standard", &[43, 38, 33, 28])
    }

    pub fn bass_5() -> Self {
        Self::new("5-string bass", &[43, 38, 33, 28, 23])
    }

    pub fn bass_6() -> Self {
        Self::new("6-string bass", &[48, 43, 38, 33, 28, 23])
    }

    /// The tunings offered by name, for menus.
    pub fn presets() -> Vec<Tuning> {
        vec![
            Self::guitar_standard(),
            Self::guitar_drop_d(),
            Self::guitar_open_g(),
            Self::guitar_open_d(),
            Self::guitar_dadgad(),
            Self::guitar_7_string(),
            Self::guitar_8_string(),
            Self::bass_4(),
            Self::bass_5(),
            Self::bass_6(),
        ]
    }

    pub fn string_count(&self) -> usize {
        self.strings.len()
    }

    /// Open pitch of a string (numbered from 1) with a capo on.
    pub fn open_pitch(&self, string: u8, capo: u8) -> Option<u8> {
        let i = usize::from(string).checked_sub(1)?;
        self.strings.get(i).map(|p| p.saturating_add(capo))
    }

    /// Sounding pitch of a string and fret (counted from the capo).
    pub fn pitch(&self, string: u8, fret: u8, capo: u8) -> Option<u8> {
        self.open_pitch(string, capo)
            .map(|p| p.saturating_add(fret))
    }

    /// The written bass-clef or treble-clef family the tuning reads in:
    /// true when its lowest string is below the guitar's low E by more than
    /// a fifth, as basses are.
    pub fn is_bass(&self) -> bool {
        self.strings.iter().copied().min().unwrap_or(40) < 33 && self.string_count() <= 6
    }

    /// Letter names from the lowest string up, such as "EADGBE".
    pub fn letters(&self) -> String {
        self.strings
            .iter()
            .rev()
            .map(|&p| pitch_class_name(p))
            .collect()
    }
}

/// "C", "C#" ... for a MIDI pitch.
pub fn pitch_class_name(pitch: u8) -> &'static str {
    NAMES[usize::from(pitch % 12)]
}

/// "E2", "A#3" ... for a MIDI pitch, with middle C as C4.
pub fn pitch_name(pitch: u8) -> String {
    format!("{}{}", pitch_class_name(pitch), i32::from(pitch) / 12 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitches_follow_tuning_and_capo() {
        let t = Tuning::guitar_standard();
        assert_eq!(t.pitch(6, 0, 0), Some(40));
        assert_eq!(t.pitch(1, 5, 0), Some(69));
        assert_eq!(t.pitch(3, 2, 2), Some(59));
        assert_eq!(t.pitch(7, 0, 0), None);
        assert_eq!(t.letters(), "EADGBE");
        assert_eq!(Tuning::guitar_dadgad().letters(), "DADGAD");
        assert_eq!(pitch_name(60), "C4");
        assert!(Tuning::bass_4().is_bass());
        assert!(Tuning::bass_5().is_bass());
        assert!(!Tuning::guitar_8_string().is_bass());
    }
}
