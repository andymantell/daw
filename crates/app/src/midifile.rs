// SPDX-License-Identifier: GPL-3.0-or-later
//! Standard MIDI files in and out of the project (roadmap 3.1): each
//! file track becomes an instrument track with one note clip, and each
//! instrument track goes out as a file track.

use std::path::{Path, PathBuf};

use daw_project::midifile::{self, MidiTrack};
use daw_project::{NoteClip, PPQ};
use eframe::egui;

use crate::{App, Track, user_dir};

/// Whether `path` looks like a MIDI file.
pub(crate) fn is_midi(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
}

/// The tracks `file` becomes: one instrument track with one clip
/// holding the file track's notes, named after it (or the file).
pub(crate) fn tracks_for(file: &midifile::MidiFile, file_name: &str) -> Vec<Track> {
    let mut out = Vec::new();
    for (i, mt) in file.tracks.iter().enumerate() {
        let name = if mt.name.is_empty() {
            if file.tracks.len() == 1 {
                file_name.to_string()
            } else {
                format!("{file_name} {}", i + 1)
            }
        } else {
            mt.name.clone()
        };
        let mut t = Track::instrument(name.clone());
        let end = mt.notes.iter().map(|n| n.end()).max().unwrap_or(PPQ);
        // The clip runs from the start of the bar to the end of the
        // last note, rounded up to a whole beat.
        let len = ((end + PPQ - 1) / PPQ) * PPQ;
        let mut clip = NoteClip::new(daw_project::new_id(), name.clone(), 0, len);
        clip.notes = mt.notes.clone();
        clip.sort_notes();
        // Drum parts (on the drum channel) and parts named for a guitar
        // or bass are shown as a drum staff or tab, with strings and
        // frets chosen for them.
        let lower = name.to_lowercase();
        let drums = !mt.notes.is_empty() && mt.notes.iter().all(|n| n.channel == 9);
        if drums || ["guitar", "gtr", "bass"].iter().any(|w| lower.contains(w)) {
            t.notation = Some(crate::notation::guess(&name, &mt.notes));
        }
        t.note_clips.push(clip);
        out.push(t);
    }
    out
}

impl App {
    /// Asks for a MIDI file and adds its tracks.
    pub(crate) fn pick_midi_file(&mut self, _ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .set_title("Import MIDI file")
            .set_directory(user_dir())
            .add_filter("MIDI file", &["mid", "midi"])
            .pick_file();
        if let Some(path) = picked {
            self.import_midi(&path);
        }
    }

