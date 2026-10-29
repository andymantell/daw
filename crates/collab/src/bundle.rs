// SPDX-License-Identifier: GPL-3.0-or-later
//! Bundles: a whole project in one file, to send by any means.
//!
//! A bundle holds the document and the audio it uses (not exports or
//! anything else in the folder). Importing one into a copy of the same
//! project merges it, so nothing either side did is lost.
//!
//! The file is a plain tar archive (the owner's choice, 2026-10-28), so
//! ordinary tar tools open it and give back a project folder:
//!
//! ```text
//! <project name>/project.daw          the document, first
//! <project name>/Audio/<hash>.<ext>   the audio it uses, frozen audio included
//! ```
//! It isn't compressed: audio barely compresses, and big bundles would
//! only be slower to make and read. A bundle comes from outside, so
//! everything in it is checked: one top folder, only those two kinds of
//! file, plain asset names, and each audio file must match the hash it
//! is named by before it is used. Anything else in it (other files,
//! links) is ignored.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use daw_project::{ASSETS, DOC_FILE, ProjectFolder, is_asset_name};

/// The extension bundles are saved with.
pub const EXT: &str = "dawbundle";

/// Far beyond any real document; stops a damaged size asking for all
/// memory.
const MAX_DOC: u64 = 1 << 30;

/// What a bundle holds, apart from its audio.
pub struct Bundle {
    pub name: String,
    pub doc: Vec<u8>,
}

/// Whether `path` is named like a bundle.
pub fn is_bundle(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXT))
}

/// Writes a bundle of the project `name` to `dest`: the document
/// `doc` and those of `assets` that are in `folder`. The file appears
/// whole or not at all.
pub fn write(
    dest: &Path,
    name: &str,
    doc: &[u8],
    folder: &ProjectFolder,
    assets: &[String],
) -> Result<(), String> {
    let mut assets: Vec<&String> = (assets.iter())
        .filter(|a| is_asset_name(a) && folder.has_asset(a))
        .collect();
    assets.sort();
    assets.dedup();
    let root = top_folder(name);
    let tmp = dest.with_extension(format!("{EXT}.part"));
    let err = |e: std::io::Error| format!("could not write {}: {e}", dest.display());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let written = (|| {
        let mut tar = tar::Builder::new(BufWriter::new(File::create(&tmp)?));
        let mut add = |path: String, len: u64, data: &mut dyn Read| {
            let mut h = tar::Header::new_gnu();
            h.set_size(len);
            h.set_mode(0o644);
            h.set_mtime(now);
            h.set_entry_type(tar::EntryType::Regular);
            tar.append_data(&mut h, path, data)
        };
        add(
            format!("{root}/{DOC_FILE}"),
            doc.len() as u64,
            &mut &doc[..],
        )?;
        for a in &assets {
            let mut f = File::open(folder.asset(a))?;
            let len = f.metadata()?.len();
            add(format!("{root}/{ASSETS}/{a}"), len, &mut (&mut f).take(len))?;
        }
        tar.into_inner()?
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(err(e));
    }
    fs::rename(&tmp, dest).map_err(err)
}

/// Reads a bundle's name and document, without its audio.
pub fn peek(path: &Path) -> Result<Bundle, String> {
    read(path, None)
}

/// Reads a bundle, putting its audio into `folder`'s (files already
/// there are kept), and returns its name and document. Stops at the
/// first problem; audio taken in before it stays, as it was checked.
pub fn unpack(path: &Path, folder: &ProjectFolder) -> Result<Bundle, String> {
    read(path, Some(folder))
}

fn read(path: &Path, into: Option<&ProjectFolder>) -> Result<Bundle, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bad = |what: &str| format!("{} is not a bundle, or is damaged: {what}", path.display());
    let mut tar = tar::Archive::new(BufReader::new(f));
    let entries = tar.entries().map_err(|e| bad(&e.to_string()))?;
    let mut root: Option<String> = None;
    let mut doc: Option<Vec<u8>> = None;
    for entry in entries {
        let mut entry = entry.map_err(|e| bad(&e.to_string()))?;
        let kind = entry.header().entry_type();
        let name = String::from_utf8(entry.path_bytes().into_owned())
            .map_err(|_| bad("a file name isn't text"))?;
        let parts: Vec<&str> = name.trim_end_matches('/').split('/').collect();
        let top = parts[0];
        if top.is_empty() || top == "." || top == ".." || top.contains('\\') {
            return Err(bad(&format!("{name:?} is not inside a project folder")));
        }
        match &root {
            None => root = Some(top.to_string()),
            Some(r) if r != top => return Err(bad("it holds more than one project")),
            Some(_) => {}
        }
        // Links, folders and anything else are never followed or used.
        if !kind.is_file() {
            continue;
        }
        match parts[1..] {
            [DOC_FILE] => {
                if doc.is_some() {
                    return Err(bad("it holds two documents"));
                }
                let len = entry.header().size().map_err(|e| bad(&e.to_string()))?;
                if len > MAX_DOC {
                    return Err(bad("the document is far too big"));
                }
                let mut d = Vec::with_capacity(len as usize);
                entry
                    .read_to_end(&mut d)
                    .map_err(|_| bad("the document is cut short"))?;
                if d.len() as u64 != len {
                    return Err(bad("the document is cut short"));
                }
                doc = Some(d);
                if into.is_none() {
                    break;
                }
            }
            [ASSETS, asset] if is_asset_name(asset) => {
                if let Some(folder) = into {
                    folder.receive_asset(asset, &mut entry)?;
                }
            }
            _ => {}
        }
    }
    match (root, doc) {
        (Some(name), Some(doc)) => Ok(Bundle { name, doc }),
        _ => Err(bad("no project document")),
    }
}

/// The bundle's top folder: the project's name, made safe for a folder
/// name on any system.
fn top_folder(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_matches('.').trim();
    if clean.is_empty() {
        "Project".into()
    } else {
        clean.into()
    }
}
