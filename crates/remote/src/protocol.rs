// SPDX-License-Identifier: GPL-3.0-or-later
//! The messages between a phone and the desktop.
//!
//! Each message is one JSON object on one line, UTF-8, ending in `\n`.
//! Its `type` field says which it is. The phone speaks first with
//! `hello`; the desktop answers `welcome` or `denied` and then sends
//! `layout` (the project as the phone needs to draw it, again whenever
//! it changes) and `status` (where playback is and the meters, many
//! times a second). Unknown fields are ignored on both sides, so either
//! side can be newer than the other within a [`VERSION`].
//!
//! Times are in frames at the desktop's sample rate, given in `layout`.
//! Levels are linear peak values (1.0 is full scale). Bars count from 1.
//!
//! The Android app has the same definitions in Kotlin
//! (`android/core`), checked against the fixtures under
//! `tests/fixtures/` that this crate's tests keep up to date.

use serde::{Deserialize, Serialize};

/// The protocol version. Bumped only when a message changes in a way an
/// older side would misread; adding fields does not.
pub const VERSION: u32 = 1;

/// Sent by the phone.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromPhone {
    /// The first message. `code` is the pairing code scanned from the
    /// desktop's screen; `token` is what `welcome` gave this phone last
    /// time, so it can come back without scanning again.
    Hello {
        version: u32,
        /// What the phone calls itself ("Andy's phone").
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// Sent every few seconds while connected, so the desktop knows the
    /// phone is still there.
    Ping,
    Play,
    Stop,
    /// Back to the start of the timeline.
    ToStart,
    /// Set the transport to record (armed tracks record while playing).
    Record {
        on: bool,
    },
    /// Move the play position, which is also where recording starts.
    Seek {
        frames: i64,
    },
    /// Move the play position to the start of a bar.
    SeekBar {
        bar: i64,
    },
    /// Arm or disarm a track, by its position in `layout`.
    Arm {
        track: usize,
        on: bool,
    },
    CountIn {
        on: bool,
    },
    Metronome {
        on: bool,
    },
    Loop {
        on: bool,
    },
    /// Take the clips of the last recording off their tracks. The
    /// desktop's Undo brings them back.
    UndoLastTake,
}

/// Sent by the desktop.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToPhone {
    /// The phone is in. `token` lets it back next time without a code.
    Welcome {
        version: u32,
        token: String,
        /// The desktop's name, for the phone's screen.
        desktop: String,
    },
    /// The phone is not in, and the connection closes after this.
    Denied { reason: String },
    /// The project as the phone draws it. Sent after `welcome` and
    /// again whenever any of it changes.
    Layout {
        /// The project's name.
        project: String,
        sample_rate: u32,
        tempo_bpm: f64,
        beats_per_bar: u32,
        /// Frames in one bar at this tempo.
        bar_frames: f64,
        /// Where the last clip ends, or a few bars if there are none.
        end_frames: i64,
        count_in: bool,
        metronome: bool,
        looping: bool,
        /// Loop from the start of the first bar to the start of the
        /// second.
        loop_bars: [i64; 2],
        /// False when this person may only comment on the project, so
        /// recording and arming are off.
        may_record: bool,
        tracks: Vec<Track>,
        markers: Vec<Marker>,
    },
    /// Where playback is and how loud things are. Sent many times a
    /// second while a phone is connected.
    Status {
        playing: bool,
        counting_in: bool,
        recording: bool,
        position: i64,
        /// The position as a bar number (from 1) and a beat in it
        /// (from 1).
        bar: i64,
        beat: i64,
        seconds: f64,
        /// Peak level per track, left and right, decaying as the
        /// desktop's meters do. An armed track shows its input.
        levels: Vec<[f32; 2]>,
        master: [f32; 2],
        /// There is a last recording to take off (`undo_last_take`).
        can_undo_take: bool,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Track {
    pub name: String,
    pub armed: bool,
    /// The track has an input to record from.
    pub can_arm: bool,
    pub clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Clip {
    pub start: i64,
    pub len: i64,
    /// Which recorded take this is (from 0); absent for a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<usize>,
    /// An older take under a newer one, not playing.
    #[serde(default)]
    pub muted: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Marker {
    pub bar: i64,
    pub name: String,
}

/// The longest line either side accepts, so a bad peer can't make the
/// other grow without limit.
pub const MAX_LINE: usize = 1 << 20;

/// One message as a line of JSON, newline included.
pub fn encode<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).expect("protocol types always serialise");
    s.push('\n');
    s
}

/// A message from a line of JSON (with or without its newline).
pub fn decode<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, String> {
    serde_json::from_str(line.trim_end_matches(['\r', '\n'])).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_survive_the_wire() {
        let msgs = [
            FromPhone::Hello {
                version: VERSION,
                name: "Phone".into(),
                code: Some("ABCD".into()),
                token: None,
            },
            FromPhone::Seek { frames: 48_000 },
            FromPhone::Arm { track: 2, on: true },
            FromPhone::UndoLastTake,
        ];
        for m in msgs {
            let line = encode(&m);
            assert!(line.ends_with('\n'));
            assert_eq!(decode::<FromPhone>(&line).unwrap(), m);
        }
        let s = ToPhone::Status {
            playing: true,
            counting_in: false,
            recording: true,
            position: 96_000,
            bar: 2,
            beat: 1,
            seconds: 2.0,
            levels: vec![[0.5, 0.25]],
            master: [0.9, 0.9],
            can_undo_take: false,
        };
        assert_eq!(decode::<ToPhone>(&encode(&s)).unwrap(), s);
    }

    #[test]
    fn the_type_field_names_the_message() {
        assert_eq!(encode(&FromPhone::Play), "{\"type\":\"play\"}\n");
        assert_eq!(
            encode(&FromPhone::Record { on: true }),
            "{\"type\":\"record\",\"on\":true}\n"
        );
    }

    #[test]
    fn unknown_fields_and_bad_lines_are_handled() {
        let m: FromPhone = decode("{\"type\":\"play\",\"later\":1}\r\n").unwrap();
        assert_eq!(m, FromPhone::Play);
        assert!(decode::<FromPhone>("{\"type\":\"dance\"}").is_err());
        assert!(decode::<FromPhone>("not json").is_err());
    }
}
