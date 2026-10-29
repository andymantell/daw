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
pub static ROOMS: &[Room] = &[];

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
