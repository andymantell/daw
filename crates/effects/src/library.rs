// SPDX-License-Identifier: GPL-3.0-or-later
//! Recorded rooms that come with the app, for the convolution reverb,
//! and recorded speaker cabinets, for the speaker cabinet.
//!
//! Each was recorded by someone else and is shipped under a licence that
//! lets us (see docs/decisions for how each was checked). Its credit is
//! shown whenever it is chosen, and all of them in Help → Sound credits.
//! The files in `rooms/` and `cabinets/` were made from the originals
//! with the `convert_room` example, which records what it changed.

use crate::response::Response;

/// Licences a room may come under: ones that let anyone pass the app on,
/// sell it and change it, as the app's own licence does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Licence {
    /// Creative Commons Attribution 4.0 International.
    CcBy4,
    /// Creative Commons Attribution-ShareAlike 4.0 International, which
    /// can be used with GPLv3 software (one-way compatible).
    CcBySa4,
    /// Creative Commons Zero: no rights kept.
    Cc0,
    /// The MIT licence, with its copyright line.
    Mit(&'static str),
}

impl Licence {
    pub fn name(self) -> &'static str {
        match self {
            Licence::CcBy4 => "CC BY 4.0",
            Licence::CcBySa4 => "CC BY-SA 4.0",
            Licence::Cc0 => "CC0",
            Licence::Mit(_) => "MIT licence",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            Licence::CcBy4 => "https://creativecommons.org/licenses/by/4.0/",
            Licence::CcBySa4 => "https://creativecommons.org/licenses/by-sa/4.0/",
            Licence::Cc0 => "https://creativecommons.org/publicdomain/zero/1.0/",
            Licence::Mit(_) => "https://opensource.org/license/mit",
        }
    }
}

/// What a recording is of, and so which effect plays it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    /// A room, for the convolution reverb.
    Room,
    /// A speaker cabinet, for the speaker cabinet.
    Cabinet,
}

/// A recorded room or cabinet.
#[derive(Debug)]
pub struct Room {
    pub what: What,
    /// How projects name it, after its prefix. Never change one.
    pub id: &'static str,
    /// What the list shows.
    pub name: &'static str,
    /// The place, or the cabinet, as its recorders name it.
    pub place: &'static str,
    /// Who recorded it, as they ask to be credited.
    pub by: &'static str,
    pub licence: Licence,
    /// Where the original is.
    pub source: &'static str,
    /// What was changed from the original.
    pub changes: &'static str,
    /// The file in `rooms/` or `cabinets/`.
    pub file: &'static str,
    wav: &'static [u8],
}

impl Room {
    /// The line shown when it is chosen.
    pub fn credit(&self) -> String {
        let licence = match self.licence {
            Licence::Mit(c) => format!("MIT licence, {c}"),
            l => l.name().to_string(),
        };
        format!(
            "{}, recorded by {}. {licence}. From {}. Changed: {}.",
            self.place, self.by, self.source, self.changes
        )
    }

    pub fn response(&self) -> Result<Response, String> {
        Response::from_wav_bytes(self.wav)
    }
}

