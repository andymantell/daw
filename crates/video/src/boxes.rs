// SPDX-License-Identifier: GPL-3.0-or-later
//! A look inside an MP4 file's tables: which samples are key frames,
//! how each sample's showing time differs from its decoding time, and
//! the edit list that says where the picture starts. Enough to see why
//! a copy of a film plays wrongly without decoding it, and to put an
//! edit list into a copy whose writer can't ([`start_later`]). Used by
//! the Windows writer, and by the tests everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// One track's timing tables, as the file states them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Track {
    /// Sound (`soun`), picture (`vide`) or other.
    pub kind: String,
    /// Ticks a second in the tables below.
    pub timescale: u32,
    /// How long the track's samples run, in ticks.
    pub duration: u64,
    /// Edit list entries: (how long, in the movie's timescale; where in
    /// the track it starts, in the track's timescale, or -1 for an empty
    /// stretch).
    pub edits: Vec<(u64, i64)>,
    /// Runs of (count, how long each sample is decoded for).
    pub durations: Vec<(u32, u32)>,
    /// Runs of (count, how far each sample's showing time is after its
    /// decoding time).
    pub offsets: Vec<(u32, i32)>,
    /// The key frames, counted from 1; empty when every sample is one.
    pub sync: Vec<u32>,
    /// How many samples there are.
    pub samples: u32,
}

impl Track {
    /// When sample `n` (from 0) is decoded and shown, in ticks.
    #[cfg(test)]
    pub fn times(&self, n: usize) -> (i64, i64) {
        let mut dts = 0i64;
        let mut i = 0usize;
        'runs: for &(count, delta) in &self.durations {
            for _ in 0..count {
                if i == n {
                    break 'runs;
                }
                dts += delta as i64;
                i += 1;
            }
        }
        let mut offset = 0i64;
        let mut i = 0usize;
        'runs: for &(count, off) in &self.offsets {
            for _ in 0..count {
                if i == n {
                    offset = off as i64;
                    break 'runs;
                }
                i += 1;
            }
        }
        (dts, dts + offset)
    }

    /// When the last sample stops showing, in ticks: the furthest any
    /// sample's showing time and length reach.
    pub fn shown_end(&self) -> u64 {
        let mut offsets = self
            .offsets
            .iter()
            .flat_map(|&(count, off)| std::iter::repeat_n(off, count as usize));
        let (mut dts, mut end) = (0i64, 0i64);
        for &(count, delta) in &self.durations {
            for _ in 0..count {
                let off = offsets.next().unwrap_or(0) as i64;
                end = end.max(dts + off + delta as i64);
                dts += delta as i64;
            }
        }
        end.max(0) as u64
    }

    /// Whether sample `n` (from 0) is a key frame.
    #[cfg(test)]
    pub fn is_sync(&self, n: usize) -> bool {
        self.sync.is_empty() || self.sync.contains(&(n as u32 + 1))
    }
}

fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

fn be64(d: &[u8]) -> u64 {
    u64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
}

/// One box: where it is in the data it was found in, its whole size,
/// how long its header is, and its kind.
#[derive(Clone, Copy, Debug)]
struct Header {
    at: usize,
    size: usize,
    head: usize,
    kind: [u8; 4],
}

impl Header {
    fn body<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        &data[self.at + self.head..self.at + self.size]
    }
}

/// The boxes one after another in `data`, stopping at anything broken.
fn boxes_in(data: &[u8]) -> Vec<Header> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let mut size = be32(&data[at..]) as usize;
        let kind = [data[at + 4], data[at + 5], data[at + 6], data[at + 7]];
        let mut head = 8;
        if size == 1 && at + 16 <= data.len() {
            size = be64(&data[at + 8..]) as usize;
            head = 16;
        } else if size == 0 {
            size = data.len() - at;
        }
        if size < head || at + size > data.len() {
            break;
        }
        out.push(Header {
            at,
            size,
            head,
            kind,
        });
        at += size;
    }
    out
}

/// Boxes that hold other boxes, as far as the tables here go.
fn holds_boxes(kind: &[u8; 4]) -> bool {
    matches!(
        kind,
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts"
    )
}

/// Walks the boxes in `data`, filling `tracks`.
fn walk(data: &[u8], tracks: &mut Vec<Track>) {
    for b in boxes_in(data) {
        let body = b.body(data);
        if &b.kind == b"trak" {
            tracks.push(Track::default());
            walk(body, tracks);
        } else if holds_boxes(&b.kind) {
            walk(body, tracks);
        } else if let Some(t) = tracks.last_mut() {
            table(&b.kind, body, t);
        }
    }
}

