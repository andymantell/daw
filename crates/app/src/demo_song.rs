// SPDX-License-Identifier: GPL-3.0-or-later
//! The demo project that comes with the repository, `examples/house-demo`:
//! a layered house track built entirely through the assistant interface,
//! so it shows the app off (every built-in synth, a drum machine pattern,
//! a sung phrase with its pitch found, buses, sends, effects, automation,
//! sections, a loop and comments) and doubles as an end-to-end test that
//! an assistant can make a whole song.
//!
//! To rebuild the folder after a change here or to the project format:
//! `cargo test -p daw-app --bin daw demo_song::write_house_demo -- --ignored`.

#![cfg(test)]

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agent::driver::Driver;

/// The ids of the things the song is made of, for tests and renders.
pub(crate) struct Demo {
    pub(crate) drums: String,
    pub(crate) bass: String,
    pub(crate) sub: String,
    pub(crate) stabs: String,
    pub(crate) pad: String,
    pub(crate) keys: String,
    pub(crate) strings: String,
    pub(crate) pluck: String,
    pub(crate) lead: String,
    pub(crate) acid: String,
    pub(crate) vocal: String,
    pub(crate) vocal_clip: String,
    pub(crate) keys_clip: String,
    pub(crate) drum_bus: String,
    pub(crate) music_bus: String,
    pub(crate) vocal_bus: String,
}

pub(crate) const BPM: f64 = 124.0;

fn c(d: &mut Driver, tool: &str, args: Value) -> Value {
    d.call(tool, args.clone())
        .unwrap_or_else(|e| panic!("{tool} {args}: {e}"))
}