/// Every recorded room, in the order listed.
pub static ROOMS: &[Room] = &[
    Room {
        what: What::Room,
        id: "concert-hall-jack-lyons",
        name: "Concert hall",
        place: "Jack Lyons Concert Hall, University of York",
        by: "Audiolab, University of York: Alex Duffell, Aishwarya Sridhar and Zhong Li (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?p=571",
        changes: "B-format decoded to stereo, resampled from 96000 to 48 kHz, trimmed to 4.0 s, level set",
        file: "concert-hall-jack-lyons.wav",
        wav: include_bytes!("../rooms/concert-hall-jack-lyons.wav"),
    },
    Room {
        what: What::Room,
        id: "cathedral-york-minster",
        name: "Cathedral",
        place: "York Minster",
        by: "Audiolab, University of York: Damian T. Murphy (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=797",
        changes: "16-bit, trimmed to 9.9 s, level set",
        file: "cathedral-york-minster.wav",
        wav: include_bytes!("../rooms/cathedral-york-minster.wav"),
    },
    Room {
        what: What::Room,
        id: "chapel-st-albans",
        name: "Chapel",
        place: "Lady Chapel, St Albans Cathedral",
        by: "Audiolab, University of York: Marcin Gorzel, Gavin Kearney, Aglaia Foteinou, Sorrel Hoare and Simon Shelley (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=595",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 2.4 s, level set",
        file: "chapel-st-albans.wav",
        wav: include_bytes!("../rooms/chapel-st-albans.wav"),
    },
    Room {
        what: What::Room,
        id: "church-patrington",
        name: "Village church",
        place: "St Patrick's Church, Patrington",
        by: "Aglaia Foteinou and Damian T. Murphy (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=722",
        changes: "resampled from 96000 to 48 kHz, 16-bit, trimmed to 2.5 s, level set",
        file: "church-patrington.wav",
        wav: include_bytes!("../rooms/church-patrington.wav"),
    },
    Room {
        what: What::Room,
        id: "theatre-dixon-studio",
        name: "Studio theatre",
        place: "The Dixon Studio Theatre, University of York",
        by: "Ben Lavin, Darren Robinson and Ya-Hsin Chou, University of York (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=452",
        changes: "B-format decoded to stereo, resampled from 96000 to 48 kHz, trimmed to 3.0 s, level set",
        file: "theatre-dixon-studio.wav",
        wav: include_bytes!("../rooms/theatre-dixon-studio.wav"),
    },
    Room {
        what: What::Room,
        id: "stairwell-york",
        name: "Stairwell",
        place: "Stairway, University of York",
        by: "Audiolab, University of York: Simon Shelley (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=678",
        changes: "resampled from 96000 to 48 kHz, 16-bit, trimmed to 1.1 s, level set",
        file: "stairwell-york.wav",
        wav: include_bytes!("../rooms/stairwell-york.wav"),
    },
    Room {
        what: What::Room,
        id: "typing-room-terrys",
        name: "Small room",
        place: "Terry's Typing Room, York",
        by: "Audiolab, University of York: Damian T. Murphy (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=740",
        changes: "resampled from 96000 to 48 kHz, 16-bit, trimmed to 2.8 s, level set",
        file: "typing-room-terrys.wav",
        wav: include_bytes!("../rooms/typing-room-terrys.wav"),
    },
    Room {
        what: What::Room,
        id: "marble-hall-elveden",
        name: "Marble hall",
        place: "Marble Hall, Elveden Hall, Suffolk",
        by: "Matt Rogalsky (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=459",
        changes: "resampled from 44100 to 48 kHz, trimmed to 8.0 s, level set",
        file: "marble-hall-elveden.wav",
        wav: include_bytes!("../rooms/marble-hall-elveden.wav"),
    },
    Room {
        what: What::Room,
        id: "warehouse-terrys",
        name: "Warehouse",
        place: "Terry's Factory Warehouse, York",
        by: "Audiolab, University of York: Damian T. Murphy (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=735",
        changes: "16-bit, trimmed to 10.0 s, level set",
        file: "warehouse-terrys.wav",
        wav: include_bytes!("../rooms/warehouse-terrys.wav"),
    },
    Room {
        what: What::Room,
        id: "reactor-hall-r1",
        name: "Reactor hall",
        place: "R1 Nuclear Reactor Hall",
        by: "Audiolab, University of York (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=626",
        changes: "16-bit, trimmed to 5.7 s, level set",
        file: "reactor-hall-r1.wav",
        wav: include_bytes!("../rooms/reactor-hall-r1.wav"),
    },
    Room {
        what: What::Room,
        id: "tunnel-innocent-railway",
        name: "Railway tunnel",
        place: "Innocent Railway Tunnel, Edinburgh",
        by: "Andrew Chadwick and Simon Shelley (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=525",
        changes: "B-format decoded to stereo, resampled from 96000 to 48 kHz, 16-bit, trimmed to 5.0 s, level set",
        file: "tunnel-innocent-railway.wav",
        wav: include_bytes!("../rooms/tunnel-innocent-railway.wav"),
    },
    Room {
        what: What::Room,
        id: "mausoleum-hamilton",
        name: "Mausoleum",
        place: "Hamilton Mausoleum, Hamilton, Scotland",
        by: "Audiolab, University of York: Damian T. Murphy (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=502",
        changes: "16-bit, trimmed to 10.0 s, level set",
        file: "mausoleum-hamilton.wav",
        wav: include_bytes!("../rooms/mausoleum-hamilton.wav"),
    },
    Room {
        what: What::Room,
        id: "hall-spokane",
        name: "Wooden hall",
        place: "Spokane Woman's Club hall, Spokane",
        by: "James Cadwallader (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=659",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 2.7 s, level set",
        file: "hall-spokane.wav",
        wav: include_bytes!("../rooms/hall-spokane.wav"),
    },
    Room {
        what: What::Room,
        id: "tennis-court-falkland",
        name: "Walled court, open sky",
        place: "Royal Tennis Court, Falkland Palace, Fife",
        by: "Audiolab, University of York: Damian T. Murphy (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=476",
        changes: "16-bit, trimmed to 4.8 s, level set",
        file: "tennis-court-falkland.wav",
        wav: include_bytes!("../rooms/tennis-court-falkland.wav"),
    },
    Room {
        what: What::Room,
        id: "forest-koli",
        name: "Forest",
        place: "Forest in Koli National Park, Finland, in summer",
        by: "Andrew Chadwick and Simon Shelley (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=577",
        changes: "B-format decoded to stereo, resampled from 96000 to 48 kHz, 16-bit, trimmed to 1.1 s, level set",
        file: "forest-koli.wav",
        wav: include_bytes!("../rooms/forest-koli.wav"),
    },
    Room {
        what: What::Room,
        id: "gorge-trollers-gill",
        name: "Limestone gorge",
        place: "Troller's Gill, Yorkshire Dales",
        by: "Andrew Chadwick and Simon Shelley (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=745",
        changes: "B-format decoded to stereo, resampled from 96000 to 48 kHz, 16-bit, trimmed to 0.8 s, level set",
        file: "gorge-trollers-gill.wav",
        wav: include_bytes!("../rooms/gorge-trollers-gill.wav"),
    },
    Room {
        what: What::Room,
        id: "cave-creswell-crags",
        name: "Cave",
        place: "Creswell Crags caves",
        by: "Francis Stevens and Joe Rees-Jones (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=441",
        changes: "B-format decoded to stereo, resampled from 96000 to 48 kHz, trimmed to 1.5 s, level set",
        file: "cave-creswell-crags.wav",
        wav: include_bytes!("../rooms/cave-creswell-crags.wav"),
    },
    Room {
        what: What::Room,
        id: "mine-gill-heads",
        name: "Disused mine",
        place: "Gill Heads Mine, Yorkshire Dales",
        by: "Andrew Chadwick and Simon Shelley (OpenAIR)",
        licence: Licence::CcBy4,
        source: "https://www.openair.hosted.york.ac.uk/?page_id=494",
        changes: "B-format decoded to stereo, resampled from 96000 to 48 kHz, 16-bit, trimmed to 4.9 s, level set",
        file: "mine-gill-heads.wav",
        wav: include_bytes!("../rooms/mine-gill-heads.wav"),
    },
    Room {
        what: What::Room,
        id: "spring-ak-springer",
        name: "Spring",
        place: "AKIR Springer, setting 10",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 1.6 s, level set",
        file: "spring-ak-springer.wav",
        wav: include_bytes!("../rooms/spring-ak-springer.wav"),
    },
    Room {
        what: What::Room,
        id: "spring-ak-smooth",
        name: "Spring, smooth",
        place: "AKIR Smooth Springer, setting 10",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 2.4 s, level set",
        file: "spring-ak-smooth.wav",
        wav: include_bytes!("../rooms/spring-ak-smooth.wav"),
    },
    Room {
        what: What::Room,
        id: "spring-ak-dual",
        name: "Spring, bright stereo",
        place: "AKIR Dual Springer, setting 8",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 2.5 s, level set",
        file: "spring-ak-dual.wav",
        wav: include_bytes!("../rooms/spring-ak-dual.wav"),
    },
];

