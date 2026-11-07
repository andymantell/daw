// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio that nothing needs any more, taken out of the project folder
//! (the owner's choice, 2026-11-05): a deleted take's audio goes once
//! nothing uses it. That is once it has been saved without it, undo can
//! no longer bring it back, and no snapshot or suggestion still refers to
//! it. Bundles already carry only the audio something uses
//! ([`ProjectDoc::assets_in_use`]), so this is what takes it out of the
//! folder, and so out of its backups and the copies a synced folder
//! makes.
//!
//! Some care is needed, as a project folder can be shared:
//!
//! - Only files this copy's history has named are removed. Audio from
//!   another computer can arrive in a synced folder before that
//!   computer's document does; it is left alone until its document
//!   says what it is.
//! - Audio that any other computer's copy of the document (in
//!   `Devices`) still uses is kept.
//! - Undo here could bring back anything the project used since it was
//!   opened, so while there is anything to undo or redo, all of that is
//!   kept. Once the project is closed and opened again, undo starts
//!   afresh.
//!
//! The same check runs on every save, and on asking for it (the
//! "Remove unused audio" command), which also says what it removed.

use std::collections::HashSet;
use std::fs;

use loro::{JsonListOp, JsonMapOp, JsonMovableListOp, JsonOpContent, LoroValue, VersionVector};

use crate::folder::{ASSETS, is_asset_name};
use crate::{Project, ProjectDoc, ProjectFolder, Track};

/// An audio file in a project folder.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioFile {
    /// Its asset name.
    pub name: String,
    pub bytes: u64,
}

/// Every sound file `p` uses: its tracks' (and their stored versions')
/// and its videos'.
pub fn project_assets(p: &Project) -> Vec<String> {
    let mut v: Vec<String> = (p.tracks.iter())
        .flat_map(|t| std::iter::once(t).chain(t.stored_versions()))
        .flat_map(Track::assets)
        .chain(p.videos.iter().map(|c| c.asset.clone()))
        .collect();
    v.sort();
    v.dedup();
    v
}

impl ProjectDoc {
    /// The assets the project uses as it is now.
    pub(crate) fn assets_now(&self) -> Vec<String> {
        project_assets(&self.get())
    }

    /// The hash part of every asset name the history mentions: ever, and
    /// in changes made or merged since this copy was opened.
    fn named_assets(&self) -> (HashSet<String>, HashSet<String>) {
        let (mut ever, mut since) = (HashSet::new(), HashSet::new());
        let Ok(json) = self.doc.export_json_updates_without_peer_compression(
            &VersionVector::new(),
            &self.doc.oplog_vv(),
        ) else {
            return (ever, since);
        };
        for ch in &json.changes {
            let new = !self.opened_at.includes_id(ch.id);
            let out = if new { &mut since } else { &mut ever };
            for op in &ch.ops {
                match &op.content {
                    JsonOpContent::Map(JsonMapOp::Insert { value, .. })
                    | JsonOpContent::MovableList(JsonMovableListOp::Set { value, .. }) => {
                        hashes_in(value, out)
                    }
                    JsonOpContent::List(JsonListOp::Insert { value, .. })
                    | JsonOpContent::MovableList(JsonMovableListOp::Insert { value, .. }) => {
                        value.iter().for_each(|v| hashes_in(v, out))
                    }
                    _ => {}
                }
            }
        }
        ever.extend(since.iter().cloned());
        (ever, since)
    }

    /// Of `files` (the audio in this project's folder), the ones nothing
    /// needs any more, as the module explains. `others` are the other
    /// computers' copies of the document. With `undo` set, what undo
    /// could bring back is kept; unset, the project is being closed, so
    /// undo ends with it.
    pub fn unneeded_audio<'a>(
        &self,
        files: &'a [AudioFile],
        others: &[ProjectDoc],
        undo: bool,
    ) -> Vec<&'a AudioFile> {
        let (ever, since) = self.named_assets();
        let can_undo = undo && (self.can_undo() || self.can_redo());
        let mut keep: HashSet<String> = self.assets_now().into_iter().collect();
        if can_undo {
            keep.extend(self.opened_with.iter().cloned());
        }
        let hash = |f: &AudioFile| f.name.split('.').next().unwrap_or_default().to_string();
        let mut out: Vec<&AudioFile> = (files.iter())
            .filter(|f| !keep.contains(&f.name) && ever.contains(&hash(f)))
            .filter(|f| !(can_undo && since.contains(&hash(f))))
            .collect();
        if out.is_empty() {
            return out;
        }
        // Slower: what snapshots and suggestions would bring back, and
        // what the other copies use.
        keep.extend(self.assets_in_use());
        for o in others {
            keep.extend(o.assets_in_use());
        }
        out.retain(|f| !keep.contains(&f.name));
        out
    }
}

