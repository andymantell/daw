// SPDX-License-Identifier: GPL-3.0-or-later
//! Bundles: a whole project in one file, to send by any means.
//!
//! A bundle holds the document and the audio it uses (not exports or
//! anything else in the folder). Importing one into a copy of the same
//! project merges it, so nothing either side did is lost.
//!
//! The file, all numbers little-endian:
//!
//! ```text
//! "DAWBUNDL"  u32 format (1)
//! u16 length, name        the project's name, UTF-8
//! u64 length, document    a Loro snapshot
//! u32 count, then for each audio file:
//!   u16 length, name      <hash>.<ext>, as in the project's Audio folder
//!   u64 length, contents
//! ```
//! Audio is stored as it is: it barely compresses. A bundle comes from
//! outside, so everything in it is checked: names must be plain asset
//! names, lengths must fit the file, and each audio file must match the
//! hash it is named by before it is used.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use daw_project::{ProjectFolder, is_asset_name};

/// The extension bundles are saved with.
pub const EXT: &str = "dawbundle";

const MAGIC: &[u8; 8] = b"DAWBUNDL";
const FORMAT: u32 = 1;
/// Far beyond any real document; stops a damaged length asking for
/// all memory.
const MAX_DOC: u64 = 1 << 30;
const MAX_ASSETS: u32 = 100_000;

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
    let tmp = dest.with_extension(format!("{EXT}.part"));
    let err = |e: std::io::Error| format!("could not write {}: {e}", dest.display());
    let written = (|| {
        let mut out = BufWriter::new(File::create(&tmp)?);
        out.write_all(MAGIC)?;
        out.write_all(&FORMAT.to_le_bytes())?;
        put_str(&mut out, name)?;
        out.write_all(&(doc.len() as u64).to_le_bytes())?;
        out.write_all(doc)?;
        out.write_all(&(assets.len() as u32).to_le_bytes())?;
        for a in &assets {
            let mut f = File::open(folder.asset(a))?;
            let len = f.metadata()?.len();
            put_str(&mut out, a)?;
            out.write_all(&len.to_le_bytes())?;
            let copied = std::io::copy(&mut (&mut f).take(len), &mut out)?;
            if copied != len {
                return Err(std::io::Error::other(format!(
                    "{a} changed while being read"
                )));
            }
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(err(e));
    }
    fs::rename(&tmp, dest).map_err(err)
}

/// Reads a bundle's name and document, without its audio.
pub fn peek(path: &Path) -> Result<Bundle, String> {
    let (b, _) = open(path)?;
    Ok(b)
}

/// Reads a bundle, putting its audio into `folder`'s (files already
/// there are kept), and returns its name and document. Stops at the
/// first problem; audio taken in before it stays, as it was checked.
pub fn unpack(path: &Path, folder: &ProjectFolder) -> Result<Bundle, String> {
    let (b, mut r) = open(path)?;
    let bad = |what: &str| format!("{} is damaged: {what}", path.display());
    let count = r.u32().map_err(|_| bad("no audio list"))?;
    if count > MAX_ASSETS {
        return Err(bad("too many audio files"));
    }
    for _ in 0..count {
        let name = r
            .str()
            .map_err(|_| bad("an audio file's name is cut short"))?;
        let len = r
            .u64()
            .map_err(|_| bad("an audio file's length is cut short"))?;
        if len > r.left {
            return Err(bad("an audio file is cut short"));
        }
        let mut body = (&mut r.inner).take(len);
        folder.receive_asset(&name, &mut body)?;
        if body.limit() != 0 {
            return Err(bad("an audio file is cut short"));
        }
        r.left -= len;
    }
    Ok(b)
}

/// Opens a bundle and reads up to its audio.
fn open(path: &Path) -> Result<(Bundle, Reader), String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let left = f.metadata().map_err(|e| e.to_string())?.len();
    let mut r = Reader {
        inner: BufReader::new(f),
        left,
    };
    let bad = |what: &str| format!("{} is not a bundle, or is damaged: {what}", path.display());
    let mut magic = [0u8; 8];
    r.bytes(&mut magic).map_err(|_| bad("too short"))?;
    if &magic != MAGIC {
        return Err(bad("wrong kind of file"));
    }
    let format = r.u32().map_err(|_| bad("too short"))?;
    if format != FORMAT {
        return Err(format!(
            "{} was made by a newer version (bundle format {format})",
            path.display()
        ));
    }
    let name = r.str().map_err(|_| bad("no name"))?;
    let len = r.u64().map_err(|_| bad("no document"))?;
    if len > MAX_DOC || len > r.left {
        return Err(bad("the document is cut short"));
    }
    let mut doc = vec![0u8; len as usize];
    r.bytes(&mut doc)
        .map_err(|_| bad("the document is cut short"))?;
    Ok((Bundle { name, doc }, r))
}

fn put_str(out: &mut impl Write, s: &str) -> std::io::Result<()> {
    let b = s.as_bytes();
    let len = u16::try_from(b.len()).map_err(|_| std::io::Error::other("name too long"))?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(b)
}

/// Reads the file, keeping count of what is left so that no length read
/// from it can ask for more than is there.
struct Reader {
    inner: BufReader<File>,
    left: u64,
}

impl Reader {
    fn bytes(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        if buf.len() as u64 > self.left {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.inner.read_exact(buf)?;
        self.left -= buf.len() as u64;
        Ok(())
    }

    fn u16(&mut self) -> std::io::Result<u16> {
        let mut b = [0u8; 2];
        self.bytes(&mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    fn u32(&mut self) -> std::io::Result<u32> {
        let mut b = [0u8; 4];
        self.bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn u64(&mut self) -> std::io::Result<u64> {
        let mut b = [0u8; 8];
        self.bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    fn str(&mut self) -> std::io::Result<String> {
        let len = self.u16()?;
        let mut b = vec![0u8; len as usize];
        self.bytes(&mut b)?;
        String::from_utf8(b).map_err(std::io::Error::other)
    }
}