    /// Adds the tracks in a MIDI file. With no tracks yet, the file's
    /// tempo and time signature become the project's.
    pub(crate) fn import_midi(&mut self, path: &Path) {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                self.message = Some(format!("Could not read {}: {e}", path.display()));
                return;
            }
        };
        let file = match midifile::read(&data) {
            Ok(f) => f,
            Err(e) => {
                self.message = Some(format!("Could not import {}: {e}", path.display()));
                return;
            }
        };
        if file.tracks.is_empty() {
            self.message = Some(format!("{} has no notes in it.", path.display()));
            return;
        }
        if self.tracks.is_empty() {
            if let Some(bpm) = file.tempo_bpm {
                self.tempo_bpm = (bpm * 1000.0).round() / 1000.0;
            }
            if let Some((beats, _)) = file.signature
                && beats > 0
            {
                self.beats_per_bar = beats as u32;
            }
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "MIDI".into());
        let added = tracks_for(&file, &stem);
        let n = added.len();
        for t in added {
            if let Some(i) = &t.instrument {
                i.make_into(self.rate, &mut self.pending_instruments);
            }
            self.tracks.push(t);
        }
        self.message = Some(format!(
            "Imported {n} track{} from {}.",
            if n == 1 { "" } else { "s" },
            path.display()
        ));
        self.rebuild();
    }

    /// The instrument tracks as MIDI file tracks, with their clips
    /// placed on the timeline.
    pub(crate) fn midi_tracks(&self) -> Vec<MidiTrack> {
        (self.tracks.iter())
            .filter(|t| t.kind.plays_notes())
            .map(|t| {
                let p = daw_project::Track {
                    note_clips: t.note_clips.clone(),
                    ..daw_project::Track::new(String::new(), String::new())
                };
                MidiTrack {
                    name: t.name.clone(),
                    notes: p.placed_notes(),
                }
            })
            .filter(|t| !t.notes.is_empty())
            .collect()
    }

    /// Asks where, then writes the instrument tracks as a MIDI file.
    pub(crate) fn export_midi_file(&mut self) {
        let tracks = self.midi_tracks();
        if tracks.is_empty() {
            self.message = Some("There are no notes to export yet.".into());
            return;
        }
        let name = self
            .proj
            .folder
            .as_ref()
            .map_or("notes".to_string(), |f| f.name());
        let Some(dest) = rfd::FileDialog::new()
            .set_title("Export MIDI file")
            .set_directory(user_dir())
            .set_file_name(format!("{name}.mid"))
            .add_filter("MIDI file", &["mid"])
            .save_file()
        else {
            return;
        };
        let dest = if is_midi(&dest) {
            dest
        } else {
            PathBuf::from(format!("{}.mid", dest.display()))
        };
        self.write_midi(&dest);
    }

    pub(crate) fn write_midi(&mut self, dest: &Path) {
        let tracks = self.midi_tracks();
        let bytes = midifile::write(
            self.tempo_bpm,
            (self.beats_per_bar.min(255) as u8, 4),
            &tracks,
        );
        self.message = Some(match std::fs::write(dest, bytes) {
            Ok(()) => format!("Exported {} track(s) to {}.", tracks.len(), dest.display()),
            Err(e) => format!("Could not write {}: {e}", dest.display()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_project::Note;

    #[test]
    fn file_tracks_become_instrument_tracks_with_a_clip_each() {
        let f = midifile::MidiFile {
            tempo_bpm: Some(90.0),
            signature: Some((3, 4)),
            tracks: vec![
                MidiTrack {
                    name: "Piano".into(),
                    notes: vec![
                        Note::new(0, PPQ, 60, 100),
                        Note::new(PPQ * 5, PPQ / 2, 62, 90),
                    ],
                },
                MidiTrack {
                    name: String::new(),
                    notes: vec![Note::new(PPQ, PPQ, 36, 120)],
                },
            ],
        };
        let t = tracks_for(&f, "song");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].name, "Piano");
        assert_eq!(t[1].name, "song 2");
        assert!(
            t.iter()
                .all(|t| t.kind.plays_notes() && t.instrument.is_some())
        );
        assert_eq!(t[0].note_clips.len(), 1);
        assert_eq!(t[0].note_clips[0].notes.len(), 2);
        // To the end of the last note, in whole beats.
        assert_eq!(t[0].note_clips[0].len, PPQ * 6);
        assert_ne!(t[0].note_clips[0].id, t[1].note_clips[0].id);
    }

    #[test]
    fn drum_and_guitar_parts_come_in_with_notation_lanes() {
        let mut hit = Note::new(0, PPQ, 36, 120);
        hit.channel = 9;
        let track = |name: &str, notes: Vec<Note>| MidiTrack {
            name: name.into(),
            notes,
        };
        let f = midifile::MidiFile {
            tempo_bpm: None,
            signature: None,
            tracks: vec![
                track("Kit", vec![hit]),
                track("Lead Guitar", vec![Note::new(0, PPQ, 64, 90)]),
                track("Fretless Bass", vec![Note::new(0, PPQ, 40, 90)]),
                track("Piano", vec![Note::new(0, PPQ, 60, 90)]),
            ],
        };
        let t = tracks_for(&f, "song");
        let views: Vec<_> = (t.iter())
            .map(|t| t.notation.as_ref().map(|n| (n.view, n.tuning.len())))
            .collect();
        use daw_project::NotationView::*;
        assert_eq!(
            views,
            [Some((Drums, 0)), Some((Tab, 6)), Some((Tab, 4)), None]
        );
    }

    #[test]
    fn midi_files_are_told_by_their_extension() {
        assert!(is_midi(Path::new("a/song.MID")));
        assert!(is_midi(Path::new("song.midi")));
        assert!(!is_midi(Path::new("song.wav")));
    }
}
