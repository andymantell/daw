// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading and writing standard MIDI files (formats 0 and 1), as notes
//! for the project: one track of notes per file track, with the tempo
//! and time signature the file states.
//!
//! Times are scaled to the project's [`PPQ`]. Controllers, pitch bend
//! and other events are left out: only the notes, the tempo, the time
//! signature and the track names are kept.

use crate::{Note, PPQ};

/// One track of a file, as notes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidiTrack {
    pub name: String,
    /// In project ticks from the file's start, sorted by start.
    pub notes: Vec<Note>,
}

/// What a file holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidiFile {
    /// The first tempo the file states, in beats per minute.
    pub tempo_bpm: Option<f64>,
    /// The first time signature the file states, as beats per bar and
    /// the beat unit (4 for quarter notes).
    pub signature: Option<(u8, u8)>,
    /// Tracks with notes in them. Empty tracks (a tempo track, say)
    /// are left out.
    pub tracks: Vec<MidiTrack>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MidiError {
    NotMidi,
    Truncated,
    Timecode,
    Format(u16),
}

impl std::fmt::Display for MidiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiError::NotMidi => write!(f, "not a MIDI file (no MThd header)"),
            MidiError::Truncated => write!(f, "the file ends early"),
            MidiError::Timecode => write!(f, "timecode-based MIDI files are not supported"),
            MidiError::Format(n) => write!(f, "unsupported MIDI file format {n}"),
        }
    }
}

impl std::error::Error for MidiError {}

