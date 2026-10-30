// SPDX-License-Identifier: GPL-3.0-or-later
//! Drum maps: which kit piece sits on which staff line or space, with which
//! notehead and stem direction.
//!
//! The default map takes its keys from the General MIDI percussion key map,
//! so drum parts from MIDI files show correctly with no setup. Staff places
//! follow widely published drumset-notation conventions; kits vary, so the
//! map is editable and printed as a legend.

/// Notehead shapes used on drum staves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notehead {
    Normal,
    X,
    CircleX,
    Diamond,
    Slashed,
}

impl Notehead {
    /// The MusicXML notehead value.
    pub fn musicxml(self) -> &'static str {
        match self {
            Notehead::Normal => "normal",
            Notehead::X => "x",
            Notehead::CircleX => "circle-x",
            Notehead::Diamond => "diamond",
            Notehead::Slashed => "slashed",
        }
    }

    pub fn from_musicxml(s: &str) -> Self {
        match s {
            "x" | "cross" => Notehead::X,
            "circle-x" | "circled" => Notehead::CircleX,
            "diamond" => Notehead::Diamond,
            "slashed" | "back slashed" => Notehead::Slashed,
            _ => Notehead::Normal,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrumEntry {
    /// The note number that plays this kit piece.
    pub key: u8,
    pub name: String,
    /// Staff place in steps above the bottom line (0 = bottom line, 1 = the
    /// space above it, -1 = the space below it).
    pub place: i8,
    pub notehead: Notehead,
    /// Hands are written stems up (voice 1), feet stems down (voice 2).
    pub stem_up: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrumMap {
    pub name: String,
    pub entries: Vec<DrumEntry>,
}

/// Where a key not in the map is drawn: the middle line, normal notehead.
const FALLBACK_PLACE: i8 = 4;

impl DrumMap {
    /// The drum kit keys of the General MIDI percussion key map.
    pub fn general_midi() -> Self {
        use Notehead::*;
        let e = |key: u8, name: &str, place: i8, notehead: Notehead, stem_up: bool| DrumEntry {
            key,
            name: name.to_owned(),
            place,
            notehead,
            stem_up,
        };
        DrumMap {
            name: "Standard kit".to_owned(),
            entries: vec![
                e(35, "Acoustic bass drum", 1, Normal, false),
                e(36, "Bass drum", 1, Normal, false),
                e(37, "Side stick", 5, CircleX, true),
                e(38, "Snare", 5, Normal, true),
                e(40, "Electric snare", 5, Normal, true),
                e(41, "Low floor tom", 2, Normal, true),
                e(42, "Closed hi-hat", 9, X, true),
                e(43, "High floor tom", 3, Normal, true),
                e(44, "Pedal hi-hat", -1, X, false),
                e(45, "Low tom", 4, Normal, true),
                e(46, "Open hi-hat", 9, X, true),
                e(47, "Low-mid tom", 6, Normal, true),
                e(48, "High-mid tom", 7, Normal, true),
                e(49, "Crash cymbal", 10, X, true),
                e(50, "High tom", 8, Normal, true),
                e(51, "Ride cymbal", 8, X, true),
                e(52, "China cymbal", 11, X, true),
                e(53, "Ride bell", 8, Diamond, true),
                e(55, "Splash cymbal", 11, CircleX, true),
                e(57, "Crash cymbal 2", 10, CircleX, true),
                e(59, "Ride cymbal 2", 8, CircleX, true),
            ],
        }
    }

    pub fn entry(&self, key: u8) -> Option<&DrumEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// Place, notehead and stem direction for a key, with a fallback for
    /// keys the map doesn't know.
    pub fn look_up(&self, key: u8) -> (i8, Notehead, bool) {
        match self.entry(key) {
            Some(e) => (e.place, e.notehead, e.stem_up),
            None => (FALLBACK_PLACE, Notehead::Normal, true),
        }
    }

    /// The key drawn at a staff place with a notehead, for files that give
    /// only where a note is drawn. Prefers an exact notehead match.
    pub fn key_at(&self, place: i8, notehead: Notehead) -> Option<u8> {
        self.entries
            .iter()
            .find(|e| e.place == place && e.notehead == notehead)
            .or_else(|| self.entries.iter().find(|e| e.place == place))
            .map(|e| e.key)
    }

    /// Entries for the keys a part uses, in staff order from the top, for
    /// the printed legend.
    pub fn legend(&self, used: impl IntoIterator<Item = u8>) -> Vec<DrumEntry> {
        let mut keys: Vec<u8> = used.into_iter().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut out: Vec<DrumEntry> = keys
            .into_iter()
            .map(|k| {
                self.entry(k).cloned().unwrap_or(DrumEntry {
                    key: k,
                    name: format!("Note {k}"),
                    place: FALLBACK_PLACE,
                    notehead: Notehead::Normal,
                    stem_up: true,
                })
            })
            .collect();
        out.sort_by(|a, b| b.place.cmp(&a.place).then(a.key.cmp(&b.key)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_kit_pieces() {
        let map = DrumMap::general_midi();
        assert_eq!(map.look_up(36), (1, Notehead::Normal, false));
        assert_eq!(map.look_up(38), (5, Notehead::Normal, true));
        assert_eq!(map.look_up(42), (9, Notehead::X, true));
        assert!(!map.look_up(44).2);
        assert_eq!(map.look_up(100), (4, Notehead::Normal, true));
        assert_eq!(map.key_at(9, Notehead::X), Some(42));
        assert_eq!(map.key_at(8, Notehead::Diamond), Some(53));
        let legend = map.legend([36, 38, 42, 38]);
        let names: Vec<&str> = legend.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Closed hi-hat", "Snare", "Bass drum"]);
    }
}
