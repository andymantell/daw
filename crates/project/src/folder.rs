// SPDX-License-Identifier: GPL-3.0-or-later
//! A project on disk: a folder with the document and its audio.
//!
//! ```text
//! My song/
//!   project.daw        the document (a Loro snapshot, history included)
//!   project.daw.bak    the save before, in case the last one is damaged
//!   Audio/<hash>.wav   audio, named by its SHA-256, never changed
//!   Devices/<id>.daw   each computer's own copy of the document
//! ```
//! Naming audio by its content means the same file added twice, or by
//! two people, is stored once, and merging two copies of a project never
//! has two different files under one name.
//!
//! When the folder is shared through a synced folder, two computers
//! saving `project.daw` would overwrite each other, or leave conflict
//! copies. So each computer also writes its own file in `Devices`, which
//! nobody else ever writes, and opening a project merges them all.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const DOC_FILE: &str = "project.daw";
pub const ASSETS: &str = "Audio";
pub const DEVICES: &str = "Devices";

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectFolder {
    pub dir: PathBuf,
}

impl ProjectFolder {
    /// Makes a new project folder at `dir`, which must not already hold
    /// a project.
    pub fn create(dir: &Path) -> Result<ProjectFolder, String> {
        if dir.join(DOC_FILE).exists() {
            return Err(format!("{} already holds a project", dir.display()));
        }
        fs::create_dir_all(dir.join(ASSETS)).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(ProjectFolder {
            dir: dir.to_path_buf(),
        })
    }

    /// Opens the project in `path`, a project folder or the document in
    /// one, and reads the document. Falls back to the backup if the
    /// document can't be read.
    pub fn open(path: &Path) -> Result<(ProjectFolder, Vec<u8>), String> {
        let dir = if path.file_name().is_some_and(|n| n == DOC_FILE) {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            path.to_path_buf()
        };
        let doc = dir.join(DOC_FILE);
        let bytes = fs::read(&doc)
            .or_else(|_| fs::read(backup(&doc)))
            .map_err(|e| format!("{}: {e}", doc.display()))?;
        Ok((ProjectFolder { dir }, bytes))
    }

    /// The save before the last, if there is one: for when the last
    /// one can't be read.
    pub fn backup(&self) -> Option<Vec<u8>> {
        fs::read(backup(&self.dir.join(DOC_FILE))).ok()
    }

    /// The project's name: its folder's.
    pub fn name(&self) -> String {
        self.dir
            .file_name()
            .map_or("Project".into(), |n| n.to_string_lossy().into_owned())
    }

    /// Writes the document. The old one becomes the backup, and the new
    /// one appears whole or not at all.
    pub fn save(&self, bytes: &[u8]) -> Result<(), String> {
        let doc = self.dir.join(DOC_FILE);
        let tmp = doc.with_extension("daw.tmp");
        let err = |e: std::io::Error| format!("could not save {}: {e}", doc.display());
        fs::write(&tmp, bytes).map_err(err)?;
        if doc.exists() {
            fs::copy(&doc, backup(&doc)).map_err(err)?;
        }
        fs::rename(&tmp, &doc).map_err(err)
    }

    /// Like [`save`](Self::save), but the backup becomes a copy of the
    /// new document rather than the old one: for when the old one holds
    /// something that was deleted, such as a comment.
    pub fn save_dropping_backup(&self, bytes: &[u8]) -> Result<(), String> {
        self.save(bytes)?;
        let doc = self.dir.join(DOC_FILE);
        let err = |e: std::io::Error| format!("could not save {}: {e}", doc.display());
        let tmp = doc.with_extension("daw.tmp");
        fs::write(&tmp, bytes).map_err(err)?;
        fs::rename(&tmp, backup(&doc)).map_err(err)
    }

