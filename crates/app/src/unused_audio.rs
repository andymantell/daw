// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio nothing needs any more, taken out of the saved project's
//! folder (the owner's choice, 2026-11-05): a deleted take's audio goes
//! on the save after nothing uses it, once undo can't bring it back and
//! no snapshot refers to it. The rules are in `daw_project`'s
//! `cleanup.rs`.
//!
//! Every save removes what can go. Undo lasts until the project is
//! closed, so leaving the project (opening another, a new one, or
//! quitting) removes what only undo was keeping. "Remove unused audio"
//! does it on asking, and says what went and what waits for undo.

use daw_project::AudioFile;
use serde_json::{Value, json};

use crate::App;
use crate::share::device_id;

impl App {
    /// Removes the saved project's audio that nothing needs any more,
    /// and returns what went. `leaving`: the project is being closed, so
    /// what undo could bring back goes too.
    pub(crate) fn remove_unused_audio(&mut self, leaving: bool) -> Vec<AudioFile> {
        let Some(folder) = self.proj.folder.clone() else {
            return Vec::new();
        };
        // Suggested edits have an undo of their own.
        if self.proj.suggesting.is_some() && !leaving {
            return Vec::new();
        }
        let gone = folder.remove_unused_audio(&self.proj.doc, device_id(), !leaving);
        if !gone.is_empty() {
            for f in &gone {
                self.proj.sources.remove(&f.name);
            }
            // The save before may still use it: the backup becomes this
            // save, as for a deleted comment.
            if let Err(e) = folder.replace_backup(&self.proj.doc.to_bytes()) {
                self.message = Some(e);
            }
            self.proj.removed_audio.extend(gone.iter().cloned());
        }
        gone
    }

    /// The command: saves (which removes what can go), and says what went
    /// and what waits.
    pub(crate) fn remove_unused_audio_now(&mut self, ctx: &egui::Context) {
        if self.proj.folder.is_none() {
            self.message = Some(
                "Save the project first: until then its audio isn't in a project folder.".into(),
            );
            return;
        }
        let before = self.proj.removed_audio.len();
        if self.proj.dirty {
            if let Err(e) = self.save(ctx) {
                self.message = Some(e);
                return;
            }
        } else {
            self.remove_unused_audio(false);
        }
        let gone = &self.proj.removed_audio[before..];
        let waiting = self.audio_kept_for_undo();
        let mut msg = if gone.is_empty() {
            "No unused audio to remove.".to_string()
        } else {
            format!(
                "Removed {} no longer used ({}).",
                files(gone.len()),
                size(gone.iter().map(|f| f.bytes).sum())
            )
        };
        if !waiting.is_empty() {
            msg += &format!(
                " {} ({}) {} until the project is closed, as Undo could still bring {} back.",
                files(waiting.len()),
                size(waiting.iter().map(|f| f.bytes).sum()),
                if waiting.len() == 1 { "stays" } else { "stay" },
                if waiting.len() == 1 { "it" } else { "them" },
            );
        }
        self.message = Some(msg);
    }

    /// Audio nothing uses except what undo could bring back: it goes when
    /// the project is closed.
    fn audio_kept_for_undo(&self) -> Vec<AudioFile> {
        let Some(folder) = &self.proj.folder else {
            return Vec::new();
        };
        let now = folder.unused_audio(&self.proj.doc, device_id(), true);
        (folder
            .unused_audio(&self.proj.doc, device_id(), false)
            .into_iter())
        .filter(|f| !now.contains(f))
        .collect()
    }

    /// For assistants: the audio that can go now, what waits for undo,
    /// and what was removed since the project was opened.
    pub(crate) fn unused_audio_value(&self) -> Value {
        let list = |v: &[AudioFile]| -> Vec<Value> {
            v.iter()
                .map(|f| json!({"file": f.name, "bytes": f.bytes}))
                .collect()
        };
        let Some(folder) = &self.proj.folder else {
            return json!({
                "saved": false,
                "unused": [], "kept_for_undo": [], "removed": [],
                "note": "The project isn't saved yet, so its audio isn't in a project folder."
            });
        };
        let unused = folder.unused_audio(&self.proj.doc, device_id(), true);
        json!({
            "saved": true,
            "audio_files": folder.audio_files().len(),
            "unused": list(&unused),
            "kept_for_undo": list(&self.audio_kept_for_undo()),
            "removed": list(&self.proj.removed_audio),
        })
    }
}

fn files(n: usize) -> String {
    if n == 1 {
        "1 audio file".into()
    } else {
        format!("{n} audio files")
    }
}

fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 0.1 {
        format!("{mb:.1} MB")
    } else {
        format!("{} KB", bytes.div_ceil(1000))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use crate::agent::driver::Driver;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-wipe-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Everything a saved document holds, history included, as text.
    fn everything_in(path: &Path) -> String {
        let doc = loro::LoroDoc::new();
        doc.import(&std::fs::read(path).unwrap()).unwrap();
        let json = doc
            .export_json_updates(&loro::VersionVector::new(), &doc.oplog_vv())
            .unwrap();
        format!("{json:?} {:?}", doc.get_deep_value())
    }

    /// Half a second of a tone at the app's rate; `pitch` makes each
    /// file different.
    fn wav(dir: &Path, name: &str, rate: u32, pitch: u32) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate / 2 {
            w.write_sample(((i % pitch) as i16 - 20) * 50).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    fn clips(d: &mut Driver) -> Vec<String> {
        let p = d.project();
        (p["tracks"].as_array().unwrap().iter())
            .flat_map(|t| t["clips"].as_array().unwrap().clone())
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn deleted_lesson_words_leave_the_saved_file_and_its_backup() {
        let dir = scratch("words");
        let mut d = Driver::new();
        let secret = "Sam's phone is 07700 900123";
        d.call(
            "edit_lesson_notes",
            json!({
                "text": format!("# Lesson 3\nBends in bar 7. {secret}."),
                "add_tabs": [{"title": "Riff", "text": format!("e|--3--| {secret}")}]
            }),
        )
        .unwrap();
        let song = dir.join("Song");
        d.call("save_project", json!({"path": song})).unwrap();
        let tab = d.call("get_lessons", json!({})).unwrap()["tabs"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        d.call(
            "edit_lesson_notes",
            json!({"text": "# Lesson 3\nBends in bar 7.", "remove_tabs": [tab]}),
        )
        .unwrap();
        d.call("save_project", json!({})).unwrap();
        for file in [daw_project::DOC_FILE, "project.daw.bak"] {
            let all = everything_in(&song.join(file));
            assert!(!all.contains("07700"), "{file} still has the words");
            assert!(!all.contains("phone"), "{file} still has the words");
        }
        let notes = d.call("get_lessons", json!({})).unwrap()["notes"].clone();
        assert_eq!(notes, "# Lesson 3\nBends in bar 7.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_take_leaves_the_folder_once_undo_cant_bring_it_back() {
        let dir = scratch("takes");
        let mut d = Driver::new();
        let e = d.call("get_unused_audio", json!({})).unwrap();
        assert_eq!(e["saved"], false);
        let rate = d.app().rate;
        for (name, pitch) in [("one.wav", 40), ("two.wav", 60)] {
            d.call(
                "import_audio",
                json!({"path": wav(&dir, name, rate, pitch)}),
            )
            .unwrap();
        }
        for _ in 0..500 {
            if clips(&mut d).len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ids = clips(&mut d);
        assert_eq!(ids.len(), 2, "both files came in");
        let song = dir.join("Song");
        d.call("save_project", json!({"path": song})).unwrap();
        let audio = || {
            let mut v: Vec<String> = std::fs::read_dir(song.join(daw_project::ASSETS))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(audio().len(), 2);
        let r = d.call("get_unused_audio", json!({})).unwrap();
        assert_eq!(r["unused"], json!([]));

        // Deleted and saved: undo could still bring it back.
        d.call("delete_clip", json!({"clip": ids[0]})).unwrap();
        d.call("save_project", json!({})).unwrap();
        assert_eq!(audio().len(), 2);
        let r = d.call("get_unused_audio", json!({})).unwrap();
        assert_eq!(r["unused"], json!([]));
        assert_eq!(r["kept_for_undo"].as_array().unwrap().len(), 1);
        d.command("remove-unused-audio");
        assert_eq!(audio().len(), 2, "kept for undo");
        let msg = d.app().message.clone().unwrap_or_default();
        assert!(msg.contains("Undo could still bring it back"), "{msg}");
        let gone = r["kept_for_undo"][0]["file"].as_str().unwrap().to_string();

        // Undo and redo still work, then the project is closed.
        d.command("undo");
        assert_eq!(clips(&mut d).len(), 2);
        d.command("redo");
        assert_eq!(clips(&mut d).len(), 1);
        d.call("new_project", json!({})).unwrap();
        assert!(!audio().contains(&gone), "closing the project removed it");
        assert_eq!(audio().len(), 1, "the take still used stays");

        // It opens again as it was, the one take playing from its file.
        d.call("open_project", json!({"project": song})).unwrap();
        for _ in 0..500 {
            if clips(&mut d).len() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(clips(&mut d).len(), 1);
        let r = d.call("get_unused_audio", json!({})).unwrap();
        assert_eq!(r["saved"], true);
        assert_eq!(r["audio_files"], 1);
        assert_eq!(r["unused"], json!([]));
        assert_eq!(r["kept_for_undo"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