impl ProjectFolder {
    /// The audio files in the folder, by name.
    pub fn audio_files(&self) -> Vec<AudioFile> {
        let mut v: Vec<AudioFile> = fs::read_dir(self.dir.join(ASSETS))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let meta = e.metadata().ok()?;
                (meta.is_file() && is_asset_name(&name)).then_some(AudioFile {
                    name,
                    bytes: meta.len(),
                })
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Every other computer's copy of the document: those in `Devices`
    /// not written by `device`. Copies that can't be read are left out.
    pub fn other_copies(&self, device: &str) -> Vec<ProjectDoc> {
        let mine = self.device_file(device);
        (self.device_files().into_iter())
            .filter(|p| *p != mine)
            .filter_map(|p| fs::read(p).ok())
            .filter_map(|b| ProjectDoc::from_bytes(&b, crate::new_peer()).ok())
            .collect()
    }

    /// The audio in the folder that nothing needs any more, for `doc`
    /// (this computer's copy, as just saved), written here as `device`;
    /// `undo` as for [`ProjectDoc::unneeded_audio`].
    pub fn unused_audio(&self, doc: &ProjectDoc, device: &str, undo: bool) -> Vec<AudioFile> {
        let files = self.audio_files();
        if files.is_empty() {
            return Vec::new();
        }
        // Reading the other copies is slow: only when something might go.
        if doc.unneeded_audio(&files, &[], undo).is_empty() {
            return Vec::new();
        }
        let others = self.other_copies(device);
        (doc.unneeded_audio(&files, &others, undo).into_iter())
            .cloned()
            .collect()
    }

    /// Removes the audio nothing needs any more (see
    /// [`unused_audio`](Self::unused_audio)) and returns what went. A
    /// file that can't be removed now (still open, say) is left for the
    /// next time.
    pub fn remove_unused_audio(
        &self,
        doc: &ProjectDoc,
        device: &str,
        undo: bool,
    ) -> Vec<AudioFile> {
        (self.unused_audio(doc, device, undo).into_iter())
            .filter(|f| fs::remove_file(self.asset(&f.name)).is_ok())
            .collect()
    }
}

/// Adds the hash part of every asset name in `v` (in its strings, at
/// any depth) to `out`.
fn hashes_in(v: &LoroValue, out: &mut HashSet<String>) {
    match v {
        LoroValue::String(s) => asset_hashes(s, out),
        LoroValue::List(l) => l.iter().for_each(|v| hashes_in(v, out)),
        LoroValue::Map(m) => m.values().for_each(|v| hashes_in(v, out)),
        _ => {}
    }
}

/// The hash parts of asset names in `s`: runs of exactly 32 lowercase
/// hex digits followed by a dot, wherever they are (alone, or in a path
/// or a longer value).
fn asset_hashes(s: &str, out: &mut HashSet<String>) {
    let b = s.as_bytes();
    let hex = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
    let mut run = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'.' && run == 32 {
            out.insert(s[i - 32..i].to_string());
        }
        run = if hex(c) { run + 1 } else { 0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_are_found_in_any_string() {
        let mut out = HashSet::new();
        let a = "0123456789abcdef0123456789abcdef";
        asset_hashes(&format!("{a}.wav"), &mut out);
        asset_hashes(&format!("C:/x/Audio/{a}.flac"), &mut out);
        // Too long, too short, or no dot: not an asset name.
        asset_hashes(&format!("f{a}.wav 0123.wav {a}"), &mut out);
        assert_eq!(out.into_iter().collect::<Vec<_>>(), [a]);
    }
}
