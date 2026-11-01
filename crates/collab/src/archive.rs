// SPDX-License-Identifier: GPL-3.0-or-later
//! A folder packed into one plain tar file (like a bundle, not
//! compressed): for stem packages, which are sent to someone working in
//! another program, so ordinary tar tools must open them.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

/// Packs the files in `dir` (not its subfolders) into `dest`, inside a
/// top folder named like `dir`. The file appears whole or not at all.
pub fn pack(dir: &Path, dest: &Path) -> Result<(), String> {
    let root = dir
        .file_name()
        .map_or("Stems".into(), |n| n.to_string_lossy().into_owned());
    let mut files: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    let tmp = dest.with_extension("tar.part");
    let written = (|| {
        let mut tar = tar::Builder::new(BufWriter::new(File::create(&tmp)?));
        for f in &files {
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            tar.append_path_with_name(f, format!("{root}/{name}"))?;
        }
        tar.into_inner()?
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("could not write {}: {e}", dest.display()));
    }
    fs::rename(&tmp, dest).map_err(|e| format!("could not write {}: {e}", dest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_packs_into_one_file_tar_can_read() {
        let base = std::env::temp_dir().join(format!("daw-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dir = base.join("Stems 1");
        fs::create_dir_all(dir.join("inner")).unwrap();
        fs::write(dir.join("01 Guitar.wav"), b"guitar").unwrap();
        fs::write(dir.join("About these stems.txt"), b"120 BPM").unwrap();
        let dest = base.join("Stems 1.tar");
        pack(&dir, &dest).unwrap();
        let mut tar = tar::Archive::new(File::open(&dest).unwrap());
        let names: Vec<String> = (tar.entries().unwrap())
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert_eq!(
            names,
            ["Stems 1/01 Guitar.wav", "Stems 1/About these stems.txt"]
        );
        let _ = fs::remove_dir_all(&base);
    }
}