/// Reads one of a track's tables into `t`.
fn table(kind: &[u8; 4], body: &[u8], t: &mut Track) {
    if body.len() < 8 {
        return;
    }
    let version = body[0];
    let count = be32(&body[4..]) as usize;
    let rest = &body[8..];
    match kind {
        b"mdhd" => {
            (t.timescale, t.duration) = if version == 1 && body.len() >= 32 {
                (be32(&body[20..]), be64(&body[24..]))
            } else if body.len() >= 20 {
                (be32(&body[12..]), be32(&body[16..]) as u64)
            } else {
                return;
            };
        }
        b"hdlr" if body.len() >= 12 => {
            t.kind = String::from_utf8_lossy(&body[8..12]).into_owned();
        }
        b"elst" => {
            let each = if version == 1 { 20 } else { 12 };
            for e in rest.chunks_exact(each).take(count) {
                t.edits.push(if version == 1 {
                    (be64(e), be64(&e[8..]) as i64)
                } else {
                    (be32(e) as u64, be32(&e[4..]) as i32 as i64)
                });
            }
        }
        b"stts" => {
            for e in rest.as_chunks::<8>().0.iter().take(count) {
                t.durations.push((be32(e), be32(&e[4..])));
                t.samples += be32(e);
            }
        }
        b"ctts" => {
            for e in rest.as_chunks::<8>().0.iter().take(count) {
                t.offsets.push((be32(e), be32(&e[4..]) as i32));
            }
        }
        b"stss" => {
            for e in rest.as_chunks::<4>().0.iter().take(count) {
                t.sync.push(be32(e));
            }
        }
        _ => {}
    }
}

/// The tracks of the MP4 file at `path`.
pub(crate) fn tracks(path: &Path) -> std::io::Result<Vec<Track>> {
    let (_, moov, _) = find_moov(&mut std::fs::File::open(path)?)?;
    let mut tracks = Vec::new();
    walk(&moov, &mut tracks);
    Ok(tracks)
}

/// The file's picture track, if it has one.
pub(crate) fn picture(path: &Path) -> std::io::Result<Option<Track>> {
    Ok(tracks(path)?.into_iter().find(|t| t.kind == "vide"))
}

fn bad(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, what)
}

/// The file's `moov` box: where it starts, its contents, and whether
/// any box holding sample data (`mdat`) comes after it, so moving it
/// would move the samples' offsets.
fn find_moov(f: &mut std::fs::File) -> std::io::Result<(u64, Vec<u8>, bool)> {
    let len = f.metadata()?.len();
    let mut at = 0u64;
    let mut moov: Option<(u64, u64)> = None;
    let mut data_after = false;
    while at + 8 <= len {
        f.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 16];
        f.read_exact(&mut head[..8])?;
        let mut size = be32(&head) as u64;
        let kind = [head[4], head[5], head[6], head[7]];
        if size == 1 {
            f.read_exact(&mut head[8..])?;
            size = be64(&head[8..]);
        } else if size == 0 {
            size = len - at;
        }
        if size < 8 || at + size > len {
            return Err(bad("a broken box"));
        }
        match &kind {
            b"moov" => moov = Some((at, size)),
            b"mdat" if moov.is_some() => data_after = true,
            _ => {}
        }
        at += size;
    }
    let (at, size) = moov.ok_or_else(|| bad("no moov box"))?;
    f.seek(SeekFrom::Start(at))?;
    let mut data = vec![0u8; size as usize];
    f.read_exact(&mut data)?;
    Ok((at, data, data_after))
}