/// Every recorded cabinet, in the order listed. All from one set, whose
/// folder names say the style of cabinet but not the make.
pub static CABINETS: &[Room] = &[
    Room {
        what: What::Cabinet,
        id: "ak-vintage-us",
        name: "Vintage American",
        place: "AK-SPKRS speakers and amps, VintageUs set, file 002",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 24 ms, level set",
        file: "ak-vintage-us.wav",
        wav: include_bytes!("../cabinets/ak-vintage-us.wav"),
    },
    Room {
        what: What::Cabinet,
        id: "ak-vintage-uk",
        name: "Vintage British",
        place: "AK-SPKRS speakers and amps, VintageUk set, file 002",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 31 ms, level set",
        file: "ak-vintage-uk.wav",
        wav: include_bytes!("../cabinets/ak-vintage-uk.wav"),
    },
    Room {
        what: What::Cabinet,
        id: "ak-modern-us",
        name: "Modern American",
        place: "AK-SPKRS speakers and amps, ModernUs set, file 003",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 71 ms, level set",
        file: "ak-modern-us.wav",
        wav: include_bytes!("../cabinets/ak-modern-us.wav"),
    },
    Room {
        what: What::Cabinet,
        id: "ak-modern-uk",
        name: "Modern British",
        place: "AK-SPKRS speakers and amps, ModernUk set, file 002",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 72 ms, level set",
        file: "ak-modern-uk.wav",
        wav: include_bytes!("../cabinets/ak-modern-uk.wav"),
    },
    Room {
        what: What::Cabinet,
        id: "ak-eighties-us",
        name: "1980s American",
        place: "AK-SPKRS speakers and amps, 80sUs set, file 002",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 30 ms, level set",
        file: "ak-eighties-us.wav",
        wav: include_bytes!("../cabinets/ak-eighties-us.wav"),
    },
    Room {
        what: What::Cabinet,
        id: "ak-eighties-uk",
        name: "1980s British",
        place: "AK-SPKRS speakers and amps, 80sUk set, file 002",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 20 ms, level set",
        file: "ak-eighties-uk.wav",
        wav: include_bytes!("../cabinets/ak-eighties-uk.wav"),
    },
    Room {
        what: What::Cabinet,
        id: "ak-radio",
        name: "Small radio speaker",
        place: "AK-SPKRS speakers and amps, Radio set, file 001",
        by: "Adventure Kid",
        licence: Licence::CcBy4,
        source: "https://www.adventurekid.se/akrt/free-reverb-impulse-responses/",
        changes: "resampled from 44100 to 48 kHz, 16-bit, trimmed to 118 ms, level set",
        file: "ak-radio.wav",
        wav: include_bytes!("../cabinets/ak-radio.wav"),
    },
];

