// SPDX-License-Identifier: GPL-3.0-or-later
//! Recorded rooms that come with the app, for the convolution reverb.
//!
//! Each was recorded by someone else and is shipped under a licence that
//! lets us (see docs/decisions for how each was checked). Its credit is
//! shown whenever it is chosen, and all of them in Help → Sound credits.
//! The files in `rooms/` were made from the originals with the
//! `convert_room` example, which records what it changed.

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

/// A recorded room.
#[derive(Debug)]
pub struct Room {
    /// How projects name it. Never change one.
    pub id: &'static str,
    /// What the list shows.
    pub name: &'static str,
    /// The place, as its recorders name it.
    pub place: &'static str,
    /// Who recorded it, as they ask to be credited.
    pub by: &'static str,
    pub licence: Licence,
    /// Where the original is.
    pub source: &'static str,
    /// What was changed from the original.
    pub changes: &'static str,
    /// The file in `rooms/`.
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

/// What a project names a recorded room by, in an effect's response.
pub const PREFIX: &str = "room:";

/// The room a response reference names, if it names one.
pub fn find(reference: &str) -> Option<&'static Room> {
    let id = reference.strip_prefix(PREFIX)?;
    ROOMS.iter().find(|r| r.id == id)
}

/// The reference for `room`, as saved in projects.
pub fn reference(room: &Room) -> String {
    format!("{PREFIX}{}", room.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file shipped is listed, credited and under a licence we may
    /// ship, and every listed room reads.
    #[test]
    fn every_room_is_listed_credited_and_reads() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("rooms");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".wav"))
            .collect();
        files.sort();
        let mut listed: Vec<String> = ROOMS.iter().map(|r| r.file.to_string()).collect();
        listed.sort();
        assert_eq!(files, listed, "every file in rooms/ is listed, and no more");
        let mut ids: Vec<&str> = ROOMS.iter().map(|r| r.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ROOMS.len(), "ids are unique");
        for r in ROOMS {
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
            assert!(resp.seconds() > 0.05, "{}", r.id);
            assert_eq!(find(&reference(r)).map(|f| f.id), Some(r.id));
        }
        assert!(find("room:nowhere").is_none());
        assert!(find("guitar.wav").is_none());
    }
}
