// SPDX-License-Identifier: GPL-3.0-or-later
//! Keys and scales (roadmap 3.1): a project has one, and a note clip
//! can have its own. The note editor shades the notes in the scale,
//! keeps drawn and moved notes on it while scale mode is on, and can
//! move notes already there onto it.

/// A kind of scale, as the steps from its root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScaleKind {
    Major,
    Minor,
    HarmonicMinor,
    MelodicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
}

impl ScaleKind {
    pub const ALL: [ScaleKind; 12] = [
        ScaleKind::Major,
        ScaleKind::Minor,
        ScaleKind::HarmonicMinor,
        ScaleKind::MelodicMinor,
        ScaleKind::Dorian,
        ScaleKind::Phrygian,
        ScaleKind::Lydian,
        ScaleKind::Mixolydian,
        ScaleKind::Locrian,
        ScaleKind::MajorPentatonic,
        ScaleKind::MinorPentatonic,
        ScaleKind::Blues,
    ];

    /// Semitones above the root of each note of the scale.
    pub fn steps(self) -> &'static [u8] {
        match self {
            ScaleKind::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleKind::Minor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleKind::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleKind::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleKind::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            ScaleKind::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            ScaleKind::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            ScaleKind::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            ScaleKind::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            ScaleKind::MajorPentatonic => &[0, 2, 4, 7, 9],
            ScaleKind::MinorPentatonic => &[0, 3, 5, 7, 10],
            ScaleKind::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ScaleKind::Major => "major",
            ScaleKind::Minor => "minor",
            ScaleKind::HarmonicMinor => "harmonic minor",
            ScaleKind::MelodicMinor => "melodic minor",
            ScaleKind::Dorian => "Dorian",
            ScaleKind::Phrygian => "Phrygian",
            ScaleKind::Lydian => "Lydian",
            ScaleKind::Mixolydian => "Mixolydian",
            ScaleKind::Locrian => "Locrian",
            ScaleKind::MajorPentatonic => "major pentatonic",
            ScaleKind::MinorPentatonic => "minor pentatonic",
            ScaleKind::Blues => "blues",
        }
    }

    /// The name a project stores. Never change one.
    pub fn name(self) -> &'static str {
        match self {
            ScaleKind::Major => "major",
            ScaleKind::Minor => "minor",
            ScaleKind::HarmonicMinor => "harmonic_minor",
            ScaleKind::MelodicMinor => "melodic_minor",
            ScaleKind::Dorian => "dorian",
            ScaleKind::Phrygian => "phrygian",
            ScaleKind::Lydian => "lydian",
            ScaleKind::Mixolydian => "mixolydian",
            ScaleKind::Locrian => "locrian",
            ScaleKind::MajorPentatonic => "major_pentatonic",
            ScaleKind::MinorPentatonic => "minor_pentatonic",
            ScaleKind::Blues => "blues",
        }
    }

    pub fn from_name(s: &str) -> Option<ScaleKind> {
        ScaleKind::ALL.into_iter().find(|k| k.name() == s)
    }
}

/// A key and scale: the root (0 for C up to 11 for B) and the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scale {
    pub root: u8,
    pub kind: ScaleKind,
}

/// The names of the twelve roots, sharps for the black keys.
pub const ROOTS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

impl Scale {
    pub fn new(root: u8, kind: ScaleKind) -> Scale {
        Scale {
            root: root % 12,
            kind,
        }
    }

    /// Such as "A minor".
    pub fn label(self) -> String {
        format!("{} {}", ROOTS[self.root as usize % 12], self.kind.label())
    }

    /// As a project stores it: such as `9:minor`.
    pub fn to_text(self) -> String {
        format!("{}:{}", self.root % 12, self.kind.name())
    }

    pub fn from_text(s: &str) -> Option<Scale> {
        let (root, kind) = s.split_once(':')?;
        let root: u8 = root.parse().ok()?;
        Some(Scale::new(root, ScaleKind::from_name(kind)?))
    }

    /// Whether `pitch` is a note of the scale.
    pub fn contains(self, pitch: u8) -> bool {
        let step = (pitch as i32 - self.root as i32).rem_euclid(12) as u8;
        self.kind.steps().contains(&step)
    }

    /// The scale's note nearest `pitch`; between two equally near, the
    /// lower. Stays on the keyboard (0 to 127).
    pub fn nearest(self, pitch: u8) -> u8 {
        if self.contains(pitch) {
            return pitch;
        }
        for d in 1..12i32 {
            for p in [pitch as i32 - d, pitch as i32 + d] {
                if (0..=127).contains(&p) && self.contains(p as u8) {
                    return p as u8;
                }
            }
        }
        pitch
    }

    /// The scale's note `by` scale steps from `pitch` (which is moved
    /// onto the scale first): up for positive, down for negative. Stays
    /// on the keyboard.
    pub fn step(self, pitch: u8, by: i32) -> u8 {
        let mut p = self.nearest(pitch) as i32;
        let dir = by.signum();
        for _ in 0..by.unsigned_abs() {
            let mut q = p + dir;
            while (0..=127).contains(&q) && !self.contains(q as u8) {
                q += dir;
            }
            if !(0..=127).contains(&q) {
                break;
            }
            p = q;
        }
        p as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_hold_their_notes_and_round_trip_as_text() {
        let a_minor = Scale::new(9, ScaleKind::Minor);
        assert_eq!(a_minor.label(), "A minor");
        // A minor has the white keys.
        for p in 60..72u8 {
            let white = !matches!(p % 12, 1 | 3 | 6 | 8 | 10);
            assert_eq!(a_minor.contains(p), white, "{p}");
        }
        assert_eq!(Scale::from_text(&a_minor.to_text()), Some(a_minor));
        for k in ScaleKind::ALL {
            assert_eq!(ScaleKind::from_name(k.name()), Some(k));
        }
        assert_eq!(Scale::from_text("13:nothing"), None);
    }

    #[test]
    fn notes_move_to_the_nearest_in_the_scale() {
        let c = Scale::new(0, ScaleKind::Major);
        assert_eq!(c.nearest(61), 60, "C# goes down to C: equally near, lower wins");
        assert_eq!(c.nearest(66), 65, "F# to F");
        assert_eq!(c.nearest(64), 64);
        let pent = Scale::new(9, ScaleKind::MinorPentatonic); // A C D E G
        assert_eq!(pent.nearest(71), 72, "B to C, the nearer");
        assert_eq!(pent.nearest(65), 64, "F to E");
        assert_eq!(pent.nearest(127), 127, "G9 is in it");
    }

    #[test]
    fn stepping_goes_by_notes_of_the_scale() {
        let c = Scale::new(0, ScaleKind::Major);
        assert_eq!(c.step(60, 1), 62);
        assert_eq!(c.step(64, 1), 65);
        assert_eq!(c.step(60, -1), 59);
        assert_eq!(c.step(60, 7), 72, "seven steps is an octave");
        assert_eq!(c.step(127, 3), 127, "stays on the keyboard");
    }
}