struct Reader<'a> {
    d: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, MidiError> {
        let b = *self.d.get(self.at).ok_or(MidiError::Truncated)?;
        self.at += 1;
        Ok(b)
    }

    fn u16(&mut self) -> Result<u16, MidiError> {
        Ok(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }

    fn u32(&mut self) -> Result<u32, MidiError> {
        Ok(u32::from_be_bytes([
            self.u8()?,
            self.u8()?,
            self.u8()?,
            self.u8()?,
        ]))
    }

    fn bytes(&mut self, n: usize) -> Result<&[u8], MidiError> {
        let end = self.at.checked_add(n).ok_or(MidiError::Truncated)?;
        let s = self.d.get(self.at..end).ok_or(MidiError::Truncated)?;
        self.at = end;
        Ok(s)
    }

    fn var(&mut self) -> Result<u32, MidiError> {
        let mut v: u32 = 0;
        for _ in 0..4 {
            let b = self.u8()?;
            v = (v << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Ok(v)
    }

    fn done(&self) -> bool {
        self.at >= self.d.len()
    }
}

/// Reads a MIDI file.
pub fn read(data: &[u8]) -> Result<MidiFile, MidiError> {
    let mut r = Reader { d: data, at: 0 };
    if r.bytes(4)? != b"MThd" {
        return Err(MidiError::NotMidi);
    }
    let len = r.u32()? as usize;
    let format = r.u16()?;
    let ntrks = r.u16()?;
    let division = r.u16()?;
    if len > 6 {
        r.bytes(len - 6)?;
    }
    if format > 2 {
        return Err(MidiError::Format(format));
    }
    if division & 0x8000 != 0 {
        return Err(MidiError::Timecode);
    }
    let div = (division as i64).max(1);
    let scale =
        |t: u64| -> i64 { ((t as i128 * PPQ as i128 + (div / 2) as i128) / div as i128) as i64 };
    let mut file = MidiFile::default();
    for _ in 0..ntrks {
        if r.done() {
            break;
        }
        let tag = r.bytes(4)?.to_vec();
        let len = r.u32()? as usize;
        let body = r.bytes(len)?;
        if tag != b"MTrk" {
            continue;
        }
        let mut t = Reader { d: body, at: 0 };
        let mut time: u64 = 0;
        let mut status: u8 = 0;
        let mut track = MidiTrack::default();
        // Notes sounding, per channel and pitch: the start tick, the
        // velocity and where in `track.notes` it will go.
        let mut open: Vec<(u8, u8, u64, u8)> = Vec::new();
        while !t.done() {
            time += t.var()? as u64;
            let mut b = t.u8()?;
            if b & 0x80 != 0 {
                status = b;
                if status < 0xf0 || status == 0xff {
                    b = t.u8()?;
                }
            } else if status == 0 || status >= 0xf0 {
                // Running status without a status to run.
                return Err(MidiError::Truncated);
            }
            match status {
                0xff => {
                    let kind = b;
                    let n = t.var()? as usize;
                    let data = t.bytes(n)?;
                    match kind {
                        0x03 if track.name.is_empty() => {
                            track.name = String::from_utf8_lossy(data).trim().to_string();
                        }
                        0x51 if data.len() == 3 && file.tempo_bpm.is_none() => {
                            let us = u32::from_be_bytes([0, data[0], data[1], data[2]]);
                            if us > 0 {
                                file.tempo_bpm = Some(60_000_000.0 / us as f64);
                            }
                        }
                        0x58 if data.len() >= 2 && file.signature.is_none() => {
                            file.signature = Some((data[0], 1u8 << data[1].min(6)));
                        }
                        _ => {}
                    }
                }
                0xf0 | 0xf7 => {
                    let n = t.var()? as usize;
                    t.bytes(n)?;
                }
                _ => {
                    let channel = status & 0x0f;
                    match status & 0xf0 {
                        0x80 | 0x90 => {
                            let pitch = b & 0x7f;
                            let velocity = t.u8()? & 0x7f;
                            let on = status & 0xf0 == 0x90 && velocity > 0;
                            if on {
                                open.push((channel, pitch, time, velocity));
                            } else if let Some(i) =
                                open.iter().position(|o| o.0 == channel && o.1 == pitch)
                            {
                                let (_, _, start, vel) = open.remove(i);
                                let mut n = Note::new(
                                    scale(start),
                                    (scale(time) - scale(start)).max(1),
                                    pitch,
                                    vel,
                                );
                                n.channel = channel;
                                track.notes.push(n);
                            }
                        }
                        0xa0 | 0xb0 | 0xe0 => {
                            t.u8()?;
                        }
                        _ => {}
                    }
                }
            }
        }
        // Notes never ended end where the track does.
        for (channel, pitch, start, vel) in open {
            let mut n = Note::new(
                scale(start),
                (scale(time) - scale(start)).max(PPQ / 4),
                pitch,
                vel,
            );
            n.channel = channel;
            track.notes.push(n);
        }
        if !track.notes.is_empty() {
            track.notes.sort_by_key(|n| (n.start, n.pitch));
            file.tracks.push(track);
        }
    }
    Ok(file)
}

fn push_var(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 4];
    let mut i = 3;
    buf[i] = (v & 0x7f) as u8;
    v >>= 7;
    while v > 0 {
        i -= 1;
        buf[i] = (v & 0x7f) as u8 | 0x80;
        v >>= 7;
    }
    out.extend_from_slice(&buf[i..]);
}

fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
}

