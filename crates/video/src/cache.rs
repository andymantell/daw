// SPDX-License-Identifier: GPL-3.0-or-later
//! The filmstrip's pictures kept on disk, so a video opened again (or a
//! project reopened) shows its whole strip straight away instead of
//! decoding the film once more.
//!
//! A file is known by its contents, not its name or place: its length
//! and its first and last megabyte. So the copy saving puts in the
//! project's folder, or one that came in a bundle, finds the same
//! pictures. The folder is kept under [`LIMIT`] by letting go of the
//! files used least recently.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::{Frame, Info};

/// How much the folder may hold, in bytes.
pub const LIMIT: u64 = 1 << 30;

const MAGIC: &[u8; 8] = b"DAWFILM1";
/// How much of each end of the file goes into its key.
const SAMPLE: u64 = 1 << 20;

/// What a cache file holds.
pub(crate) struct Strip {
    pub(crate) info: Info,
    pub(crate) every: u64,
    pub(crate) kept: HashMap<u64, Arc<Frame>>,
}

/// A 64-bit FNV-1a hash: the same on every computer and every build.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// The cache file for `video` in `dir`.
pub(crate) fn file_for(dir: &Path, video: &Path) -> io::Result<PathBuf> {
    let mut f = File::open(video)?;
    let len = f.metadata()?.len();
    let mut h = Fnv::new();
    h.write(&len.to_le_bytes());
    // The thumbnails' size is part of what is kept.
    h.write(&crate::thumbs::HEIGHT.to_le_bytes());
    let mut buf = vec![0u8; SAMPLE.min(len) as usize];
    f.read_exact(&mut buf)?;
    h.write(&buf);
    if len > SAMPLE {
        f.seek(SeekFrom::Start(len - SAMPLE.min(len - SAMPLE)))?;
        let mut tail = vec![0u8; SAMPLE.min(len - SAMPLE) as usize];
        f.read_exact(&mut tail)?;
        h.write(&tail);
    }
    Ok(dir.join(format!("{:016x}.strip", h.0)))
}

fn u32_of(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn u64_of(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn f64_of(r: &mut impl Read) -> io::Result<f64> {
    Ok(f64::from_bits(u64_of(r)?))
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

/// The pictures kept for `video`, if there are any.
pub(crate) fn load(dir: &Path, video: &Path) -> io::Result<Strip> {
    let path = file_for(dir, video)?;
    let strip = read(&mut BufReader::new(File::open(&path)?))?;
    // Just used: the last to be let go of.
    if let Ok(f) = File::options().write(true).open(&path) {
        let _ = f.set_modified(SystemTime::now());
    }
    Ok(strip)
}

fn read(r: &mut impl Read) -> io::Result<Strip> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("not a filmstrip"));
    }
    let info = Info {
        width: u32_of(r)?,
        height: u32_of(r)?,
        rate: (u32_of(r)?, u32_of(r)?),
        seconds: f64_of(r)?,
    };
    let every = u64_of(r)?.max(1);
    let count = u32_of(r)?;
    let mut kept = HashMap::new();
    for _ in 0..count {
        let n = u64_of(r)?;
        let (w, h) = (u32_of(r)?, u32_of(r)?);
        let time = f64_of(r)?;
        let len = u32_of(r)? as usize;
        if w == 0 || h == 0 || w > 4096 || h > 4096 || len > 64 << 20 {
            return Err(bad("a picture's size is wrong"));
        }
        let mut packed = vec![0u8; len];
        r.read_exact(&mut packed)?;
        let rgb = lz4_flex::decompress_size_prepended(&packed).map_err(|_| bad("damaged"))?;
        if rgb.len() != w as usize * h as usize * 3 {
            return Err(bad("a picture came out short"));
        }
        let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
        for p in rgb.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        let frame = Frame {
            width: w,
            height: h,
            rgba,
            time,
        };
        kept.insert(n, Arc::new(frame));
    }
    Ok(Strip { info, every, kept })
}

/// Keeps `strip` for `video`, then trims the folder to [`LIMIT`].
pub(crate) fn save(dir: &Path, video: &Path, strip: &Strip) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = file_for(dir, video)?;
    // Written beside it and renamed, so a half-written file is never
    // read.
    let part = path.with_extension("part");
    {
        let mut w = BufWriter::new(File::create(&part)?);
        w.write_all(MAGIC)?;
        let i = &strip.info;
        for v in [i.width, i.height, i.rate.0, i.rate.1] {
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&i.seconds.to_bits().to_le_bytes())?;
        w.write_all(&strip.every.to_le_bytes())?;
        w.write_all(&(strip.kept.len() as u32).to_le_bytes())?;
        let mut frames: Vec<_> = strip.kept.iter().collect();
        frames.sort_by_key(|(n, _)| **n);
        for (n, f) in frames {
            let rgb: Vec<u8> = (f.rgba.as_chunks::<4>().0.iter())
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            let packed = lz4_flex::compress_prepend_size(&rgb);
            w.write_all(&n.to_le_bytes())?;
            w.write_all(&f.width.to_le_bytes())?;
            w.write_all(&f.height.to_le_bytes())?;
            w.write_all(&f.time.to_bits().to_le_bytes())?;
            w.write_all(&(packed.len() as u32).to_le_bytes())?;
            w.write_all(&packed)?;
        }
        w.flush()?;
    }
    std::fs::rename(&part, &path)?;
    trim(dir, LIMIT);
    Ok(())
}

/// Lets go of the files used least recently until `dir` holds no more
/// than `limit` bytes.
pub(crate) fn trim(dir: &Path, limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "strip"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("daw-film-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pictures_are_kept_and_read_back_by_the_files_contents() {
        let d = dir("keep");
        let video = d.join("film.mp4");
        std::fs::write(&video, b"not really a film").unwrap();
        let info = Info {
            width: 64,
            height: 48,
            rate: (12, 1),
            seconds: 2.0,
        };
        let frame = |n: u8| Frame {
            width: 2,
            height: 1,
            rgba: vec![n, 1, 2, 255, 3, 4, n, 255],
            time: n as f64 / 12.0,
        };
        let strip = Strip {
            info,
            every: 2,
            kept: [(0, Arc::new(frame(0))), (2, Arc::new(frame(2)))].into(),
        };
        let cache = d.join("cache");
        save(&cache, &video, &strip).unwrap();
        let back = load(&cache, &video).unwrap();
        assert_eq!(back.info, info);
        assert_eq!(back.every, 2);
        assert_eq!(*back.kept[&2], frame(2));

        // A copy elsewhere, under another name, finds the same.
        let copy = d.join("Copy of film.mp4");
        std::fs::copy(&video, &copy).unwrap();
        assert_eq!(load(&cache, &copy).unwrap().kept.len(), 2);
        // A different file doesn't.
        std::fs::write(&video, b"another film altogether").unwrap();
        assert!(load(&cache, &video).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_folder_lets_go_of_the_oldest_files() {
        let d = dir("trim");
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let p = d.join(format!("{name}.strip"));
            std::fs::write(&p, [0u8; 100]).unwrap();
            let f = File::options().write(true).open(&p).unwrap();
            let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 + i as u64);
            f.set_modified(t).unwrap();
        }
        trim(&d, 250);
        assert!(!d.join("a.strip").exists(), "the oldest goes");
        assert!(d.join("b.strip").exists() && d.join("c.strip").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