fn track(d: &mut Driver, kind: &str, name: &str) -> String {
    c(d, "add_track", json!({"kind": kind, "name": name}))["track"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Steps the track's instrument through its sounds until `name` plays.
fn sound(d: &mut Driver, t: &str, name: &str) {
    c(d, "select_track", json!({"track": t}));
    for _ in 0..16 {
        let now = c(d, "get_instrument", json!({"track": t}));
        if now["sound"] == name {
            return;
        }
        c(d, "run_command", json!({"id": "next-sound"}));
    }
    panic!("{t} has no sound {name}");
}

/// A note clip at `bar`, `bars` long, with `notes`, and linked copies at
/// each bar in `copies`.
fn part(
    d: &mut Driver,
    t: &str,
    name: &str,
    bar: i64,
    bars: i64,
    notes: Value,
    copies: &[f64],
) -> String {
    let clip = c(
        d,
        "add_note_clip",
        json!({"track": t, "bar": bar, "bars": bars, "name": name}),
    )["clip"]
        .as_str()
        .unwrap()
        .to_string();
    c(d, "edit_notes", json!({"clip": clip, "add": notes}));
    if !copies.is_empty() {
        c(d, "duplicate_clip", json!({"clip": clip, "bars": copies}));
    }
    clip
}

fn every(from: i64, to: i64, step: i64) -> Vec<f64> {
    (from..=to)
        .step_by(step as usize)
        .map(|b| b as f64)
        .collect()
}

fn n(beat: f64, beats: f64, pitch: i64, vel: i64) -> Value {
    json!({"beat": beat, "beats": beats, "pitch": pitch, "velocity": vel})
}

fn chord(beat: f64, beats: f64, name: &str, octave: i64, vel: i64) -> Value {
    json!({"beat": beat, "beats": beats, "chord": name, "octave": octave, "velocity": vel})
}

/// The first automatable setting of the track whose name has `word` in
/// it, as list_parameters names it.
fn param_like(d: &mut Driver, t: &str, word: &str) -> Option<String> {
    let p = c(d, "list_parameters", json!({"track": t}));
    let groups = p.as_array()?;
    for g in groups {
        for q in g["params"].as_array()? {
            let name = q["param"].as_str()?;
            let shown = q["name"].as_str().unwrap_or("");
            if name.to_lowercase().contains(word) || shown.to_lowercase().contains(word) {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Writes a sung-style phrase in A minor, four bars at the song's tempo,
/// as a mono wav: harmonics with vibrato and an envelope, each note a
/// little off pitch so the pitch editor has something to show.
pub(crate) fn write_vocal(path: &Path) {
    let rate = 48_000u32;
    let beat = 60.0 / BPM;
    // (midi note, beats); None is a rest.
    let phrase: [(Option<i32>, f64); 14] = [
        (Some(69), 1.5),
        (None, 0.5),
        (Some(72), 1.0),
        (Some(71), 1.0),
        (Some(69), 2.0),
        (None, 2.0),
        (Some(64), 1.0),
        (Some(67), 1.0),
        (Some(69), 1.5),
        (None, 0.5),
        (Some(72), 1.0),
        (Some(74), 1.0),
        (Some(72), 1.5),
        (None, 0.5),
    ];
    let detune = [12.0, -20.0, 25.0, -8.0, 18.0, -30.0, 9.0, -14.0, 22.0];
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut sung = 0usize;
    for (note, beats) in phrase {
        let len = (beats * beat * rate as f64) as usize;
        let Some(m) = note else {
            for _ in 0..len {
                w.write_sample(0i16).unwrap();
            }
            continue;
        };
        let cents = detune[sung % detune.len()];
        sung += 1;
        let f0 = 440.0 * 2f64.powf((m as f64 - 69.0 + cents / 100.0) / 12.0);
        let mut phase = 0.0f64;
        for i in 0..len {
            let t = i as f64 / rate as f64;
            let vib =
                1.0 + 0.006 * (2.0 * std::f64::consts::PI * 5.5 * t).sin() * (t / 0.3).min(1.0);
            phase += 2.0 * std::f64::consts::PI * f0 * vib / rate as f64;
            let s: f64 = (1..7)
                .map(|k| (0.6 / k as f64) * (k as f64 * phase).sin())
                .sum();
            let left = (len - i) as f64 / rate as f64;
            let env = (t / 0.04).min(1.0) * (left / 0.08).min(1.0);
            w.write_sample((0.3 * s * env * 32767.0) as i16).unwrap();
        }
    }
    w.finalize().unwrap();
}

/// Builds the song in the open (empty) project. `vocal` is where the
/// sung phrase is written first.
pub(crate) fn build(d: &mut Driver, vocal: &Path) -> Demo {
    c(
        d,
        "set_tempo",
        json!({"bpm": BPM, "beats_per_bar": 4, "beat_unit": 4}),
    );
    c(d, "set_key", json!({"key": "A minor"}));
    c(
        d,
        "edit_markers",
        json!({"add": [
            {"bar": 1, "name": "Intro"}, {"bar": 17, "name": "Build"},
            {"bar": 33, "name": "Drop"}, {"bar": 49, "name": "Breakdown"},
            {"bar": 65, "name": "Drop 2"}, {"bar": 97, "name": "Outro"},
            {"bar": 113, "name": "End"}]}),
    );

    // Drums: three patterns on one drum machine.
    let drums = track(d, "drum-machine", "Drums");
    let pattern = |d: &mut Driver,
                   name: &str,
                   bar: i64,
                   len_beats: i64,
                   copies: &[f64],
                   clap: bool,
                   hats: bool,
                   kick: bool| {
        let p = c(
            d,
            "edit_pattern",
            json!({"new": true, "empty": true, "name": name, "track": drums, "bar": bar, "bars": 1,
                   "swing": 0.08,
                   "add_lanes": [
                       {"name": "Kick", "pad": "Kick", "steps": 16, "rate": "1/16"},
                       {"name": "Clap", "pad": "Clap", "steps": 16, "rate": "1/16"},
                       {"name": "Closed hat", "pad": "Closed hi-hat", "steps": 16, "rate": "1/16"},
                       {"name": "Open hat", "pad": "Open hi-hat", "steps": 16, "rate": "1/16"},
                       {"name": "Rim", "pad": "Rim", "steps": 16, "rate": "1/16"}]}),
        );
        let id = p["pattern"].as_str().unwrap().to_string();
        let clip = p["clip"].as_str().unwrap().to_string();
        let mut steps = Vec::new();
        if kick {
            for s in [1, 5, 9, 13] {
                steps.push(json!({"lane": "Kick", "step": s, "on": true, "velocity": 120}));
            }
        }
        if clap {
            for s in [5, 13] {
                steps.push(json!({"lane": "Clap", "step": s, "on": true, "velocity": 104}));
            }
            steps.push(json!({"lane": "Rim", "step": 4, "on": true, "velocity": 70}));
            steps.push(json!({"lane": "Rim", "step": 11, "on": true, "velocity": 70}));
        }
        if hats {
            for s in 1..=16 {
                let off = s % 4 == 3;
                let vel = if off { 96 } else { 54 };
                steps.push(json!({"lane": "Closed hat", "step": s, "on": true, "velocity": vel, "vary": 8}));
            }
            for s in [3, 7, 11, 15] {
                steps.push(
                    json!({"lane": "Open hat", "step": s, "on": true, "velocity": 88, "gate": 0.8}),
                );
            }
        }
        c(d, "edit_pattern", json!({"pattern": id, "steps": steps}));
        c(
            d,
            "edit_clip",
            json!({"clip": clip, "len_beats": len_beats}),
        );
        if !copies.is_empty() {
            c(d, "duplicate_clip", json!({"clip": clip, "bars": copies}));
        }
        clip
    };
    pattern(d, "Intro beat", 1, 64, &[97.0], false, true, true);
    pattern(d, "House beat", 17, 128, &[65.0], true, true, true);
    pattern(d, "Hats only", 49, 64, &[], false, true, false);

    // Bass: a rolling offbeat line, with the sub under it in the drops.
    let bass = track(d, "model-mono", "Bass");
    sound(d, &bass, "Round bass");
    let bass_line = json!([
        n(0.5, 0.4, 33, 112),
        n(1.5, 0.4, 33, 96),
        n(2.5, 0.4, 33, 112),
        n(3.5, 0.4, 45, 90),
        n(4.5, 0.4, 41, 112),
        n(5.5, 0.4, 41, 96),
        n(6.5, 0.4, 41, 112),
        n(7.5, 0.4, 43, 90),
        n(8.5, 0.4, 43, 112),
        n(9.5, 0.4, 43, 96),
        n(10.5, 0.4, 43, 112),
        n(11.5, 0.4, 40, 90),
        n(12.5, 0.4, 40, 112),
        n(13.5, 0.4, 40, 96),
        n(14.5, 0.4, 40, 112),
        n(15.25, 0.6, 43, 100)
    ]);
    let mut b = every(21, 45, 4);
    b.extend(every(65, 93, 4));
    part(d, &bass, "Bass line", 17, 4, bass_line, &b);

    let sub = track(d, "oh-one", "Sub");
    sound(d, &sub, "Sub bass");
    let sub_line = json!([
        n(0.0, 3.8, 33, 100),
        n(4.0, 3.8, 29, 100),
        n(8.0, 3.8, 31, 100),
        n(12.0, 3.8, 28, 100)
    ]);
    let mut s = every(37, 45, 4);
    s.extend(every(65, 93, 4));
    part(d, &sub, "Sub roots", 33, 4, sub_line, &s);

    // Chords: stabs on the offbeats, a pad for the quiet parts.
    let stabs = track(d, "june", "Stabs");
    sound(d, &stabs, "House stab");
    let stab_line = json!([
        chord(0.5, 0.3, "Am7", 4, 104),
        chord(2.75, 0.3, "Am7", 4, 86),
        chord(3.5, 0.3, "Am7", 4, 96),
        chord(4.5, 0.3, "Fmaj7", 4, 104),
        chord(6.75, 0.3, "Fmaj7", 4, 86),
        chord(7.5, 0.3, "Fmaj7", 4, 96),
        chord(8.5, 0.3, "G", 4, 104),
        chord(10.75, 0.3, "G", 4, 86),
        chord(11.5, 0.3, "G6", 4, 96),
        chord(12.5, 0.3, "Em7", 4, 104),
        chord(14.75, 0.3, "Em7", 4, 86),
        chord(15.5, 0.3, "Em7", 4, 96)
    ]);
    let mut st = every(21, 45, 4);
    st.extend(every(65, 93, 4));
    part(d, &stabs, "Stabs", 17, 4, stab_line, &st);

    let pad = track(d, "oracle", "Pad");
    sound(d, &pad, "Warm pad");
    let pad_line = json!([
        chord(0.0, 16.0, "Am9", 3, 72),
        chord(16.0, 8.0, "Fmaj7", 3, 72),
        chord(24.0, 4.0, "G", 3, 72),
        chord(28.0, 4.0, "Em7", 3, 72)
    ]);
    let mut pd = every(9, 9, 8);
    pd.extend(every(49, 57, 8));
    pd.extend(every(65, 89, 8));
    pd.extend(every(97, 105, 8));
    part(d, &pad, "Pad", 1, 8, pad_line, &pd);

    // Keys: a piano riff in the drops, strings in the breakdown.
    let keys = track(d, "lucky-seven", "Keys");
    sound(d, &keys, "Tine piano");
    let mut riff = Vec::new();
    for (i, ch) in ["Am7", "Fmaj7", "G", "Em7"].iter().enumerate() {
        let b0 = i as f64 * 4.0;
        riff.push(chord(b0 + 0.5, 0.5, ch, 4, 92));
        riff.push(chord(b0 + 1.5, 0.75, ch, 4, 80));
        riff.push(chord(b0 + 2.5, 0.25, ch, 4, 96));
        riff.push(chord(b0 + 3.0, 0.75, ch, 5, 74));
    }
    let mut k = every(37, 45, 4);
    k.extend(every(65, 93, 4));
    let keys_clip = part(d, &keys, "Piano riff", 33, 4, Value::Array(riff), &k);

    let strings = track(d, "eighty", "Strings");
    sound(d, &strings, "Strings");
    let string_line = json!([
        chord(0.0, 8.0, "Am", 4, 64),
        chord(8.0, 8.0, "F", 4, 68),
        chord(16.0, 8.0, "G", 4, 74),
        chord(24.0, 8.0, "E", 4, 80)
    ]);
    part(d, &strings, "Swell", 49, 8, string_line, &[57.0]);

    // Pluck: a sixteenth arpeggio.
    let pluck = track(d, "blue-wave", "Pluck");
    sound(d, &pluck, "Wave bells");
    let seq = [69, 72, 76, 79, 81, 79, 76, 72];
    let arp: Vec<Value> = (0..32)
        .map(|i| {
            n(
                i as f64 * 0.5,
                0.25,
                seq[i % 8] + if i >= 16 { -2 } else { 0 },
                64 + (i as i64 % 4) * 14,
            )
        })
        .collect();
    let mut a = every(45, 45, 4);
    a.extend(every(65, 93, 4));
    part(d, &pluck, "Arp", 41, 4, Value::Array(arp), &a);

    // Lead: the hook over the second drop.
    let lead = track(d, "patch-twenty", "Lead");
    let hook = json!([
        n(0.0, 0.75, 76, 110),
        n(1.0, 0.5, 79, 100),
        n(1.5, 1.5, 81, 112),
        n(3.5, 0.5, 79, 96),
        n(4.0, 0.75, 76, 110),
        n(5.0, 0.5, 74, 100),
        n(5.5, 2.0, 72, 108),
        n(8.0, 0.75, 71, 104),
        n(9.0, 0.5, 72, 100),
        n(9.5, 1.5, 74, 112),
        n(11.5, 0.5, 72, 96),
        n(12.0, 0.75, 71, 104),
        n(13.0, 0.5, 69, 100),
        n(13.5, 2.5, 64, 108)
    ]);
    part(d, &lead, "Hook", 81, 4, hook, &every(85, 93, 4));

    // Acid: a line with accents and slides through the build.
    let acid = track(d, "acid-three", "Acid");
    let acid_line = json!([
        n(0.0, 0.3, 45, 70),
        n(0.5, 0.3, 45, 120),
        n(1.0, 0.3, 57, 70),
        n(1.5, 0.6, 45, 70),
        n(2.0, 0.3, 48, 120),
        n(2.5, 0.3, 45, 70),
        n(3.0, 0.75, 43, 70),
        n(3.5, 0.3, 55, 120),
        n(4.0, 0.3, 45, 70),
        n(4.5, 0.3, 45, 120),
        n(5.0, 0.3, 57, 70),
        n(5.5, 0.6, 45, 70),
        n(6.0, 0.3, 52, 120),
        n(6.5, 0.3, 45, 70),
        n(7.0, 0.75, 40, 70),
        n(7.5, 0.3, 52, 120)
    ]);
    part(d, &acid, "Acid line", 25, 2, acid_line, &every(27, 31, 2));

    // The vocal: a sung phrase, its pitch found, and chops of it.
    write_vocal(vocal);
    let before = d.project()["tracks"].as_array().unwrap().len();
    c(
        d,
        "import_audio",
        json!({"path": vocal.display().to_string(), "bar": 33}),
    );
    let mut vocal_track = String::new();
    for _ in 0..4000 {
        d.h.step();
        let p = d.project();
        let ts = p["tracks"].as_array().unwrap();
        if ts.len() > before {
            vocal_track = ts.last().unwrap()["id"].as_str().unwrap().to_string();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!vocal_track.is_empty(), "the vocal imported");
    c(
        d,
        "edit_track",
        json!({"track": vocal_track, "name": "Vocal"}),
    );
    let p = d.project();
    let vt = (p["tracks"].as_array().unwrap().iter())
        .find(|t| t["id"] == vocal_track.as_str())
        .unwrap()
        .clone();
    let vocal_clip = vt["clips"][0]["id"]
        .as_str()
        .or(vt["clips"][0]["clip"].as_str())
        .unwrap()
        .to_string();
    c(
        d,
        "edit_clip",
        json!({"clip": vocal_clip, "name": "Phrase", "fade_in_seconds": 0.02, "fade_out_seconds": 0.2, "gain_db": -1.5}),
    );
    c(
        d,
        "edit_pitch_notes",
        json!({"clip": vocal_clip, "find": true}),
    );
    c(
        d,
        "duplicate_clip",
        json!({"clip": vocal_clip, "bars": [41.0, 65.0, 73.0, 81.0, 89.0]}),
    );

    // Buses, effects and sends.
    let drum_bus = track(d, "bus", "Drum bus");
    let music_bus = track(d, "bus", "Music bus");
    let vocal_bus = track(d, "bus", "Vocal bus");
    let reverb = track(d, "bus", "Reverb");
    let delay = track(d, "bus", "Delay");
    c(d, "edit_track", json!({"track": drums, "output": drum_bus}));
    for t in [
        &bass, &sub, &stabs, &pad, &keys, &strings, &pluck, &lead, &acid,
    ] {
        c(d, "edit_track", json!({"track": t, "output": music_bus}));
    }
    c(
        d,
        "edit_track",
        json!({"track": vocal_track, "output": vocal_bus}),
    );
    for (t, fx) in [
        (&bass, vec!["eq", "saturation", "compressor"]),
        (&sub, vec!["eq"]),
        (&stabs, vec!["eq", "chorus"]),
        (&pad, vec!["phaser", "eq"]),
        (&keys, vec!["tremolo", "eq"]),
        (&strings, vec!["eq"]),
        (&pluck, vec!["delay", "eq"]),
        (&lead, vec!["delay", "eq"]),
        (&acid, vec!["saturation", "filter"]),
        (&vocal_track, vec!["eq", "compressor", "de-esser"]),
        (&drum_bus, vec!["transient-shaper", "compressor", "tape"]),
        (&music_bus, vec!["compressor"]),
        (&vocal_bus, vec!["compressor"]),
        (&reverb, vec!["reverb"]),
        (&delay, vec!["delay"]),
    ] {
        for f in fx {
            c(d, "add_effect", json!({"track": t, "effect": f}));
        }
    }
    for f in ["eq", "compressor", "limiter"] {
        c(d, "add_effect", json!({"track": "master", "effect": f}));
    }
    for (t, to, g) in [
        (&vocal_track, &reverb, -10.0),
        (&vocal_track, &delay, -14.0),
        (&stabs, &reverb, -16.0),
        (&pad, &reverb, -8.0),
        (&keys, &reverb, -12.0),
        (&strings, &reverb, -6.0),
        (&pluck, &delay, -10.0),
        (&lead, &reverb, -12.0),
        (&lead, &delay, -9.0),
    ] {
        c(d, "set_send", json!({"track": t, "to": to, "gain_db": g}));
    }
    let mut ch = Vec::new();
    for (t, g, pan) in [
        (&drums, -6.0, 0.0),
        (&bass, -4.0, 0.0),
        (&sub, -7.0, 0.0),
        (&stabs, -5.0, -0.2),
        (&pad, -10.0, 0.15),
        (&keys, -6.0, 0.25),
        (&strings, -9.0, -0.1),
        (&pluck, -9.0, -0.35),
        (&lead, -5.0, 0.1),
        (&acid, -8.0, 0.3),
        (&vocal_track, -2.0, 0.0),
        (&drum_bus, -2.0, 0.0),
        (&music_bus, -3.0, 0.0),
        (&vocal_bus, -1.0, 0.0),
        (&reverb, -6.0, 0.0),
        (&delay, -9.0, 0.0),
    ] {
        ch.push(json!({"track": t, "param": "gain", "value": g}));
        if pan != 0.0 {
            ch.push(json!({"track": t, "param": "pan", "value": pan}));
        }
    }
    c(d, "set_parameters", json!({"changes": ch}));

    // Automation: the pad's filter opens through the breakdown, the
    // music bus dips for the drop, the acid's cutoff rises.
    if let Some(cutoff) = param_like(d, &pad, "cutoff") {
        c(
            d,
            "edit_automation",
            json!({"track": pad, "param": cutoff, "add_lane": true, "add": [
                {"bar": 49, "value": 300}, {"bar": 64.9, "value": 8000},
                {"bar": 65, "value": 4000}]}),
        );
    }
    if let Some(cutoff) = param_like(d, &acid, "cutoff") {
        c(
            d,
            "edit_automation",
            json!({"track": acid, "param": cutoff, "add_lane": true, "add": [
                {"bar": 25, "value": 200}, {"bar": 33, "value": 2500}]}),
        );
    }
    c(
        d,
        "edit_automation",
        json!({"track": music_bus, "param": "gain", "add_lane": true, "add": [
            {"bar": 1, "value": -3.0}, {"bar": 32, "value": -3.0}, {"bar": 32.9, "value": -12.0},
            {"bar": 33, "value": -3.0}, {"bar": 64, "value": -3.0}, {"bar": 64.9, "value": -12.0},
            {"bar": 65, "value": -3.0}]}),
    );

    // The loop over the start of the second drop, and comments.
    c(
        d,
        "set_loop",
        json!({"from_bar": 65, "to_bar": 73, "on": true}),
    );
    c(
        d,
        "add_comment",
        json!({"text": "Try a louder clap through the second drop.", "track": drums, "bar": 65, "to_bar": 97}),
    );
    c(
        d,
        "add_comment",
        json!({"text": "Double the vocal here and pan the copies.", "track": vocal_track, "bar": 81}),
    );
    c(d, "seek", json!({"bar": 65}));
    c(d, "select_clips", json!({"clips": []}));

    Demo {
        drums,
        bass,
        sub,
        stabs,
        pad,
        keys,
        strings,
        pluck,
        lead,
        acid,
        vocal: vocal_track,
        vocal_clip,
        keys_clip,
        drum_bus,
        music_bus,
        vocal_bus,
    }
}

/// A fresh folder for this process's scratch files.
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-demo-{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The repository's `examples/house-demo` folder.
pub(crate) fn demo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/house-demo")
}

/// Writes the demo project folder into the repository. The device copy
/// of the document and the backup are not kept: the folder is the
/// project as any computer opens it.
#[test]
#[ignore]
fn write_house_demo() {
    let dir = demo_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let tmp = scratch("write");
    let mut d = Driver::new();
    let demo = build(&mut d, &tmp.join("vocal-phrase.wav"));
    c(
        &mut d,
        "save_project",
        json!({"path": dir.display().to_string()}),
    );
    let _ = std::fs::remove_dir_all(dir.join(daw_project::DEVICES));
    let _ = std::fs::remove_file(dir.join("project.daw.bak"));
    assert!(dir.join("project.daw").exists());
    assert!(!demo.vocal.is_empty());
    // A listen: `DEMO_MP3=<folder>` writes the mix there as MP3.
    if let Ok(out) = std::env::var("DEMO_MP3") {
        let r = c(
            &mut d,
            "export_mix",
            json!({"format": "mp3", "folder": out}),
        );
        let mut done = Value::Null;
        for _ in 0..240 {
            c(
                &mut d,
                "wait_for_change",
                json!({"revision": r["revision"], "timeout_seconds": 25}),
            );
            done = c(&mut d, "get_exports", json!({}));
            if !done["finished"].as_array().is_none_or(|f| f.is_empty()) {
                break;
            }
        }
        println!("exported {done}");
    }
}

/// An assistant can build a whole layered song: every built-in synth,
/// a drum machine pattern, a sung phrase with its pitch found, buses,
/// sends, effects, automation, sections, a loop and comments, and read
/// it all back.
#[test]
fn an_assistant_builds_the_demo_song_and_reads_it_back() {
    let tmp = scratch("test");
    let mut d = Driver::new();
    let demo = build(&mut d, &tmp.join("vocal-phrase.wav"));
    let p = d.project();
    let tracks = p["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 16, "{}", tracks.len());
    let names: Vec<&str> = tracks.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"Vocal") && names.contains(&"Drum bus"),
        "{names:?}"
    );
    let drums = c(&mut d, "get_patterns", json!({"track": demo.drums}));
    assert!(drums.to_string().contains("House beat"), "{drums}");
    let keys = c(&mut d, "get_instrument", json!({"track": demo.keys}));
    assert_eq!(keys["sound"], "Tine piano", "{keys}");
    let auto = c(&mut d, "get_automation", json!({"track": demo.music_bus}));
    assert!(auto.to_string().contains("gain"), "{auto}");
    let comments = c(&mut d, "get_comments", json!({}));
    assert_eq!(
        comments["comments"].as_array().map(Vec::len),
        Some(2),
        "{comments}"
    );
    let transport = c(&mut d, "get_transport", json!({}));
    assert_eq!(transport["loop"], json!(true), "{transport}");
    // Every instrument track plays the sound the song asks for.
    for (t, want) in [
        (&demo.bass, "Round bass"),
        (&demo.sub, "Sub bass"),
        (&demo.stabs, "House stab"),
        (&demo.pad, "Warm pad"),
        (&demo.strings, "Strings"),
        (&demo.pluck, "Wave bells"),
    ] {
        let i = c(&mut d, "get_instrument", json!({"track": t}));
        assert_eq!(i["sound"], want, "{i}");
    }
    for t in [&demo.lead, &demo.acid, &demo.drum_bus, &demo.vocal_bus] {
        assert!(tracks.iter().any(|x| x["id"] == t.as_str()), "{t}");
    }
    let riff = c(&mut d, "get_note_clip", json!({"clip": demo.keys_clip}));
    assert!(
        riff["notes"].as_array().is_some_and(|n| !n.is_empty()),
        "{riff}"
    );
    let _ = demo.vocal_clip;
}