/// Rewrites the MP4 file at `path` so its picture is shown from `delay`
/// ticks (of the picture track's timescale) into the track, by putting
/// an edit list on that track. A writer that can't take decoding times
/// below zero is given every showing time `delay` later instead, and
/// this puts the picture back where it was: its first frame at 0, with
/// the sound.
pub(crate) fn start_later(path: &Path, delay: u64) -> std::io::Result<()> {
    if delay == 0 {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let (at, moov, data_after) = find_moov(&mut f)?;
    let mut new = with_edit(&moov, delay)?;
    let grown = new.len() as i64 - moov.len() as i64;
    if data_after && grown != 0 {
        move_chunks(&mut new, grown);
    }
    if at + moov.len() as u64 == f.metadata()?.len() {
        // The last box: only it is written again.
        f.set_len(at)?;
        f.seek(SeekFrom::Start(at))?;
        f.write_all(&new)?;
        return Ok(());
    }
    // Everything after it moves along: copied into a new file.
    let tmp = path.with_extension("moov.tmp");
    let r = (|| -> std::io::Result<()> {
        let mut out = std::fs::File::create(&tmp)?;
        f.seek(SeekFrom::Start(0))?;
        std::io::copy(&mut (&mut f).take(at), &mut out)?;
        out.write_all(&new)?;
        f.seek(SeekFrom::Start(at + moov.len() as u64))?;
        std::io::copy(&mut f, &mut out)?;
        out.flush()?;
        drop(out);
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// `moov` with an edit list on its picture track that starts the
/// picture `delay` ticks in.
fn with_edit(moov: &[u8], delay: u64) -> std::io::Result<Vec<u8>> {
    let top = boxes_in(moov);
    let (Some(head), Some(body)) = (top.first(), moov.get(8..)) else {
        return Err(bad("an empty moov box"));
    };
    if head.head != 8 || &head.kind != b"moov" || head.size != moov.len() {
        return Err(bad("an unusual moov box"));
    }
    let inner = boxes_in(body);
    let mvhd = inner
        .iter()
        .find(|b| &b.kind == b"mvhd")
        .ok_or_else(|| bad("no mvhd box"))?;
    let mvhd_body = mvhd.body(body);
    let movie_scale = if mvhd_body.first() == Some(&1) {
        be32(&mvhd_body[20..])
    } else {
        be32(&mvhd_body[12..])
    };
    let mut out = Vec::with_capacity(moov.len() + 40);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"moov");
    let mut done = false;
    for b in &inner {
        let raw = &body[b.at..b.at + b.size];
        if &b.kind == b"trak" && !done && is_picture(b.body(body)) {
            out.extend_from_slice(&edited_trak(b.body(body), movie_scale, delay)?);
            done = true;
        } else {
            out.extend_from_slice(raw);
        }
    }
    if !done {
        return Err(bad("no picture track"));
    }
    let size = u32::try_from(out.len()).map_err(|_| bad("too big a moov box"))?;
    out[..4].copy_from_slice(&size.to_be_bytes());
    Ok(out)
}

/// Whether a track (the body of its `trak` box) holds pictures.
fn is_picture(trak: &[u8]) -> bool {
    let mut t = Track::default();
    walk_one(trak, &mut t);
    t.kind == "vide"
}

/// Reads one track's tables, from the body of its `trak` box.
fn walk_one(trak: &[u8], t: &mut Track) {
    for b in boxes_in(trak) {
        let body = b.body(trak);
        if holds_boxes(&b.kind) {
            walk_one(body, t);
        } else {
            table(&b.kind, body, t);
        }
    }
}

/// The `trak` box (whole, with its header) rebuilt with an edit list
/// that starts it `delay` ticks in, after its `tkhd`, in place of any
/// edit list it had.
fn edited_trak(trak: &[u8], movie_scale: u32, delay: u64) -> std::io::Result<Vec<u8>> {
    let mut t = Track::default();
    walk_one(trak, &mut t);
    if t.timescale == 0 || movie_scale == 0 {
        return Err(bad("a track without a timescale"));
    }
    let in_movie =
        |ticks: u64| (ticks as f64 * movie_scale as f64 / t.timescale as f64).round() as u64;
    let mut out = Vec::with_capacity(trak.len() + 48);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"trak");
    for b in boxes_in(trak) {
        match &b.kind {
            b"edts" => continue,
            b"tkhd" => {
                let mut tkhd = trak[b.at..b.at + b.size].to_vec();
                let body = b.head;
                // How long the track is shown for, in the movie's
                // timescale: up to its last frame's end, from `delay`.
                let (duration_at, wide) = if tkhd.get(body) == Some(&1) {
                    (body + 28, true)
                } else {
                    (body + 20, false)
                };
                let shown = in_movie(t.shown_end().saturating_sub(delay));
                if wide {
                    tkhd[duration_at..duration_at + 8].copy_from_slice(&shown.to_be_bytes());
                } else {
                    tkhd[duration_at..duration_at + 4]
                        .copy_from_slice(&(shown as u32).to_be_bytes());
                }
                out.extend_from_slice(&tkhd);
                // One entry: the whole track from `delay`, at full rate.
                let mut elst = Vec::with_capacity(28);
                elst.extend_from_slice(&28u32.to_be_bytes());
                elst.extend_from_slice(b"elst");
                elst.extend_from_slice(&[0, 0, 0, 0]);
                elst.extend_from_slice(&1u32.to_be_bytes());
                elst.extend_from_slice(&(shown as u32).to_be_bytes());
                elst.extend_from_slice(&(delay as u32).to_be_bytes());
                elst.extend_from_slice(&0x0001_0000u32.to_be_bytes());
                out.extend_from_slice(&36u32.to_be_bytes());
                out.extend_from_slice(b"edts");
                out.extend_from_slice(&elst);
            }
            _ => out.extend_from_slice(&trak[b.at..b.at + b.size]),
        }
    }
    let size = u32::try_from(out.len()).map_err(|_| bad("too big a track"))?;
    out[..4].copy_from_slice(&size.to_be_bytes());
    Ok(out)
}

/// Moves every chunk offset in `moov` (the whole box) along by `by`
/// bytes, for sample data that now sits later in the file.
fn move_chunks(moov: &mut [u8], by: i64) {
    fn go(data: &mut [u8], by: i64) {
        for b in boxes_in(data) {
            let (from, to) = (b.at + b.head, b.at + b.size);
            if holds_boxes(&b.kind) {
                go(&mut data[from..to], by);
            } else if &b.kind == b"stco" && to - from >= 8 {
                let n = be32(&data[from + 4..]) as usize;
                for e in data[from + 8..to].as_chunks_mut::<4>().0.iter_mut().take(n) {
                    *e = (be32(e) as i64 + by).max(0).to_be_bytes()[4..]
                        .try_into()
                        .unwrap();
                }
            } else if &b.kind == b"co64" && to - from >= 8 {
                let n = be32(&data[from + 4..]) as usize;
                for e in data[from + 8..to].as_chunks_mut::<8>().0.iter_mut().take(n) {
                    *e = ((be64(e) as i64 + by).max(0) as u64).to_be_bytes();
                }
            }
        }
    }
    go(moov, by);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn the_fixtures_tables_are_read() {
        let t = picture(&fixture("counting.mp4")).unwrap().unwrap();
        assert_eq!(t.timescale, 12288);
        assert_eq!(t.samples, 24);
        assert_eq!(t.edits, vec![(2000, 2048)], "starts two frames in");
        assert_eq!(t.sync, vec![1, 12, 23]);
        assert_eq!(t.times(0), (0, 2048));
        assert_eq!(t.times(1), (1024, 3072));
        assert!(t.is_sync(0) && !t.is_sync(1) && t.is_sync(11));

        // Frames out of order: the second sample decoded is shown fourth.
        let t = picture(&fixture("counting-bframes.mp4")).unwrap().unwrap();
        assert_eq!(t.samples, 24);
        assert_eq!(t.sync, vec![1, 9, 17]);
        assert_eq!(t.times(0), (0, 2048));
        assert_eq!(t.times(1), (1024, 5120));
        assert_eq!(t.times(2), (2048, 3072));
    }

    /// An edit list goes in whether the tables come before or after the
    /// sample data, and the sample data is found afterwards either way.
    #[test]
    fn an_edit_list_is_put_in_and_the_samples_still_found() {
        for name in ["counting.mp4", "counting-bframes.mp4"] {
            let d = std::env::temp_dir().join(format!("daw-boxes-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join("film.mp4");
            std::fs::copy(fixture(name), &p).unwrap();
            let before = tracks(&p).unwrap();
            let chunks_before = first_chunk(&p);
            start_later(&p, 3072).unwrap();
            let after = tracks(&p).unwrap();
            let v = after.iter().find(|t| t.kind == "vide").unwrap();
            assert_eq!(v.edits.len(), 1, "{:?}", v.edits);
            assert_eq!(v.edits[0].1, 3072);
            // The last frame ends 2048 + 24 * 1024 ticks in.
            assert_eq!(v.edits[0].0, 1917, "from a quarter second in");
            // Nothing else changed.
            let same = |t: &Track| Track {
                edits: Vec::new(),
                ..t.clone()
            };
            assert_eq!(
                before.iter().map(same).collect::<Vec<_>>(),
                after.iter().map(same).collect::<Vec<_>>()
            );
            // The sample data is where the tables now say.
            let (at, bytes) = first_chunk(&p);
            assert_eq!(bytes, chunks_before.1, "{name}");
            assert!(at >= chunks_before.0);
            // Still a file the tables can be read from after a second
            // edit (the first is replaced).
            start_later(&p, 1024).unwrap();
            let v = picture(&p).unwrap().unwrap();
            assert_eq!(v.edits, vec![(2083, 1024)]);
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// Where the picture's first chunk is, and its first bytes.
    fn first_chunk(p: &Path) -> (u64, Vec<u8>) {
        let data = std::fs::read(p).unwrap();
        let mut at = None;
        fn find(data: &[u8], at: &mut Option<u64>) {
            for b in boxes_in(data) {
                let body = b.body(data);
                if holds_boxes(&b.kind) {
                    find(body, at);
                } else if &b.kind == b"stco" && at.is_none() {
                    *at = Some(be32(&body[8..]) as u64);
                }
            }
        }
        find(&data, &mut at);
        let at = at.unwrap();
        (at, data[at as usize..at as usize + 16].to_vec())
    }
}