/// Writes a format 1 MIDI file at the project's resolution: a tempo
/// track, then one track per entry of `tracks`.
pub fn write(tempo_bpm: f64, signature: (u8, u8), tracks: &[MidiTrack]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut head = Vec::new();
    head.extend_from_slice(&1u16.to_be_bytes());
    head.extend_from_slice(&(tracks.len() as u16 + 1).to_be_bytes());
    head.extend_from_slice(&(PPQ as u16).to_be_bytes());
    push_chunk(&mut out, b"MThd", &head);

    let mut t0 = Vec::new();
    let us = (60_000_000.0 / tempo_bpm.max(1.0))
        .round()
        .clamp(1.0, 16_777_215.0) as u32;
    t0.extend_from_slice(&[0, 0xff, 0x51, 3]);
    t0.extend_from_slice(&us.to_be_bytes()[1..]);
    let unit_log = signature.1.max(1).ilog2() as u8;
    t0.extend_from_slice(&[0, 0xff, 0x58, 4, signature.0.max(1), unit_log, 24, 8]);
    t0.extend_from_slice(&[0, 0xff, 0x2f, 0]);
    push_chunk(&mut out, b"MTrk", &t0);

    for track in tracks {
        let mut body = Vec::new();
        if !track.name.is_empty() {
            let name = track.name.as_bytes();
            body.extend_from_slice(&[0, 0xff, 0x03]);
            push_var(&mut body, name.len().min(127) as u32);
            body.extend_from_slice(&name[..name.len().min(127)]);
        }
        // (time, order, bytes): offs before ons at the same time.
        let mut events: Vec<(i64, u8, [u8; 3])> = Vec::new();
        for n in &track.notes {
            let ch = n.channel & 0x0f;
            events.push((
                n.start.max(0),
                1,
                [0x90 | ch, n.pitch & 0x7f, n.velocity.clamp(1, 127)],
            ));
            events.push((n.end().max(0), 0, [0x80 | ch, n.pitch & 0x7f, 64]));
        }
        events.sort_by_key(|e| (e.0, e.1));
        let mut last = 0;
        for (at, _, bytes) in events {
            push_var(&mut body, (at - last).max(0) as u32);
            body.extend_from_slice(&bytes);
            last = at;
        }
        body.extend_from_slice(&[0, 0xff, 0x2f, 0]);
        push_chunk(&mut out, b"MTrk", &body);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_tempo_and_signature_round_trip() {
        let mut a = Note::new(0, PPQ, 60, 100);
        a.channel = 2;
        let b = Note::new(PPQ, PPQ / 2, 64, 80);
        let c = Note::new(PPQ, PPQ * 2, 67, 90);
        let tracks = vec![
            MidiTrack {
                name: "Keys".into(),
                notes: vec![a.clone(), b.clone(), c.clone()],
            },
            MidiTrack {
                name: String::new(),
                notes: vec![Note::new(PPQ * 4, PPQ, 36, 127)],
            },
        ];
        let bytes = write(100.0, (3, 4), &tracks);
        let f = read(&bytes).unwrap();
        assert!((f.tempo_bpm.unwrap() - 100.0).abs() < 0.001);
        assert_eq!(f.signature, Some((3, 4)));
        assert_eq!(f.tracks.len(), 2);
        assert_eq!(f.tracks[0].name, "Keys");
        let got: Vec<_> = (f.tracks[0].notes.iter())
            .map(|n| (n.start, n.len, n.pitch, n.velocity, n.channel))
            .collect();
        assert_eq!(
            got,
            [
                (0, PPQ, 60, 100, 2),
                (PPQ, PPQ / 2, 64, 80, 0),
                (PPQ, PPQ * 2, 67, 90, 0)
            ]
        );
        assert_eq!(f.tracks[1].notes[0].start, PPQ * 4);
    }

    #[test]
    fn other_resolutions_are_scaled_and_running_status_read() {
        // Format 0, 96 ticks per beat, one note with running status and
        // a note-on of velocity 0 as its end.
        let mut d = Vec::new();
        push_chunk(&mut d, b"MThd", &[0, 0, 0, 1, 0, 96]);
        let body = [0, 0x90, 60, 100, 96, 60, 0, 0, 0xff, 0x2f, 0];
        push_chunk(&mut d, b"MTrk", &body);
        let f = read(&d).unwrap();
        assert_eq!(f.tracks.len(), 1);
        assert_eq!(f.tracks[0].notes[0].start, 0);
        assert_eq!(f.tracks[0].notes[0].len, PPQ);
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(matches!(read(b"hello"), Err(MidiError::NotMidi)));
        let mut d = Vec::new();
        push_chunk(&mut d, b"MThd", &[0, 1, 0, 1, 0xe7, 0x28]);
        assert!(matches!(read(&d), Err(MidiError::Timecode)));
        let mut d = Vec::new();
        push_chunk(&mut d, b"MThd", &[0, 1, 0, 1, 0, 96]);
        d.extend_from_slice(b"MTrk");
        d.extend_from_slice(&[0, 0, 0, 40]);
        assert!(matches!(read(&d), Err(MidiError::Truncated)));
    }
}
