// SPDX-License-Identifier: GPL-3.0-or-later
//! Live sessions, bundles, shared-folder sync and asset transfer
//! (ARCHITECTURE §6).
//!
//! Every way of sharing ends the same way: the other copy's document is
//! merged into this one ([`ProjectDoc::merge`]), and audio arrives as
//! files named by their content, checked before use.

pub mod bundle;
pub mod watch;

use std::path::Path;

use daw_project::{ProjectDoc, ProjectFolder};
use loro::VersionVector;

/// Whether two documents are copies of one project: whether they share
/// any history. A project's first edit is made by the computer that
/// created it, so two projects made apart never do.
pub fn related(a: &ProjectDoc, b: &ProjectDoc) -> bool {
    let (Ok(a), Ok(b)) = (
        VersionVector::decode(&a.version()),
        VersionVector::decode(&b.version()),
    ) else {
        return false;
    };
    a.keys().any(|peer| b.contains_key(peer))
}

/// Opens the document in a project folder, edited as `peer`: the saved
/// document (or its backup, if damaged) with every computer's copy in
/// `Devices` merged in. Copies that can't be read are named in the
/// second value.
pub fn open_doc(
    folder_path: &Path,
    peer: u64,
) -> Result<(ProjectFolder, ProjectDoc, Vec<String>), String> {
    let (folder, bytes) = ProjectFolder::open(folder_path)?;
    let mut doc = match ProjectDoc::from_bytes(&bytes, peer) {
        Ok(d) => d,
        // The last save is damaged: the one before it.
        Err(e) => folder
            .backup()
            .and_then(|b| ProjectDoc::from_bytes(&b, peer).ok())
            .ok_or(e)?,
    };
    let mut failed = Vec::new();
    for f in folder.device_files() {
        let merged = std::fs::read(&f)
            .map_err(|e| e.to_string())
            .and_then(|b| doc.merge(&b));
        if let Err(e) = merged {
            failed.push(format!("{}: {e}", f.display()));
        }
    }
    Ok((folder, doc, failed))
}
