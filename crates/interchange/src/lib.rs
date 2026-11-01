// SPDX-License-Identifier: GPL-3.0-or-later
//! Swapping projects with other DAWs (roadmap Phase 4): our own reader
//! and writer for DAWproject 1.0, the open exchange format (a ZIP of
//! `project.xml` and `metadata.xml`, with the audio and plugin settings
//! beside them). Written from the format's published schema and
//! reference, which are MIT-licensed; none of their files are copied in.
//!
//! What maps, both ways: tracks, folders and buses, with their volume,
//! pan, mute, solo, routing and sends; audio clips with their fades;
//! note clips with their notes, per-note expression (pitch, pressure,
//! timbre) and controller lines; markers; the tempo and time signature
//! and their changes; automation of volume, pan, sends, built-in effect
//! settings and plugin parameters; VST 3 and CLAP plugins with their
//! settings; and the four generic built-in devices (EQ, compressor,
//! gate, limiter) to ours. Our other built-in effects and instruments
//! go out as devices of our own, with their settings in our own format,
//! so they come back whole here and are listed as left out elsewhere.
//!
//! Going in, clips inside clips are flattened onto the track, looped
//! clips are laid out loop by loop, and held automation steps become
//! steps of ours.
//!
//! Nothing is dropped silently: [`export`] and [`import`] each return a
//! [`Report`] of everything that doesn't survive the trip, and
//! [`export_report`] gives the export's before anything is written.
//!
//! Times are converted as [`time`] describes.

mod devices;
mod export;
mod import;
pub mod time;
mod xml;

use std::path::PathBuf;

use daw_project::Effect;

pub use export::{export, export_report};
pub use import::import;

/// What a project file is called: `Song.dawproject`.
pub const EXTENSION: &str = "dawproject";

/// The format version we read and write.
pub const VERSION: &str = "1.0";

/// What doesn't survive an export or an import, for the user to read
/// before (or after) it happens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub items: Vec<Item>,
}

/// One kind of thing that doesn't survive, and how many.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What it is about: "Comments", "Plugins", or a track's name.
    pub what: String,
    /// What happens to it, in a sentence.
    pub detail: String,
    /// How many times it came up.
    pub count: usize,
}

impl Report {
    /// Notes one more of `what`: the same thing said twice is counted.
    pub fn add(&mut self, what: impl Into<String>, detail: impl Into<String>) {
        let (what, detail) = (what.into(), detail.into());
        match (self.items.iter_mut()).find(|i| i.what == what && i.detail == detail) {
            Some(i) => i.count += 1,
            None => self.items.push(Item {
                what,
                detail,
                count: 1,
            }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether anything said starts with `what`.
    pub fn mentions(&self, what: &str) -> bool {
        self.items.iter().any(|i| i.what.starts_with(what))
    }

    /// As lines of text, one per item.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for i in &self.items {
            s.push_str(&i.what);
            s.push_str(": ");
            s.push_str(&i.detail);
            if i.count > 1 {
                s.push_str(&format!(" (×{})", i.count));
            }
            s.push('\n');
        }
        s
    }
}

/// What an export needs from the app.
pub struct ExportOptions<'a> {
    /// What the song is called, for the file's details.
    pub title: &'a str,
    /// Which version of the app wrote it.
    pub app_version: &'a str,
    /// Where an asset's file is: in the project folder, or the full path
    /// it is named by before the project is saved.
    pub asset_path: &'a dyn Fn(&str) -> PathBuf,
    /// A CLAP plugin's own id ("com.example.reverb"), which the project
    /// keeps only as a class id made from it.
    pub clap_id: &'a dyn Fn(&Effect) -> Option<String>,
}

/// What an import needs from the app.
pub struct ImportOptions<'a> {
    /// The sample rate the project counts frames at.
    pub rate: u32,
    /// Where to put the audio and sound files that come in the file;
    /// clips name them by full path until the project is saved.
    pub audio_dir: PathBuf,
    /// The folder the file is in, for audio it names outside itself.
    pub base_dir: Option<PathBuf>,
    /// Whether a plugin is on this computer.
    pub has_plugin: &'a dyn Fn(&Effect) -> bool,
}

/// A project read from another DAW's file.
#[derive(Clone, Debug)]
pub struct Imported {
    pub project: daw_project::Project,
    /// The song's title, if the file has one.
    pub title: Option<String>,
    /// Which DAW wrote it, and its version.
    pub application: String,
    pub report: Report,
}

#[cfg(test)]
mod tests;
