// SPDX-License-Identifier: GPL-3.0-or-later
//! Keeps `tests/fixtures/*.json` as the protocol writes it. The Android
//! app's tests (`android/core`) read the same files, so the two sides
//! can't drift apart unnoticed. After changing a message, run
//! `UPDATE_FIXTURES=1 cargo test -p daw-remote --test fixtures` and
//! commit the files.

use std::path::PathBuf;

use daw_remote::{Clip, FromPhone, Marker, ToPhone, Track, VERSION};

fn fixtures() -> Vec<(&'static str, String)> {
    let from = |name, m: &FromPhone| (name, daw_remote::protocol::encode(m));
    let to = |name, m: &ToPhone| (name, daw_remote::protocol::encode(m));
    vec![
        from(
            "hello",
            &FromPhone::Hello {
                version: VERSION,
                name: "Andy's phone".into(),
                code: Some("ABCD2345EFGH6789".into()),
                token: None,
            },
        ),
        from(
            "hello_again",
            &FromPhone::Hello {
                version: VERSION,
                name: "Andy's phone".into(),
                code: None,
                token: Some("TOKEN".into()),
            },
        ),
        from("ping", &FromPhone::Ping),
        from("play", &FromPhone::Play),
        from("stop", &FromPhone::Stop),
        from("to_start", &FromPhone::ToStart),
        from("record", &FromPhone::Record { on: true }),
        from("seek", &FromPhone::Seek { frames: 96_000 }),
        from("seek_bar", &FromPhone::SeekBar { bar: 17 }),
        from("arm", &FromPhone::Arm { track: 1, on: true }),
        from("count_in", &FromPhone::CountIn { on: false }),
        from("metronome", &FromPhone::Metronome { on: true }),
        from("loop", &FromPhone::Loop { on: true }),
        from("undo_last_take", &FromPhone::UndoLastTake),
        to(
            "welcome",
            &ToPhone::Welcome {
                version: VERSION,
                token: "TOKEN".into(),
                desktop: "STUDIO-PC".into(),
            },
        ),
        to(
            "denied",
            &ToPhone::Denied {
                reason: "not paired: scan the code on the desktop".into(),
            },
        ),
        to(
            "layout",
            &ToPhone::Layout {
                project: "Lesson 3".into(),
                sample_rate: 48_000,
                tempo_bpm: 120.0,
                beats_per_bar: 4,
                bar_frames: 96_000.0,
                end_frames: 960_000,
                count_in: true,
                metronome: false,
                looping: true,
                loop_bars: [3, 5],
                may_record: true,
                tracks: vec![
                    Track {
                        name: "Drums".into(),
                        armed: true,
                        can_arm: true,
                        clips: vec![
                            Clip {
                                start: 0,
                                len: 480_000,
                                take: None,
                                muted: false,
                            },
                            Clip {
                                start: 192_000,
                                len: 96_000,
                                take: Some(0),
                                muted: true,
                            },
                            Clip {
                                start: 192_000,
                                len: 100_000,
                                take: Some(1),
                                muted: false,
                            },
                        ],
                    },
                    Track {
                        name: "Guitar".into(),
                        armed: false,
                        can_arm: false,
                        clips: vec![],
                    },
                ],
                markers: vec![
                    Marker {
                        bar: 1,
                        name: "Intro".into(),
                    },
                    Marker {
                        bar: 3,
                        name: "Chorus".into(),
                    },
                ],
            },
        ),
        to(
            "status",
            &ToPhone::Status {
                playing: true,
                counting_in: false,
                recording: true,
                position: 240_000,
                bar: 3,
                beat: 2,
                seconds: 5.0,
                levels: vec![[0.5, 0.25], [0.0, 0.0]],
                master: [0.75, 0.75],
                can_undo_take: true,
            },
        ),
    ]
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn fixtures_match_the_protocol() {
    let update = std::env::var_os("UPDATE_FIXTURES").is_some();
    let mut stale = Vec::new();
    for (name, text) in fixtures() {
        let path = dir().join(format!("{name}.json"));
        if update {
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        if std::fs::read_to_string(&path).ok().as_deref() != Some(&text) {
            stale.push(name);
        }
    }
    assert!(
        stale.is_empty(),
        "fixtures out of date: {stale:?}; run UPDATE_FIXTURES=1 cargo test -p daw-remote --test fixtures"
    );
}

#[test]
fn every_fixture_is_read_back() {
    for (name, text) in fixtures() {
        let ok = daw_remote::protocol::decode::<FromPhone>(&text).is_ok()
            || daw_remote::protocol::decode::<ToPhone>(&text).is_ok();
        assert!(ok, "{name} can't be read back");
    }
}