    /// Writes this computer's own copy of the document, named by
    /// `device`, whole or not at all.
    pub fn save_device(&self, device: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.device_file(device);
        let err = |e: std::io::Error| format!("could not save {}: {e}", path.display());
        fs::create_dir_all(self.dir.join(DEVICES)).map_err(err)?;
        let tmp = path.with_extension("daw.tmp");
        fs::write(&tmp, bytes).map_err(err)?;
        fs::rename(&tmp, &path).map_err(err)
    }

    /// The copy of the document that `device` writes.
    pub fn device_file(&self, device: &str) -> PathBuf {
        self.dir.join(DEVICES).join(format!("{device}.daw"))
    }

    /// Every computer's copy of the document, as files.
    pub fn device_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(self.dir.join(DEVICES))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "daw"))
            .collect();
        v.sort();
        v
    }

    /// Where an asset is.
    pub fn asset(&self, name: &str) -> PathBuf {
        self.dir.join(ASSETS).join(name)
    }

    /// Whether `name` is an asset in this project: a plain file name
    /// (not a path to somewhere else) that is there.
    pub fn has_asset(&self, name: &str) -> bool {
        Path::new(name).file_name().is_some_and(|n| n == name) && self.asset(name).exists()
    }

    /// Brings `src` into the project's audio, copied (or moved, if
    /// `take` is set and it can be renamed), and returns its asset name.
    pub fn add_asset(&self, src: &Path, take: bool) -> Result<String, String> {
        let err = |e: std::io::Error| format!("{}: {e}", src.display());
        let ext = src
            .extension()
            .map_or("wav".into(), |e| e.to_string_lossy().to_lowercase());
        let name = format!("{}.{ext}", hash_file(src).map_err(err)?);
        let dest = self.asset(&name);
        if dest.exists() {
            if take {
                let _ = fs::remove_file(src);
            }
            return Ok(name);
        }
        fs::create_dir_all(self.dir.join(ASSETS)).map_err(err)?;
        if take && fs::rename(src, &dest).is_ok() {
            return Ok(name);
        }
        let tmp = dest.with_extension("part");
        fs::copy(src, &tmp).map_err(err)?;
        fs::rename(&tmp, &dest).map_err(err)?;
        if take {
            let _ = fs::remove_file(src);
        }
        Ok(name)
    }

    /// Takes in an asset sent from elsewhere (a bundle, another
    /// computer), checking that `src` really holds what `name` says. An
    /// asset already here is read past and kept.
    pub fn receive_asset(&self, name: &str, src: &mut impl Read) -> Result<(), String> {
        if !is_asset_name(name) {
            return Err(format!("{name:?} is not an audio file name"));
        }
        let dest = self.asset(name);
        let err = |e: std::io::Error| format!("{}: {e}", dest.display());
        if dest.exists() {
            std::io::copy(src, &mut std::io::sink()).map_err(err)?;
            return Ok(());
        }
        fs::create_dir_all(self.dir.join(ASSETS)).map_err(err)?;
        let tmp = dest.with_extension("part");
        let mut out = fs::File::create(&tmp).map_err(err)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let copied = (|| loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                return out.sync_all();
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        })();
        drop(out);
        if let Err(e) = copied {
            let _ = fs::remove_file(&tmp);
            return Err(err(e));
        }
        if !name.starts_with(&hex(&h.finalize()[..16])) {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "{name} is damaged: its contents don't match its name"
            ));
        }
        fs::rename(&tmp, &dest).map_err(err)
    }
}

/// Whether `name` is how assets are named: 32 hex digits, a dot, and a
/// short extension (not `part`, which marks a file still arriving).
/// Nothing else (no paths) is accepted from outside.
pub fn is_asset_name(name: &str) -> bool {
    let Some((hash, ext)) = name.split_once('.') else {
        return false;
    };
    hash.len() == 32
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && (1..=8).contains(&ext.len())
        && ext
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && ext != "part"
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn backup(doc: &Path) -> PathBuf {
    doc.with_extension("daw.bak")
}

/// The first 128 bits of the file's SHA-256, in hex.
fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()[..16]))
}