/// What a project names a recorded room by, in an effect's response.
pub const PREFIX: &str = "room:";

/// What a project names a recorded cabinet by, in an effect's response.
pub const CABINET_PREFIX: &str = "cabinet:";

/// The room or cabinet a response reference names, if it names one.
pub fn find(reference: &str) -> Option<&'static Room> {
    if let Some(id) = reference.strip_prefix(PREFIX) {
        return ROOMS.iter().find(|r| r.id == id);
    }
    let id = reference.strip_prefix(CABINET_PREFIX)?;
    CABINETS.iter().find(|r| r.id == id)
}

/// Whether `reference` names a recording that comes with the app (one
/// this version may not have), rather than a file.
pub fn is_recorded(reference: &str) -> bool {
    reference.starts_with(PREFIX) || reference.starts_with(CABINET_PREFIX)
}

/// The reference for `room`, as saved in projects.
pub fn reference(room: &Room) -> String {
    let prefix = match room.what {
        What::Room => PREFIX,
        What::Cabinet => CABINET_PREFIX,
    };
    format!("{prefix}{}", room.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file shipped is listed, credited and under a licence we may
    /// ship, and every listed room and cabinet reads.
    #[test]
    fn every_room_is_listed_credited_and_reads() {
        check("rooms", ROOMS, What::Room);
        check("cabinets", CABINETS, What::Cabinet);
        assert!(find("room:nowhere").is_none());
        assert!(find("cabinet:nowhere").is_none());
        assert!(find("guitar.wav").is_none());
        // A room isn't found as a cabinet, or the reverse.
        assert!(find(&format!("{CABINET_PREFIX}{}", ROOMS[0].id)).is_none());
        assert!(find(&format!("{PREFIX}{}", CABINETS[0].id)).is_none());
        assert!(is_recorded("cabinet:ak-radio") && is_recorded("room:x"));
        assert!(!is_recorded("guitar.wav"));
    }

    /// A recorded cabinet is short: a speaker in a box, not a room.
    #[test]
    fn recorded_cabinets_are_short() {
        for c in CABINETS {
            let r = c.response().unwrap();
            assert!(r.seconds() < 0.2, "{}: {}", c.id, r.seconds());
        }
    }

    fn check(folder: &str, list: &[Room], what: What) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(folder);
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".wav"))
            .collect();
        files.sort();
        let mut listed: Vec<String> = list.iter().map(|r| r.file.to_string()).collect();
        listed.sort();
        assert_eq!(
            files, listed,
            "every file in {folder}/ is listed, and no more"
        );
        let mut ids: Vec<&str> = list.iter().map(|r| r.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), list.len(), "ids are unique");
        for r in list {
            assert_eq!(r.what, what, "{}", r.id);
            for (what, v) in [
                ("name", r.name),
                ("place", r.place),
                ("by", r.by),
                ("source", r.source),
                ("changes", r.changes),
            ] {
                assert!(!v.trim().is_empty(), "{}: no {what}", r.id);
            }
            assert!(r.source.starts_with("https://"), "{}", r.id);
            if let Licence::Mit(c) = r.licence {
                assert!(
                    c.contains("Copyright"),
                    "{}: MIT needs its copyright line",
                    r.id
                );
            }
            let resp = r.response().unwrap();
            // A room rings on; a cabinet lasts a few milliseconds.
            let least = if what == What::Room { 0.05 } else { 0.005 };
            assert!(resp.seconds() > least, "{}", r.id);
            assert_eq!(find(&reference(r)).map(|f| f.id), Some(r.id));
        }
    }
}
