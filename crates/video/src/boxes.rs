// SPDX-License-Identifier: GPL-3.0-or-later
//! A look inside an MP4 file's tables, for the tests: which samples are
//! key frames, how each sample's showing time differs from its decoding
//! time, and the edit list that says where the picture starts. Enough to
//! see why a copy of a film plays wrongly, without decoding it.

use std::path::Path;

/// One track's timing tables, as the file states them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Track {
    /// Sound (`soun`), picture (`vide`) or other.
    pub kind: String,
    /// Ticks a second in the tables below.
    pub timescale: u32,
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

    /// Whether sample `n` (from 0) is a key frame.
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

/// Walks the boxes in `data`, filling `tracks`.
fn walk(data: &[u8], tracks: &mut Vec<Track>) {
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let mut size = be32(&data[at..]) as usize;
        let kind = &data[at + 4..at + 8];
        let mut head = 8;
        if size == 1 && at + 16 <= data.len() {
            size = be64(&data[at + 8..]) as usize;
            head = 16;
        } else if size == 0 {
            size = data.len() - at;
        }
        if size < head || at + size > data.len() {
            return;
        }
        let body = &data[at + head..at + size];
        match kind {
            b"moov" | b"mdia" | b"minf" | b"stbl" | b"edts" => walk(body, tracks),
            b"trak" => {
                tracks.push(Track::default());
                walk(body, tracks);
            }
            _ => {
                if let Some(t) = tracks.last_mut() {
                    table(kind, body, t);
                }
            }
        }
        at += size;
    }
}

/// Reads one of a track's tables into `t`.
fn table(kind: &[u8], body: &[u8], t: &mut Track) {
    if body.len() < 8 {
        return;
    }
    let version = body[0];
    let count = be32(&body[4..]) as usize;
    let rest = &body[8..];
    match kind {
        b"mdhd" => {
            t.timescale = if version == 1 {
                be32(&body[20..])
            } else {
                be32(&body[12..])
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
    let data = std::fs::read(path)?;
    let mut tracks = Vec::new();
    walk(&data, &mut tracks);
    Ok(tracks)
}

/// The file's picture track, if it has one.
pub(crate) fn picture(path: &Path) -> std::io::Result<Option<Track>> {
    Ok(tracks(path)?.into_iter().find(|t| t.kind == "vide"))
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
}
