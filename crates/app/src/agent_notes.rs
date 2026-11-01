// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for what the note editor, the note tools and the
//! step sequencer do by hand (roadmap 2.11): note clips (moving,
//! resizing, muting, duplicating, their key), notes with their chance
//! and groups, controller and expression lanes, quantise, humanise,
//! legato, fit to scale and grooves with their settings, and patterns.
//! Each is one undo step, as every edit by an assistant is (see
//! `agent.rs`); they edit the project as the document has it.
//!
//! Times in a clip are in beats (quarter notes): notes and controller
//! points from the clip's content start, expression points from the
//! note's start. A clip's place is a bar (from 1, fractions within it)
//! or seconds.

use std::collections::HashSet;

use daw_engine::time::{Frames, PPQ, TempoMap, Ticks};
use daw_project::noteops::{self, Humanise, Random};
use daw_project::{
    AutoPoint, Controller, Direction, Lane, Note, NoteClip, Pattern, PatternLane, Project, Step,
    StepRate,
};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::notes_editor::{Grid, quantise_tick};

type Done = Result<(Value, Option<String>), String>;

fn clip_props(mut props: Value) -> Value {
    props["bar"] =
        json!({"type": "number", "description": "A bar, from 1; 2.5 is halfway through bar 2."});
    props["seconds"] = json!({"type": "number"});
    props
}

fn picks() -> Value {
    json!({"type": "array", "items": {"type": "string"},
           "description": "The notes' ids; every note in the clip if left out."})
}

fn note_fields(mut props: Value) -> Value {
    for (k, v) in json!({
        "beat": {"type": "number", "description": "From the clip's start, in quarter notes."},
        "beats": {"type": "number", "description": "How long."},
        "pitch": {"type": "integer", "description": "A MIDI number; 60 is middle C."},
        "velocity": {"type": "integer", "description": "1 to 127."},
        "chance": {"type": "number", "description": "0 to 1: how likely it is to play each time round."},
        "channel": {"type": "integer", "description": "1 to 16."}
    })
    .as_object()
    .unwrap()
    {
        props[k] = v.clone();
    }
    props
}

/// Notes to add: one pitch, several, or a chord by name.
fn new_notes() -> Value {
    json!({"type": "array", "description": "Each needs a beat, beats and a pitch, pitches or a chord.",
    "items": {"type": "object", "properties": note_fields(json!({
        "pitches": {"type": "array", "items": {"type": "integer"},
                    "description": "Several notes at once, all alike."},
        "chord": {"type": "string", "description": "Such as Dm9, Cmaj7, A7b9, Em7b5, G/B or Fsus4: \
                  its notes, the root in the octave given."},
        "octave": {"type": "integer", "description": "For chord: 4 (the octave of middle C) if left out."}
    }))}})
}

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_note_clip",
        description: "Everything in a note clip: where it is (ticks, at 960 a quarter \
                      note, and bars), its offset, mute, own key, the pattern it plays, \
                      its notes (with chance, group and play-one-at-random), each note's \
                      expression lines, and its controller lanes. Times inside are in \
                      beats.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "clip": {"type": "string"}
            }, "required": ["clip"]})
        },
    },
    Tool {
        name: "duplicate_clip",
        description: "Copies an audio clip or a note clip, straight after it unless a bar \
                      or seconds is given, as many times as asked, back to back (a 4-bar \
                      riff 8 times fills 32 bars). A copy of a pattern clip plays the same \
                      pattern (linked: editing the pattern changes both) unless linked is \
                      false, when it gets a copy of the pattern of its own. Gives the new \
                      clip's id.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": clip_props(json!({
                "clip": {"type": "string"},
                "track": {"type": "string", "description": "Put the copy on this track."},
                "linked": {"type": "boolean", "description": "Pattern clips: true if left out."},
                "times": {"type": "integer", "description": "How many copies; 1 if left out."}
            })), "required": ["clip"]})
        },
    },
    Tool {
        name: "add_note_clip",
        description: "Adds a note clip to an instrument track, starting on a bar, some \
                      bars long, with the notes given (as edit_notes adds them, chords \
                      too). Gives its id.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "bar": {"type": "integer", "description": "From 1."},
                "bars": {"type": "integer", "description": "How long; 1 if left out."},
                "name": {"type": "string"},
                "notes": new_notes()
            }, "required": ["track", "bar"]})
        },
    },
    Tool {
        name: "edit_notes",
        description: "Adds notes to a note clip, changes or removes some by id, or clears \
                      it first; and puts notes in groups. Notes in a group share one roll \
                      of the dice for their chance, so they play together or not at all; \
                      with one_of, one note of the group is picked at random each time \
                      round. Notes are placed in beats (quarter notes) from the clip's \
                      start; pitch is a MIDI number (60 is middle C). get_note_clip gives \
                      their ids.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clip": {"type": "string"},
                "clear": {"type": "boolean"},
                "remove": {"type": "array", "items": {"type": "string"}},
                "add": new_notes(),
                "change": {"type": "array", "items": {"type": "object",
                    "properties": note_fields(json!({"id": {"type": "string"}})),
                    "required": ["id"]}},
                "group": {"type": "array", "description": "New groups, each of notes by id.",
                    "items": {"type": "object", "properties": {
                        "notes": {"type": "array", "items": {"type": "string"}},
                        "one_of": {"type": "boolean", "description": "Play one of them at random."}
                    }, "required": ["notes"]}},
                "ungroup": {"type": "array", "items": {"type": "string"},
                    "description": "Notes, by id, to take out of their groups."}
            }, "required": ["clip"]})
        },
    },
    Tool {
        name: "edit_note_lanes",
        description: "Draws a note clip's controller lanes (a MIDI controller such as \
                      cc:1 for the mod wheel or cc:64 for the sustain pedal, 0 to 127; \
                      bend, -8192 to 8191; pressure, 0 to 127), with points in beats from \
                      the clip's start; and a note's own expression (pitch in semitones, \
                      -48 to 48; pressure and timbre, 0 to 1), with points in beats from \
                      the note's start. Points given replace the line's points; an empty \
                      list, or remove, takes the line away.",
        access: Access::Edit,
        schema: || {
            let points = json!({"type": "array", "items": {"type": "object", "properties": {
                "beat": {"type": "number"}, "value": {"type": "number"}
            }, "required": ["beat", "value"]}});
            json!({"type": "object", "properties": {
                "clip": {"type": "string"},
                "controllers": {"type": "array", "items": {"type": "object", "properties": {
                    "controller": {"type": "string", "description": "cc:0 to cc:127, bend or pressure."},
                    "points": points,
                    "remove": {"type": "boolean"}
                }, "required": ["controller"]}},
                "expression": {"type": "array", "items": {"type": "object", "properties": {
                    "note": {"type": "string"},
                    "kind": {"type": "string", "enum": ["pitch", "pressure", "timbre"]},
                    "points": points,
                    "remove": {"type": "boolean"}
                }, "required": ["note", "kind"]}}
            }, "required": ["clip"]})
        },
    },
    Tool {
        name: "note_tool",
        description: "Runs a note tool on a note clip's notes (those given, or all), with \
                      its settings: quantise (grid, strength and swing in percent), \
                      humanise (up to how far in time, velocity and length), legato (the \
                      gap left, in beats; negative overlaps), fit_to_scale (the clip's \
                      key, else the song's, else the key given), groove (apply one, by id \
                      or name, with timing and accent in percent) or make_groove (a \
                      groove from the clip's notes, on a grid; gives its id).",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clip": {"type": "string"},
                "tool": {"type": "string", "enum": ["quantise", "humanise", "legato",
                         "fit_to_scale", "groove", "make_groove"]},
                "notes": picks(),
                "grid": {"type": "string", "enum": ["bar", "beat", "1/8", "1/16", "1/32",
                         "1/8t", "1/16t"], "description": "Quantise and make_groove; 1/16 if left out."},
                "strength": {"type": "number", "description": "Quantise, percent; 100 if left out."},
                "swing": {"type": "number", "description": "Quantise, percent; 0 if left out."},
                "timing_beats": {"type": "number", "description": "Humanise: up to this far early or late; 0.02 if left out."},
                "velocity": {"type": "integer", "description": "Humanise: up to this much softer or harder; 8 if left out."},
                "length_pct": {"type": "number", "description": "Humanise: up to this much shorter or longer; 10 if left out."},
                "seed": {"type": "integer", "description": "Humanise: the same seed moves notes the same way."},
                "gap_beats": {"type": "number", "description": "Legato; 0 if left out."},
                "key": {"type": "string", "description": "fit_to_scale: such as \"A minor\"."},
                "groove": {"type": "string", "description": "groove: an id or name from list_grooves."},
                "timing": {"type": "number", "description": "groove: percent; 100 if left out."},
                "accent": {"type": "number", "description": "groove: percent; 50 if left out."},
                "name": {"type": "string", "description": "make_groove: its name."}
            }, "required": ["clip", "tool"]})
        },
    },
    Tool {
        name: "list_grooves",
        description: "The project's grooves, by id and name, with their grid step in \
                      beats and how many steps they have.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "get_patterns",
        description: "The step-sequencer patterns: each with its name, bars, swing, the \
                      clips playing it, and its lanes (the note or drum pad each plays, \
                      rate, direction, mute) with every step (on, velocity, vary, pitch, \
                      chance, repeats, offset, gate).",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "pattern": {"type": "string", "description": "Just this one."}
            }})
        },
    },
    Tool {
        name: "edit_pattern",
        description: "Makes or changes a step-sequencer pattern. new makes one (a plain \
                      drum beat, or empty with empty true); track and bar then add a clip \
                      playing it. Changes its name, bars and swing (0 to 1); adds, \
                      changes, moves or removes lanes (a lane plays a MIDI note, or a \
                      drum machine's pad by name; drum machine pads start at note 36); \
                      fills a lane with a Euclidean rhythm; and sets steps, counted from \
                      1, in a lane given by id or name. Step offset is -0.5 to 0.5 of a \
                      step, gate 0.05 to 4, chance 0 to 1, repeats 1 to 8. delete removes \
                      a pattern no clip plays. Gives the pattern's id.",
        access: Access::Edit,
        schema: || {
            let lane = json!({
                "name": {"type": "string"},
                "pitch": {"type": "integer"},
                "pad": {"type": "string", "description": "A drum machine pad's name."},
                "steps": {"type": "integer", "description": "How many steps, 1 to 64."},
                "rate": {"type": "string", "enum": ["1/4", "1/8", "1/8t", "1/16", "1/16t", "1/32"]},
                "direction": {"type": "string", "enum": ["forward", "backward", "pingpong", "random"]},
                "muted": {"type": "boolean"}
            });
            let mut change = lane.clone();
            change["lane"] = json!({"type": "string", "description": "Its id or name."});
            change["index"] =
                json!({"type": "integer", "description": "Move it here, from 0 at the top."});
            change["euclid"] = json!({"type": "object", "properties": {
                "hits": {"type": "integer"}, "rotate": {"type": "integer"}
            }, "required": ["hits"]});
            json!({"type": "object", "properties": {
                "pattern": {"type": "string", "description": "Its id; left out with new."},
                "new": {"type": "boolean"},
                "empty": {"type": "boolean", "description": "With new: no lanes to start."},
                "track": {"type": "string", "description": "With bar: add a clip playing it here."},
                "bar": {"type": "integer"},
                "name": {"type": "string"},
                "bars": {"type": "number"},
                "swing": {"type": "number"},
                "add_lanes": {"type": "array", "items": {"type": "object", "properties": lane}},
                "lanes": {"type": "array", "items": {"type": "object", "properties": change,
                          "required": ["lane"]}},
                "remove_lanes": {"type": "array", "items": {"type": "string"}},
                "steps": {"type": "array", "items": {"type": "object", "properties": {
                    "lane": {"type": "string"},
                    "step": {"type": "integer", "description": "From 1."},
                    "on": {"type": "boolean"},
                    "velocity": {"type": "integer"},
                    "vary": {"type": "integer"},
                    "pitch": {"type": "integer", "description": "Semitones from the lane's note."},
                    "chance": {"type": "number"},
                    "repeats": {"type": "integer"},
                    "offset": {"type": "number"},
                    "gate": {"type": "number"}
                }, "required": ["lane", "step"]}},
                "delete": {"type": "boolean"}
            }})
        },
    },
];

fn text<'a>(args: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("Give {what}."))
}

fn ids(v: &Value) -> Vec<String> {
    (v.as_array().into_iter().flatten())
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}

fn beats(b: f64) -> i64 {
    (b * PPQ as f64).round() as i64
}

fn in_beats(t: i64) -> f64 {
    t as f64 / PPQ as f64
}

/// The tick of a bar, from 1, fractions within it.
fn bar_ticks(map: &TempoMap, bar: f64) -> i64 {
    let bar = bar.max(1.0);
    let whole = bar.floor() as i64;
    let from = map.bar_start(whole).0;
    let len = map.bar_start(whole + 1).0 - from;
    from + ((bar - bar.floor()) * len as f64).round() as i64
}

/// The bar (from 1, with the fraction) a tick is in.
fn bar_of(map: &TempoMap, t: i64) -> f64 {
    let bar = map.bar_beat(Ticks(t)).bar;
    let from = map.bar_start(bar).0;
    let len = (map.bar_start(bar + 1).0 - from).max(1);
    bar as f64 + (t - from) as f64 / len as f64
}

/// Where note clip `id` is: its track and place on it.
fn note_clip_at(p: &Project, id: &str) -> Result<(usize, usize), String> {
    for (i, t) in p.tracks.iter().enumerate() {
        if let Some(j) = t.note_clips.iter().position(|c| c.id == id) {
            return Ok((i, j));
        }
        if t.clips.iter().any(|c| c.id == id) {
            return Err(format!("{id} is an audio clip, which has no notes."));
        }
    }
    Err(format!("There is no note clip {id}."))
}

fn grid_of(s: Option<&str>) -> Result<Grid, String> {
    Ok(match s.unwrap_or("1/16") {
        "bar" => Grid::Bar,
        "beat" => Grid::Beat,
        "1/8" => Grid::Eighth,
        "1/16" => Grid::Sixteenth,
        "1/32" => Grid::ThirtySecond,
        "1/8t" => Grid::EighthTriplet,
        "1/16t" => Grid::SixteenthTriplet,
        other => {
            return Err(format!(
                "{other} isn't a grid: bar, beat, 1/8, 1/16, 1/32, 1/8t or 1/16t."
            ));
        }
    })
}

fn rate_of(s: &str) -> Result<StepRate, String> {
    let name = s.trim_start_matches("1/");
    StepRate::from_name(name)
        .ok_or_else(|| format!("{s} isn't a step rate: 1/4, 1/8, 1/8t, 1/16, 1/16t or 1/32."))
}

fn points_of(v: &Value, lo: f64, hi: f64) -> Result<Vec<AutoPoint>, String> {
    let mut out = Vec::new();
    for p in v.as_array().into_iter().flatten() {
        let (Some(b), Some(value)) = (p["beat"].as_f64(), p["value"].as_f64()) else {
            return Err("Each point needs a beat and a value.".into());
        };
        out.push(AutoPoint {
            at: beats(b.max(0.0)),
            value: value.clamp(lo, hi),
        });
    }
    out.sort_by_key(|p| p.at);
    Ok(out)
}

fn lane_value(l: &Lane, per: f64) -> Value {
    json!({
        "points": l.points.iter().map(|p| json!({"beat": p.at as f64 / per, "value": p.value}))
            .collect::<Vec<_>>(),
    })
}

fn note_value(n: &Note) -> Value {
    json!({
        "id": n.id,
        "beat": in_beats(n.start),
        "beats": in_beats(n.len),
        "pitch": n.pitch,
        "velocity": n.velocity,
        "channel": n.channel + 1,
        "chance": n.chance,
        "group": n.group,
        "one_of": n.one_of,
        "expression": n.expression.iter().filter(|l| !l.points.is_empty()).map(|l| {
            let mut v = lane_value(l, PPQ as f64);
            v["kind"] = json!(l.param);
            v
        }).collect::<Vec<_>>(),
    })
}

/// Changes note `n` by what `a` gives.
fn change_note(n: &mut Note, a: &Value) -> Result<(), String> {
    if let Some(b) = a["beat"].as_f64() {
        n.start = beats(b.max(0.0));
    }
    if let Some(b) = a["beats"].as_f64() {
        n.len = beats(b).max(1);
    }
    if let Some(p) = a["pitch"].as_u64() {
        if p > 127 {
            return Err(format!("Pitch {p} is past 127."));
        }
        n.pitch = p as u8;
    }
    if let Some(v) = a["velocity"].as_u64() {
        n.velocity = v.clamp(1, 127) as u8;
    }
    if let Some(c) = a["chance"].as_f64() {
        n.chance = c.clamp(0.0, 1.0);
    }
    if let Some(c) = a["channel"].as_u64() {
        n.channel = (c.clamp(1, 16) - 1) as u8;
    }
    Ok(())
}

fn step_value(s: &Step) -> Value {
    json!({"on": s.on, "velocity": s.velocity, "vary": s.vary, "pitch": s.pitch,
           "chance": s.chance, "repeats": s.repeats, "offset": s.offset, "gate": s.gate})
}

fn set_step(s: &mut Step, a: &Value) {
    if let Some(b) = a["on"].as_bool() {
        s.on = b;
    }
    if let Some(v) = a["velocity"].as_i64() {
        s.velocity = v.clamp(1, 127) as u8;
    }
    if let Some(v) = a["vary"].as_i64() {
        s.vary = v.clamp(0, 127) as u8;
    }
    if let Some(v) = a["pitch"].as_i64() {
        s.pitch = v.clamp(-48, 48) as i8;
    }
    if let Some(v) = a["chance"].as_f64() {
        s.chance = v.clamp(0.0, 1.0);
    }
    if let Some(v) = a["repeats"].as_i64() {
        s.repeats = v.clamp(1, daw_project::MAX_REPEATS as i64) as u8;
    }
    if let Some(v) = a["offset"].as_f64() {
        s.offset = v.clamp(-0.5, 0.5);
    }
    if let Some(v) = a["gate"].as_f64() {
        s.gate = v.clamp(daw_project::MIN_GATE, daw_project::MAX_GATE);
    }
}

fn find_lane(p: &Pattern, key: &str) -> Result<usize, String> {
    (p.lanes.iter())
        .position(|l| l.id == key)
        .or_else(|| (p.lanes.iter()).position(|l| l.name.eq_ignore_ascii_case(key)))
        .ok_or_else(|| format!("The pattern has no lane {key}."))
}

/// The notes a list of new notes gives: each a pitch, pitches or a
/// chord.
fn notes_of(v: &Value) -> Result<Vec<Note>, String> {
    let mut out = Vec::new();
    for n in v.as_array().into_iter().flatten() {
        let (Some(beat), Some(len)) = (n["beat"].as_f64(), n["beats"].as_f64()) else {
            return Err("Each note needs a beat and beats (how long).".into());
        };
        let pitches: Vec<i64> = if let Some(c) = n["chord"].as_str() {
            let octave = n["octave"].as_i64().unwrap_or(4);
            chord(c)?.iter().map(|p| p + 12 * (octave + 1)).collect()
        } else if let Some(ps) = n["pitches"].as_array() {
            ps.iter()
                .map(|p| p.as_i64().ok_or("Pitches are whole numbers."))
                .collect::<Result<_, _>>()?
        } else {
            vec![
                n["pitch"]
                    .as_i64()
                    .ok_or("Each note needs a pitch, pitches or a chord.")?,
            ]
        };
        for p in pitches {
            if !(0..=127).contains(&p) {
                return Err(format!("Pitch {p} is outside 0 to 127."));
            }
            let mut note = Note::new(beats(beat.max(0.0)), beats(len).max(1), p as u8, 100);
            let mut rest = n.clone();
            rest["pitch"] = Value::Null;
            change_note(&mut note, &rest)?;
            out.push(note);
        }
    }
    Ok(out)
}

/// A chord's notes as semitones from C of octave -1 plus the root, for
/// a name such as "Dm9", "Bbmaj7", "A7b9" or "G/B" (the bass below).
fn chord(name: &str) -> Result<Vec<i64>, String> {
    let bad = || format!("{name} isn't a chord this knows, such as Dm9, Cmaj7, A7b9 or G/B.");
    let (body, bass) = match name.trim().split_once('/') {
        Some((b, s)) => (b, Some(s)),
        None => (name.trim(), None),
    };
    let note = |s: &str| -> Option<(i64, usize)> {
        let mut c = s.chars();
        let base = match c.next()?.to_ascii_uppercase() {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };
        match c.next() {
            Some('#') => Some((base + 1, 2)),
            Some('b') if s.len() > 1 => Some((base - 1, 2)),
            _ => Some((base, 1)),
        }
    };
    let (root, used) = note(body).ok_or_else(bad)?;
    let quality = &body[used..];
    let steps: &[i64] = match quality {
        "" | "maj" | "M" => &[0, 4, 7],
        "m" | "min" | "-" => &[0, 3, 7],
        "dim" | "o" => &[0, 3, 6],
        "aug" | "+" => &[0, 4, 8],
        "5" => &[0, 7],
        "sus2" => &[0, 2, 7],
        "sus4" | "sus" => &[0, 5, 7],
        "6" => &[0, 4, 7, 9],
        "m6" => &[0, 3, 7, 9],
        "7" => &[0, 4, 7, 10],
        "maj7" | "M7" => &[0, 4, 7, 11],
        "m7" | "min7" | "-7" => &[0, 3, 7, 10],
        "mmaj7" => &[0, 3, 7, 11],
        "m7b5" | "ø" => &[0, 3, 6, 10],
        "dim7" | "o7" => &[0, 3, 6, 9],
        "7sus4" => &[0, 5, 7, 10],
        "add9" => &[0, 4, 7, 14],
        "madd9" => &[0, 3, 7, 14],
        "9" => &[0, 4, 7, 10, 14],
        "maj9" => &[0, 4, 7, 11, 14],
        "m9" => &[0, 3, 7, 10, 14],
        "7b9" => &[0, 4, 7, 10, 13],
        "7#9" => &[0, 4, 7, 10, 15],
        "11" => &[0, 7, 10, 14, 17],
        "m11" => &[0, 3, 7, 10, 14, 17],
        "13" => &[0, 4, 10, 14, 21],
        "maj13" => &[0, 4, 11, 14, 21],
        "m13" => &[0, 3, 10, 14, 21],
        _ => return Err(bad()),
    };
    let mut out: Vec<i64> = steps.iter().map(|s| root + s).collect();
    if let Some(b) = bass {
        let (b, used) = note(b).ok_or_else(bad)?;
        if used != bass.map_or(0, str::len) {
            return Err(bad());
        }
        // The bass goes below the root.
        let below = (root - b).rem_euclid(12);
        out.insert(0, root - if below == 0 { 12 } else { below });
    }
    Ok(out)
}

impl App {
    /// Runs one of the tools here, or `None` if `name` isn't one.
    pub(crate) fn notes_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        Some(match name {
            "duplicate_clip" => self.duplicate_clip_tool(args),
            "add_note_clip" => self.add_note_clip_tool(args),
            "edit_notes" => self.edit_notes_tool(args),
            "edit_note_lanes" => self.edit_note_lanes_tool(args),
            "note_tool" => self.note_tool(args),
            "edit_pattern" => self.edit_pattern_tool(args),
            _ => return None,
        })
    }

    /// Runs one of the reading tools here, or `None` if `name` isn't one.
    pub(crate) fn notes_read_tool(
        &self,
        name: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        Some(match name {
            "get_note_clip" => self.get_note_clip(args),
            "list_grooves" => Ok(self.grooves_value()),
            "get_patterns" => self.patterns_value(args),
            _ => return None,
        })
    }

    /// A tick on the timeline from a bar or seconds in `args`.
    fn tick_arg(&self, args: &Value) -> Option<i64> {
        let map = self.tempo_map();
        match (args["bar"].as_f64(), args["seconds"].as_f64()) {
            (Some(bar), _) => Some(bar_ticks(&map, bar)),
            (None, Some(s)) => {
                let f = (s.max(0.0) * self.rate as f64).round() as i64;
                Some(map.ticks_at(Frames(f)).0)
            }
            (None, None) => None,
        }
    }

    fn get_note_clip(&self, args: &Value) -> Result<Value, String> {
        let id = text(args, "clip", "the clip's id")?;
        let p = self.project();
        let (i, j) = note_clip_at(&p, id)?;
        let c = &p.tracks[i].note_clips[j];
        let map = self.tempo_map();
        Ok(json!({
            "clip": c.id,
            "track": p.tracks[i].id,
            "name": c.name,
            "start_ticks": c.start,
            "len_ticks": c.len,
            "bar": bar_of(&map, c.start),
            "end_bar": bar_of(&map, c.end()),
            "offset_beats": in_beats(c.offset),
            "muted": c.muted,
            "key": c.scale.map(|s| s.label()),
            "pattern": c.pattern,
            "notes": c.notes.iter().map(note_value).collect::<Vec<_>>(),
            "controllers": c.controllers.iter().filter(|l| !l.points.is_empty()).map(|l| {
                let mut v = lane_value(l, PPQ as f64);
                v["controller"] = json!(l.param);
                v
            }).collect::<Vec<_>>(),
        }))
    }

    /// edit_clip for a note clip: what `edit_clip` hands on.
    pub(crate) fn edit_note_clip(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        let start = self.tick_arg(args);
        let key = match args["key"].as_str() {
            Some(k) => Some(crate::agent_arrange::parse_key(k)?),
            None => None,
        };
        let map = self.tempo_map();
        self.edit_model(|p| {
            let (mut i, j) = note_clip_at(p, &id)?;
            if let Some(to) = args["track"].as_str() {
                let ti = (p.tracks.iter())
                    .position(|t| t.id == to)
                    .ok_or_else(|| format!("There is no track {to}."))?;
                if !p.tracks[ti].kind.plays_notes() {
                    return Err(format!("{} isn't an instrument track.", p.tracks[ti].name));
                }
                if ti != i {
                    let c = p.tracks[i].note_clips.remove(j);
                    p.tracks[ti].note_clips.push(c);
                    i = ti;
                }
            }
            if let Some(pat) = args["pattern"].as_str()
                && !p.patterns.iter().any(|x| x.id == pat)
            {
                return Err(format!("There is no pattern {pat}."));
            }
            let (patterns, tracks) = (&mut p.patterns, &mut p.tracks);
            let clips = &mut tracks[i].note_clips;
            let c = (clips.iter_mut())
                .find(|c| c.id == id)
                .expect("still there");
            if let Some(s) = start {
                c.start = s;
            }
            if let Some(b) = args["len_beats"].as_f64() {
                c.len = beats(b).max(1);
            }
            if let Some(b) = args["offset_beats"].as_f64() {
                c.offset = beats(b.max(0.0));
            }
            if let Some(m) = args["muted"].as_bool() {
                c.muted = m;
            }
            if let Some(n) = args["name"].as_str() {
                c.name = n.into();
            }
            if let Some(k) = key {
                c.scale = k;
            }
            if let Some(pat) = args["pattern"].as_str() {
                c.pattern = Some(pat.into());
                daw_project::fill_pattern_clip(patterns, c);
            }
            if args["pattern_unique"].as_bool() == Some(true) {
                let src = (c.pattern.as_ref())
                    .and_then(|id| patterns.iter().find(|x| &x.id == id))
                    .ok_or("The clip doesn't play a pattern.")?;
                let copy = src.copy(format!("{} copy", src.name));
                c.pattern = Some(copy.id.clone());
                patterns.push(copy);
            }
            if args["pattern_to_notes"].as_bool() == Some(true) {
                if c.pattern.is_none() {
                    return Err("The clip doesn't play a pattern.".into());
                }
                daw_project::fill_pattern_clip(patterns, c);
                c.pattern = None;
                for n in &mut c.notes {
                    n.id = daw_project::new_id();
                }
            }
            let v = json!({"clip": id, "bar": bar_of(&map, c.start),
                           "len_beats": in_beats(c.len), "pattern": c.pattern});
            clips.sort_by_key(|c| c.start);
            Ok(v)
        })
        .map(|v| (v, Some("Change a note clip".into())))
    }

    fn duplicate_clip_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        let at_tick = self.tick_arg(args);
        let at_frame = at_tick.map(|t| self.tempo_map().frames_at(Ticks(t)).0);
        let linked = args["linked"].as_bool().unwrap_or(true);
        let to = args["track"].as_str().map(String::from);
        let times = args["times"].as_i64().unwrap_or(1).clamp(1, 256);
        self.edit_model(|p| {
            let dest = |p: &Project, i: usize, audio: bool| -> Result<usize, String> {
                let Some(to) = &to else { return Ok(i) };
                let ti = (p.tracks.iter())
                    .position(|t| &t.id == to)
                    .ok_or_else(|| format!("There is no track {to}."))?;
                let t = &p.tracks[ti];
                let ok = if audio {
                    !t.kind.is_bus() && !t.kind.plays_notes()
                } else {
                    t.kind.plays_notes()
                };
                if !ok {
                    return Err(format!("{} can't take that clip.", t.name));
                }
                Ok(ti)
            };
            for i in 0..p.tracks.len() {
                if let Some(src) = p.tracks[i].clips.iter().find(|c| c.id == id).cloned() {
                    let ti = dest(p, i, true)?;
                    let first = at_frame.unwrap_or(src.start + src.len);
                    let mut new = Vec::new();
                    for k in 0..times {
                        let mut c = src.clone();
                        c.id = daw_project::new_id();
                        c.start = first + k * src.len;
                        new.push(c.id.clone());
                        p.tracks[ti].clips.push(c);
                    }
                    return Ok(json!({"clip": new[0], "clips": new}));
                }
                if let Some(src) = p.tracks[i].note_clips.iter().find(|c| c.id == id).cloned() {
                    let ti = dest(p, i, false)?;
                    let first = at_tick.unwrap_or(src.end());
                    let mut new = Vec::new();
                    let mut pattern = src.pattern.clone();
                    if !linked
                        && let Some(pat) = (src.pattern.as_ref())
                            .and_then(|id| p.patterns.iter().find(|x| &x.id == id))
                    {
                        // One copy of the pattern, shared by the new clips.
                        let copy = pat.copy(format!("{} copy", pat.name));
                        pattern = Some(copy.id.clone());
                        p.patterns.push(copy);
                    }
                    for k in 0..times {
                        let mut c = src.clone();
                        c.id = daw_project::new_id();
                        c.start = first + k * src.len;
                        c.pattern = pattern.clone();
                        for n in &mut c.notes {
                            n.id = daw_project::new_id();
                        }
                        new.push(c.id.clone());
                        p.tracks[ti].note_clips.push(c);
                    }
                    p.tracks[ti].note_clips.sort_by_key(|c| c.start);
                    return Ok(json!({"clip": new[0], "clips": new, "pattern": pattern}));
                }
            }
            Err(format!("There is no clip {id}."))
        })
        .map(|v| (v, Some("Duplicate a clip".into())))
    }

    fn add_note_clip_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let bar = args["bar"]
            .as_i64()
            .ok_or("Give the bar it starts on.")?
            .max(1);
        let bars = args["bars"].as_i64().unwrap_or(1).max(1);
        let map = self.tempo_map();
        let (start, end) = (map.bar_start(bar).0, map.bar_start(bar + bars).0);
        let name = args["name"].as_str().map(String::from);
        let notes = notes_of(&args["notes"])?;
        self.edit_model(|p| {
            let t = (p.tracks.iter_mut())
                .find(|t| t.id == key)
                .ok_or_else(|| format!("There is no track {key}."))?;
            if !t.kind.plays_notes() {
                return Err(format!("{} isn't an instrument track.", t.name));
            }
            let id = daw_project::new_id();
            let n = t.note_clips.len() + 1;
            let name = name.unwrap_or_else(|| format!("{} {n}", t.name));
            let mut c = NoteClip::new(id.clone(), name, start, end - start);
            c.notes = notes;
            c.sort_notes();
            let count = c.notes.len();
            t.note_clips.push(c);
            t.note_clips.sort_by_key(|c| c.start);
            Ok(
                json!({"clip": id, "start_ticks": start, "len_ticks": end - start,
                      "notes": count}),
            )
        })
        .map(|v| (v, Some("Add a note clip".into())))
    }

    fn edit_notes_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        let add = notes_of(&args["add"])?;
        let remove = ids(&args["remove"]);
        let ungroup: HashSet<String> = ids(&args["ungroup"]).into_iter().collect();
        let clear = args["clear"].as_bool() == Some(true);
        self.edit_model(|p| {
            let (i, j) = note_clip_at(p, &id)?;
            let c = &mut p.tracks[i].note_clips[j];
            if c.pattern.is_some() {
                return Err(
                    "The clip plays a pattern; its notes come from the pattern (edit_pattern), \
                     or edit_clip with pattern_to_notes makes them its own."
                        .into(),
                );
            }
            if clear {
                c.notes.clear();
            }
            let before = c.notes.len();
            c.notes.retain(|n| !remove.contains(&n.id));
            let removed = before - c.notes.len();
            let added: Vec<String> = add.iter().map(|n| n.id.clone()).collect();
            c.notes.extend(add);
            for ch in args["change"].as_array().into_iter().flatten() {
                let nid = ch["id"].as_str().ok_or("Each change needs a note's id.")?;
                let n = (c.notes.iter_mut())
                    .find(|n| n.id == nid)
                    .ok_or_else(|| format!("There is no note {nid}."))?;
                change_note(n, ch)?;
            }
            for n in c.notes.iter_mut().filter(|n| ungroup.contains(&n.id)) {
                n.group = None;
                n.one_of = false;
            }
            let mut groups = Vec::new();
            for g in args["group"].as_array().into_iter().flatten() {
                let members = ids(&g["notes"]);
                if let Some(m) = members
                    .iter()
                    .find(|m| !c.notes.iter().any(|n| &n.id == *m))
                {
                    return Err(format!("There is no note {m}."));
                }
                let k = daw_project::next_group(&c.notes);
                let one_of = g["one_of"].as_bool() == Some(true);
                for n in c.notes.iter_mut().filter(|n| members.contains(&n.id)) {
                    n.group = Some(k);
                    n.one_of = one_of;
                }
                groups.push(k);
            }
            c.sort_notes();
            Ok(json!({"added": added, "removed": removed, "groups": groups,
                      "notes": c.notes.len()}))
        })
        .map(|v| (v, Some("Edit notes".into())))
    }

    fn edit_note_lanes_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        self.edit_model(|p| {
            let (i, j) = note_clip_at(p, &id)?;
            let c = &mut p.tracks[i].note_clips[j];
            for l in args["controllers"].as_array().into_iter().flatten() {
                let name = l["controller"]
                    .as_str()
                    .ok_or("Each lane needs a controller.")?;
                let ctl = Controller::from_param(name).ok_or_else(|| {
                    format!("{name} isn't a controller: cc:0 to cc:127, bend or pressure.")
                })?;
                let (lo, hi, _) = ctl.range();
                let points = points_of(&l["points"], lo, hi)?;
                if l["remove"].as_bool() == Some(true) || points.is_empty() {
                    c.controllers.retain(|x| x.param != ctl.param());
                } else {
                    c.controller_mut(ctl).points = points;
                }
            }
            for e in args["expression"].as_array().into_iter().flatten() {
                let nid = e["note"]
                    .as_str()
                    .ok_or("Each expression needs a note's id.")?;
                let kind = e["kind"].as_str().ok_or("Say which expression.")?;
                let kind = *Note::EXPRESSIONS
                    .iter()
                    .find(|k| **k == kind)
                    .ok_or_else(|| {
                        format!("{kind} isn't an expression: pitch, pressure or timbre.")
                    })?;
                let (lo, hi) = if kind == Note::PITCH {
                    (-48.0, 48.0)
                } else {
                    (0.0, 1.0)
                };
                let points = points_of(&e["points"], lo, hi)?;
                let n = (c.notes.iter_mut())
                    .find(|n| n.id == nid)
                    .ok_or_else(|| format!("There is no note {nid}."))?;
                n.expression.retain(|l| l.param != kind);
                if e["remove"].as_bool() != Some(true) && !points.is_empty() {
                    n.expression.push(Lane {
                        param: kind.into(),
                        points,
                    });
                }
            }
            Ok(json!({
                "controllers": c.controllers.iter().map(|l| &l.param).collect::<Vec<_>>(),
            }))
        })
        .map(|v| (v, Some("Draw note lanes".into())))
    }

    fn note_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        let tool = text(args, "tool", "the tool")?.to_string();
        let picked: HashSet<String> = ids(&args["notes"]).into_iter().collect();
        let map = self.tempo_map();
        let song_key = self.scale;
        let key = match args["key"].as_str() {
            Some(k) => crate::agent_arrange::parse_key(k)?,
            None => None,
        };
        let grid = grid_of(args["grid"].as_str())?;
        let seed = (args["seed"].as_u64()).unwrap_or_else(daw_project::new_peer);
        let bar_start = |t: i64| map.bar_start(map.bar_beat(Ticks(t)).bar).0;
        let mut label = "Use a note tool";
        let v = self.edit_model(|p| {
            let (i, j) = note_clip_at(p, &id)?;
            let grooves = &mut p.grooves;
            let c = &mut p.tracks[i].note_clips[j];
            if c.pattern.is_some() {
                return Err("The clip plays a pattern; its notes come from the pattern.".into());
            }
            if let Some(m) = picked.iter().find(|m| !c.notes.iter().any(|n| &n.id == *m)) {
                return Err(format!("There is no note {m}."));
            }
            let pick = |n: &Note| picked.is_empty() || picked.contains(&n.id);
            let origin = c.start - c.offset;
            let mut out = json!({});
            let (changed, name) = match tool.as_str() {
                "quantise" => {
                    let strength = args["strength"].as_f64().unwrap_or(100.0) / 100.0;
                    let swing = args["swing"].as_f64().unwrap_or(0.0) / 100.0;
                    let mut changed = false;
                    for n in c.notes.iter_mut().filter(|n| pick(n)) {
                        let at =
                            quantise_tick(&map, grid, origin + n.start, strength, swing) - origin;
                        if at != n.start && at >= 0 {
                            n.start = at;
                            changed = true;
                        }
                    }
                    (changed, "Quantise notes")
                }
                "humanise" => {
                    let d = Humanise::default();
                    let h = Humanise {
                        timing: args["timing_beats"]
                            .as_f64()
                            .map_or(d.timing, |b| beats(b.abs())),
                        velocity: (args["velocity"].as_u64())
                            .map_or(d.velocity, |v| v.min(127) as u8),
                        length_pct: (args["length_pct"].as_f64())
                            .map_or(d.length_pct, |v| v.clamp(0.0, 100.0)),
                    };
                    let mut rng = Random::new(seed);
                    (
                        noteops::humanise(&mut c.notes, &pick, h, &mut rng),
                        "Humanise notes",
                    )
                }
                "legato" => {
                    let gap = beats(args["gap_beats"].as_f64().unwrap_or(0.0));
                    (noteops::legato(&mut c.notes, &pick, gap), "Legato notes")
                }
                "fit_to_scale" => {
                    let s = c.scale.or(song_key).or(key).ok_or(
                        "Neither the clip nor the song has a key; give one, such as \"A minor\".",
                    )?;
                    out["key"] = json!(s.label());
                    (
                        noteops::fit_to_scale(&mut c.notes, &pick, s),
                        "Fit notes to the scale",
                    )
                }
                "groove" => {
                    let want = text(args, "groove", "the groove's id or name")?;
                    let g = (grooves.iter())
                        .find(|g| g.id == want)
                        .or_else(|| grooves.iter().find(|g| g.name.eq_ignore_ascii_case(want)))
                        .ok_or_else(|| {
                            format!("There is no groove {want}; list_grooves gives them.")
                        })?;
                    let timing = args["timing"].as_f64().unwrap_or(100.0) / 100.0;
                    let accent = args["accent"].as_f64().unwrap_or(50.0) / 100.0;
                    let changed = noteops::apply_groove(
                        &mut c.notes,
                        &pick,
                        g,
                        origin,
                        &bar_start,
                        timing,
                        accent,
                    );
                    (changed, "Apply a groove")
                }
                "make_groove" => {
                    let hits: Vec<(i64, f64)> = (c.notes.iter())
                        .filter(|n| pick(n))
                        .map(|n| (origin + n.start, n.velocity as f64))
                        .collect();
                    if hits.is_empty() {
                        return Err("There are no notes to take a groove from.".into());
                    }
                    let step = grid.ticks(&map, c.start).max(1);
                    let bar = map.signature_at(Ticks(c.start)).bar_ticks();
                    let steps = noteops::groove_steps(
                        &hits,
                        &bar_start,
                        step,
                        (bar / step).max(1) as usize,
                    );
                    let gid = daw_project::new_id();
                    let name = (args["name"].as_str())
                        .map_or_else(|| format!("Groove from {}", c.name), String::from);
                    grooves.push(daw_project::Groove {
                        id: gid.clone(),
                        name: name.clone(),
                        step,
                        steps,
                    });
                    out["groove"] = json!(gid);
                    out["name"] = json!(name);
                    (true, "Make a groove")
                }
                other => return Err(format!("There is no note tool {other}.")),
            };
            c.sort_notes();
            out["changed"] = json!(changed);
            label = name;
            Ok(out)
        })?;
        Ok((v, Some(label.into())))
    }

    fn grooves_value(&self) -> Value {
        json!({"grooves": self.grooves.iter().map(|g| json!({
            "id": g.id, "name": g.name, "step_beats": in_beats(g.step), "steps": g.steps.len(),
        })).collect::<Vec<_>>()})
    }

    /// Drum machine pads (note, name) of the tracks that play pattern
    /// `id`, or of any drum machine if none does.
    fn pads_for(&self, id: Option<&str>) -> Vec<(u8, String)> {
        use daw_instruments::drum_machine as dm;
        let plays = |t: &&crate::Track| {
            id.is_some_and(|id| {
                t.note_clips
                    .iter()
                    .any(|c| c.pattern.as_deref() == Some(id))
            })
        };
        let drums = |t: &&crate::Track| {
            t.instrument
                .as_ref()
                .is_some_and(|i| i.kind() == Some(daw_instruments::Kind::DrumMachine))
        };
        let slot = (self.tracks.iter().filter(plays).find(drums))
            .or_else(|| self.tracks.iter().find(drums))
            .and_then(|t| t.instrument.as_ref());
        slot.map_or_else(Vec::new, |s| {
            (s.pads_in_use().into_iter())
                .map(|p| (dm::FIRST_NOTE + p as u8, s.pad_name(p)))
                .collect()
        })
    }

    fn patterns_value(&self, args: &Value) -> Result<Value, String> {
        let only = args["pattern"].as_str();
        if let Some(id) = only
            && !self.patterns.iter().any(|p| p.id == id)
        {
            return Err(format!("There is no pattern {id}."));
        }
        let bar = self.tempo_map().signature_at(Ticks(0)).bar_ticks().max(1);
        let list: Vec<Value> = (self.patterns.iter())
            .filter(|p| only.is_none_or(|id| p.id == id))
            .map(|p| {
                let pads = self.pads_for(Some(&p.id));
                let clips: Vec<&str> = (self.tracks.iter())
                    .flat_map(|t| &t.note_clips)
                    .filter(|c| c.pattern.as_deref() == Some(p.id.as_str()))
                    .map(|c| c.id.as_str())
                    .collect();
                json!({
                    "id": p.id,
                    "name": p.name,
                    "bars": p.len as f64 / bar as f64,
                    "swing": p.swing,
                    "clips": clips,
                    "lanes": p.lanes.iter().map(|l| json!({
                        "id": l.id,
                        "name": l.name,
                        "pitch": l.pitch,
                        "pad": pads.iter().find(|(n, _)| *n == l.pitch).map(|(_, s)| s),
                        "rate": format!("1/{}", l.rate.name()),
                        "direction": l.direction.name(),
                        "muted": l.muted,
                        "steps": l.steps.iter().map(step_value).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({"patterns": list}))
    }

    fn edit_pattern_tool(&mut self, args: &Value) -> Done {
        let map = self.tempo_map();
        let bar = map.signature_at(Ticks(0)).bar_ticks().max(1);
        let new = args["new"].as_bool() == Some(true);
        let pid = match args["pattern"].as_str() {
            Some(id) => id.to_string(),
            None if new => daw_project::new_id(),
            None => return Err("Give the pattern's id, or new.".into()),
        };
        let pads = self.pads_for(Some(&pid));
        let pitch_of = |a: &Value| -> Result<Option<u8>, String> {
            if let Some(pad) = a["pad"].as_str() {
                return (pads.iter())
                    .find(|(_, n)| n.eq_ignore_ascii_case(pad))
                    .map(|(p, _)| Some(*p))
                    .ok_or_else(|| {
                        let names: Vec<&str> = pads.iter().map(|(_, n)| n.as_str()).collect();
                        format!("There is no pad {pad}; the pads are: {}.", names.join(", "))
                    });
            }
            match a["pitch"].as_u64() {
                Some(p) if p > 127 => Err(format!("Pitch {p} is past 127.")),
                Some(p) => Ok(Some(p as u8)),
                None => Ok(None),
            }
        };
        let place = match (args["track"].as_str(), args["bar"].as_i64()) {
            (Some(t), Some(b)) => Some((t.to_string(), map.bar_start(b.max(1)).0)),
            (None, None) => None,
            _ => return Err("Give both track and bar to add a clip playing it.".into()),
        };
        let mut label = "Change a pattern";
        let v = self.edit_model(|p| {
            if new {
                let n = p.patterns.len() + 1;
                let name = format!("Pattern {n}");
                let mut pat = if args["empty"].as_bool() == Some(true) {
                    Pattern::new(pid.clone(), name, bar)
                } else {
                    Pattern::drums(pid.clone(), name, bar)
                };
                pat.id = pid.clone();
                p.patterns.push(pat);
                label = "Add a pattern";
            }
            let k = (p.patterns.iter())
                .position(|x| x.id == pid)
                .ok_or_else(|| format!("There is no pattern {pid}."))?;
            if args["delete"].as_bool() == Some(true) {
                let used = (p.tracks.iter())
                    .flat_map(|t| &t.note_clips)
                    .any(|c| c.pattern.as_deref() == Some(pid.as_str()));
                if used {
                    return Err(
                        "Clips play the pattern; delete them, or turn them to notes, first.".into(),
                    );
                }
                p.patterns.remove(k);
                label = "Delete a pattern";
                return Ok(json!({"deleted": pid}));
            }
            let pat = &mut p.patterns[k];
            if let Some(n) = args["name"].as_str() {
                pat.name = n.into();
            }
            if let Some(b) = args["bars"].as_f64() {
                pat.len = ((b * bar as f64).round() as i64).max(1);
            }
            if let Some(s) = args["swing"].as_f64() {
                pat.swing = s.clamp(0.0, 1.0);
            }
            let set_lane = |l: &mut PatternLane, a: &Value| -> Result<(), String> {
                if let Some(n) = a["name"].as_str() {
                    l.name = n.into();
                }
                if let Some(p) = pitch_of(a)? {
                    l.pitch = p;
                }
                if let Some(n) = a["steps"].as_u64() {
                    l.set_len(n as usize);
                }
                if let Some(r) = a["rate"].as_str() {
                    l.rate = rate_of(r)?;
                }
                if let Some(d) = a["direction"].as_str() {
                    l.direction = Direction::from_name(d).ok_or_else(|| {
                        format!("{d} isn't a direction: forward, backward, pingpong or random.")
                    })?;
                }
                if let Some(m) = a["muted"].as_bool() {
                    l.muted = m;
                }
                if let Some(e) = a.get("euclid").filter(|e| e.is_object()) {
                    let hits = e["hits"].as_u64().ok_or("Euclid needs hits.")? as usize;
                    l.fill(hits, e["rotate"].as_u64().unwrap_or(0) as usize);
                }
                Ok(())
            };
            let mut added = Vec::new();
            for a in args["add_lanes"].as_array().into_iter().flatten() {
                let name = (a["name"].as_str())
                    .or(a["pad"].as_str())
                    .unwrap_or("Lane")
                    .to_string();
                let mut l = PatternLane::new(daw_project::new_id(), name, 36, 16);
                set_lane(&mut l, a)?;
                added.push(l.id.clone());
                pat.lanes.push(l);
            }
            for a in args["lanes"].as_array().into_iter().flatten() {
                let key = a["lane"].as_str().ok_or("Each lane change needs a lane.")?;
                let li = find_lane(pat, key)?;
                set_lane(&mut pat.lanes[li], a)?;
                if let Some(to) = a["index"].as_i64() {
                    let l = pat.lanes.remove(li);
                    let to = (to.max(0) as usize).min(pat.lanes.len());
                    pat.lanes.insert(to, l);
                }
            }
            for key in ids(&args["remove_lanes"]) {
                let li = find_lane(pat, &key)?;
                pat.lanes.remove(li);
            }
            for s in args["steps"].as_array().into_iter().flatten() {
                let key = s["lane"].as_str().ok_or("Each step needs a lane.")?;
                let li = find_lane(pat, key)?;
                let n = s["step"]
                    .as_u64()
                    .ok_or("Each step needs its number, from 1.")? as usize;
                let lane = &mut pat.lanes[li];
                if n == 0 || n > lane.steps.len() {
                    return Err(format!(
                        "{} has steps 1 to {}; there is no step {n}.",
                        lane.name,
                        lane.steps.len()
                    ));
                }
                set_step(&mut lane.steps[n - 1], s);
            }
            let (name, len) = (pat.name.clone(), pat.len);
            let mut clip = None;
            if let Some((track, start)) = &place {
                let t = (p.tracks.iter_mut())
                    .find(|t| &t.id == track)
                    .ok_or_else(|| format!("There is no track {track}."))?;
                if !t.kind.plays_notes() {
                    return Err(format!("{} isn't an instrument track.", t.name));
                }
                let mut c = NoteClip::new(daw_project::new_id(), name, *start, len);
                c.pattern = Some(pid.clone());
                clip = Some(c.id.clone());
                t.note_clips.push(c);
                t.note_clips.sort_by_key(|c| c.start);
            }
            daw_project::fill_pattern_clips(&p.patterns, &mut p.tracks);
            Ok(json!({"pattern": pid, "added_lanes": added, "clip": clip}))
        })?;
        Ok((v, Some(label.into())))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;

    fn id(v: Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    /// An instrument track with a two-bar note clip on bar 1.
    fn with_clip(d: &mut Driver) -> (String, String) {
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        let c = id(
            d.call("add_note_clip", json!({"track": t, "bar": 1, "bars": 2}))
                .unwrap(),
            "clip",
        );
        (t, c)
    }

    fn clip(d: &mut Driver, c: &str) -> Value {
        d.call("get_note_clip", json!({"clip": c})).unwrap()
    }

    #[test]
    fn chords_are_read_from_names() {
        use super::chord;
        assert_eq!(chord("C").unwrap(), [0, 4, 7]);
        assert_eq!(chord("Dm9").unwrap(), [2, 5, 9, 12, 16]);
        assert_eq!(chord("Bbmaj7").unwrap(), [10, 14, 17, 21]);
        assert_eq!(chord("F#m7b5").unwrap(), [6, 9, 12, 16]);
        assert_eq!(chord("A7b9").unwrap(), [9, 13, 16, 19, 22]);
        // The bass below the root.
        assert_eq!(chord("G/B").unwrap(), [-1, 7, 11, 14]);
        assert_eq!(chord("C/C").unwrap(), [-12, 0, 4, 7]);
        assert!(chord("H7").is_err());
        assert!(chord("Cweird").is_err());
    }

    #[test]
    fn a_clip_comes_with_its_chords_and_is_repeated_to_fill_a_section() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        let v = d
            .call(
                "add_note_clip",
                json!({"track": t, "bar": 5, "bars": 2, "notes": [
                    {"beat": 0, "beats": 4, "chord": "Dm9", "velocity": 70},
                    {"beat": 4, "beats": 4, "chord": "Gm9", "octave": 3},
                    {"beat": 0, "beats": 8, "pitches": [38, 50]},
                ]}),
            )
            .unwrap();
        assert_eq!(v["notes"], 12);
        let c = id(v, "clip");
        let notes = clip(&mut d, &c)["notes"].clone();
        let at0: Vec<u64> = (notes.as_array().unwrap().iter())
            .filter(|n| n["beat"] == 0.0 && n["beats"] == 4.0)
            .map(|n| n["pitch"].as_u64().unwrap())
            .collect();
        assert_eq!(at0, [62, 65, 69, 72, 76]);
        assert!(notes.as_array().unwrap().iter().any(|n| n["pitch"] == 55));

        let v = d
            .call("duplicate_clip", json!({"clip": c, "times": 3}))
            .unwrap();
        let copies: Vec<String> = (v["clips"].as_array().unwrap().iter())
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let bars: Vec<f64> = (copies.iter())
            .map(|k| clip(&mut d, k)["bar"].as_f64().unwrap())
            .collect();
        assert_eq!(bars, [7.0, 9.0, 11.0]);
        assert_eq!(
            clip(&mut d, &copies[2])["notes"].as_array().unwrap().len(),
            12
        );
        // All in one step.
        d.command("undo");
        assert!(d.call("get_note_clip", json!({"clip": copies[0]})).is_err());

        // A note clip can be picked, so commands act on it.
        d.call("select_clips", json!({"clips": [c]})).unwrap();
        let sel = d.call("get_selection", json!({})).unwrap();
        assert_eq!(sel["note_clip"], json!(c));
        assert!(
            d.call(
                "edit_notes",
                json!({"clip": c, "add": [{"beat": 0, "beats": 1, "chord": "Xm"}]})
            )
            .is_err()
        );
    }

    #[test]
    fn note_clips_are_moved_resized_muted_keyed_and_duplicated() {
        let mut d = Driver::new();
        let (t, c) = with_clip(&mut d);
        let other = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        d.call(
            "edit_clip",
            json!({"clip": c, "bar": 3, "len_beats": 4, "offset_beats": 1,
                   "muted": true, "name": "Riff", "key": "A minor"}),
        )
        .unwrap();
        let v = clip(&mut d, &c);
        assert_eq!(v["bar"], 3.0);
        assert_eq!(v["start_ticks"], 2 * 4 * 960);
        assert_eq!(v["len_ticks"], 4 * 960);
        assert_eq!(v["offset_beats"], 1.0);
        assert_eq!(v["muted"], true);
        assert_eq!(v["name"], "Riff");
        assert_eq!(v["key"], "A minor");
        assert!(
            d.call("edit_clip", json!({"clip": c, "key": "H major"}))
                .is_err()
        );

        // Straight after it, on another track.
        let copy = id(
            d.call("duplicate_clip", json!({"clip": c, "track": other}))
                .unwrap(),
            "clip",
        );
        let v = clip(&mut d, &copy);
        assert_eq!(
            (v["track"].clone(), v["bar"].clone()),
            (json!(other), json!(4.0))
        );
        assert_eq!(v["name"], "Riff");
        d.call(
            "edit_clip",
            json!({"clip": copy, "track": t, "key": "none"}),
        )
        .unwrap();
        let v = clip(&mut d, &copy);
        assert_eq!(
            (v["track"].clone(), v["key"].clone()),
            (json!(t), json!(null))
        );
        // Each was its own step.
        d.command("undo");
        assert_eq!(clip(&mut d, &copy)["track"], json!(other));
        d.command("undo");
        assert!(d.call("get_note_clip", json!({"clip": copy})).is_err());
    }

    #[test]
    fn notes_get_chance_and_groups_and_are_changed_by_id() {
        let mut d = Driver::new();
        let (_, c) = with_clip(&mut d);
        let added = d
            .call(
                "edit_notes",
                json!({"clip": c, "add": [
                    {"beat": 0, "beats": 1, "pitch": 60},
                    {"beat": 0, "beats": 1, "pitch": 64, "chance": 0.5, "channel": 2},
                    {"beat": 0, "beats": 1, "pitch": 67, "velocity": 90},
                ]}),
            )
            .unwrap();
        let ids: Vec<String> = (added["added"].as_array().unwrap().iter())
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let v = d
            .call(
                "edit_notes",
                json!({"clip": c,
                       "change": [{"id": ids[0], "beat": 1.5, "pitch": 62, "chance": 0.25}],
                       "group": [{"notes": [ids[1], ids[2]], "one_of": true}]}),
            )
            .unwrap();
        assert_eq!(v["groups"].as_array().unwrap().len(), 1);
        let v = clip(&mut d, &c);
        let note = |i: &str| {
            (v["notes"].as_array().unwrap().iter())
                .find(|n| n["id"] == i)
                .unwrap()
                .clone()
        };
        assert_eq!(note(&ids[0])["beat"], 1.5);
        assert_eq!(note(&ids[0])["pitch"], 62);
        assert_eq!(note(&ids[0])["chance"], 0.25);
        assert_eq!(note(&ids[1])["chance"], 0.5);
        assert_eq!(note(&ids[1])["channel"], 2);
        assert_eq!(note(&ids[2])["velocity"], 90);
        assert_eq!(note(&ids[1])["group"], note(&ids[2])["group"]);
        assert_eq!(note(&ids[2])["one_of"], true);
        assert_eq!(note(&ids[0])["group"], json!(null));

        d.call("edit_notes", json!({"clip": c, "ungroup": [ids[1]]}))
            .unwrap();
        let v = clip(&mut d, &c);
        let n1 = (v["notes"].as_array().unwrap().iter())
            .find(|n| n["id"] == ids[1].as_str())
            .unwrap();
        assert_eq!(
            (n1["group"].clone(), n1["one_of"].clone()),
            (json!(null), json!(false))
        );
        assert!(
            d.call("edit_notes", json!({"clip": c, "change": [{"id": "nope"}]}))
                .is_err()
        );
    }

    #[test]
    fn controller_and_expression_lanes_are_drawn_and_reach_the_engine() {
        let mut d = Driver::new();
        let (_, c) = with_clip(&mut d);
        let n = d
            .call(
                "edit_notes",
                json!({"clip": c, "add": [{"beat": 0, "beats": 2, "pitch": 60}]}),
            )
            .unwrap();
        let n = n["added"][0].as_str().unwrap().to_string();
        d.call(
            "edit_note_lanes",
            json!({"clip": c,
                   "controllers": [
                       {"controller": "cc:1", "points": [{"beat": 0, "value": 0}, {"beat": 4, "value": 300}]},
                       {"controller": "bend", "points": [{"beat": 1, "value": 4096}]}],
                   "expression": [{"note": n, "kind": "pitch",
                                   "points": [{"beat": 0, "value": 0}, {"beat": 1, "value": 2}]}]}),
        )
        .unwrap();
        let v = clip(&mut d, &c);
        let lanes = v["controllers"].as_array().unwrap();
        assert_eq!(lanes.len(), 2);
        let mw = lanes.iter().find(|l| l["controller"] == "cc:1").unwrap();
        assert_eq!(mw["points"][1], json!({"beat": 4.0, "value": 127.0}));
        assert_eq!(v["notes"][0]["expression"][0]["kind"], "pitch");
        assert_eq!(v["notes"][0]["expression"][0]["points"][1]["value"], 2.0);
        // The engine is given the lines.
        let app = d.app();
        let clips: Vec<_> = (app.tracks.iter())
            .flat_map(|t| t.note_clips.clone())
            .collect();
        let (_, expr, controls) = crate::note_tools::specs(&clips);
        assert!(!expr.is_empty() && !controls.is_empty());

        d.call(
            "edit_note_lanes",
            json!({"clip": c, "controllers": [{"controller": "bend", "remove": true}],
                   "expression": [{"note": n, "kind": "pitch", "points": []}]}),
        )
        .unwrap();
        let v = clip(&mut d, &c);
        assert_eq!(v["controllers"].as_array().unwrap().len(), 1);
        assert_eq!(v["notes"][0]["expression"], json!([]));
        assert!(
            d.call(
                "edit_note_lanes",
                json!({"clip": c, "controllers": [{"controller": "cc:200"}]})
            )
            .is_err()
        );
    }

    #[test]
    fn note_tools_take_their_settings_and_a_clip() {
        let mut d = Driver::new();
        let (_, c) = with_clip(&mut d);
        let added = d
            .call(
                "edit_notes",
                json!({"clip": c, "add": [
                    {"beat": 0.1, "beats": 0.25, "pitch": 61},
                    {"beat": 1.05, "beats": 0.25, "pitch": 63},
                    {"beat": 2.0, "beats": 0.25, "pitch": 66, "velocity": 50},
                ]}),
            )
            .unwrap();
        let first = added["added"][0].as_str().unwrap().to_string();
        let beats = |d: &mut Driver| -> Vec<f64> {
            (clip(d, &c)["notes"].as_array().unwrap().iter())
                .map(|n| n["beat"].as_f64().unwrap())
                .collect()
        };
        // Only the note given.
        d.call(
            "note_tool",
            json!({"clip": c, "tool": "quantise", "notes": [first], "grid": "beat"}),
        )
        .unwrap();
        assert_eq!(beats(&mut d)[..2], [0.0, 1.05]);
        d.call(
            "note_tool",
            json!({"clip": c, "tool": "quantise", "grid": "1/16", "strength": 100}),
        )
        .unwrap();
        assert_eq!(beats(&mut d), [0.0, 1.0, 2.0]);

        d.call(
            "note_tool",
            json!({"clip": c, "tool": "legato", "gap_beats": 0.25}),
        )
        .unwrap();
        let v = clip(&mut d, &c);
        assert_eq!(v["notes"][0]["beats"], 0.75);
        assert_eq!(v["notes"][1]["beats"], 0.75);

        // No key anywhere: it asks for one; given one, notes move to it.
        assert!(
            d.call("note_tool", json!({"clip": c, "tool": "fit_to_scale"}))
                .is_err()
        );
        let v = d
            .call(
                "note_tool",
                json!({"clip": c, "tool": "fit_to_scale", "key": "C major"}),
            )
            .unwrap();
        assert_eq!(v["key"], "C major");
        let pitches: Vec<u64> = (clip(&mut d, &c)["notes"].as_array().unwrap().iter())
            .map(|n| n["pitch"].as_u64().unwrap())
            .collect();
        assert!(
            pitches
                .iter()
                .all(|p| [0, 2, 4, 5, 7, 9, 11].contains(&(p % 12))),
            "{pitches:?}"
        );

        let before = beats(&mut d);
        d.call(
            "note_tool",
            json!({"clip": c, "tool": "humanise", "timing_beats": 0.1, "seed": 7}),
        )
        .unwrap();
        assert_ne!(beats(&mut d), before);

        // A groove from these notes, then put on another clip.
        let g = d
            .call(
                "note_tool",
                json!({"clip": c, "tool": "make_groove", "name": "Mine", "grid": "beat"}),
            )
            .unwrap();
        let list = d.call("list_grooves", json!({})).unwrap();
        assert_eq!(list["grooves"][0]["id"], g["groove"]);
        assert_eq!(list["grooves"][0]["step_beats"], 1.0);
        let v = d
            .call(
                "note_tool",
                json!({"clip": c, "tool": "groove", "groove": "mine", "accent": 100}),
            )
            .unwrap();
        assert!(v["changed"].is_boolean());
        assert!(
            d.call(
                "note_tool",
                json!({"clip": c, "tool": "groove", "groove": "nope"})
            )
            .is_err()
        );
        // Every tool was a step of its own.
        let h = d.call("get_history", json!({})).unwrap();
        assert!(h.to_string().contains("Humanise notes"), "{h}");
    }

    #[test]
    fn patterns_are_made_read_and_edited_step_by_step() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "drum-machine"}))
                .unwrap(),
            "track",
        );
        let v = d
            .call(
                "edit_pattern",
                json!({"new": true, "empty": true, "name": "Beat", "bars": 1,
                       "track": t, "bar": 1,
                       "add_lanes": [{"name": "Kick", "pitch": 36, "steps": 16},
                                     {"name": "Hat", "pitch": 42, "steps": 12, "rate": "1/16t"}]}),
            )
            .unwrap();
        let pat = id(v.clone(), "pattern");
        let pc = id(v, "clip");
        d.call(
            "edit_pattern",
            json!({"pattern": pat, "swing": 0.5,
                   "lanes": [{"lane": "kick", "euclid": {"hits": 4}},
                             {"lane": "Hat", "direction": "pingpong", "index": 0}],
                   "steps": [{"lane": "Kick", "step": 2, "on": true, "velocity": 60,
                              "chance": 0.5, "repeats": 2, "offset": 0.9, "gate": 1.0}]}),
        )
        .unwrap();
        let ps = d.call("get_patterns", json!({"pattern": pat})).unwrap();
        let p = &ps["patterns"][0];
        assert_eq!(p["name"], "Beat");
        assert_eq!(p["swing"], 0.5);
        assert_eq!(p["clips"], json!([pc]));
        assert_eq!(p["lanes"][0]["name"], "Hat");
        assert_eq!(p["lanes"][0]["rate"], "1/16t");
        assert_eq!(p["lanes"][0]["direction"], "pingpong");
        let kick = &p["lanes"][1]["steps"];
        let on: Vec<bool> = (kick.as_array().unwrap().iter())
            .map(|s| s["on"].as_bool().unwrap())
            .collect();
        assert_eq!(on.iter().filter(|o| **o).count(), 5);
        assert_eq!(
            kick[1],
            json!({"on": true, "velocity": 60, "vary": 0, "pitch": 0, "chance": 0.5,
                   "repeats": 2, "offset": 0.5, "gate": 1.0})
        );
        // The clip plays it: four kicks, and step 2 twice.
        let notes = clip(&mut d, &pc)["notes"].as_array().unwrap().len();
        assert_eq!(notes, 6);
        assert!(
            d.call(
                "edit_pattern",
                json!({"pattern": pat, "steps": [{"lane": "Kick", "step": 17, "on": true}]})
            )
            .is_err()
        );

        // A drum machine's pad by name.
        let pads = d.call("get_patterns", json!({})).unwrap();
        if let Some(name) = pads["patterns"][0]["lanes"][1]["pad"].as_str() {
            let v = d
                .call(
                    "edit_pattern",
                    json!({"pattern": pat, "add_lanes": [{"pad": name}]}),
                )
                .unwrap();
            assert_eq!(v["added_lanes"].as_array().unwrap().len(), 1);
        }

        // Linked and unlinked copies.
        let linked = id(
            d.call("duplicate_clip", json!({"clip": pc})).unwrap(),
            "clip",
        );
        let own = d
            .call("duplicate_clip", json!({"clip": pc, "linked": false}))
            .unwrap();
        assert_eq!(clip(&mut d, &linked)["pattern"], json!(pat));
        assert_ne!(own["pattern"], json!(pat));
        assert!(
            d.call("edit_notes", json!({"clip": pc, "clear": true}))
                .is_err()
        );
        // Kept as notes, it no longer follows the pattern.
        d.call(
            "edit_clip",
            json!({"clip": linked, "pattern_to_notes": true}),
        )
        .unwrap();
        let v = clip(&mut d, &linked);
        assert_eq!(v["pattern"], json!(null));
        assert!(!v["notes"].as_array().unwrap().is_empty());
        d.call("edit_clip", json!({"clip": pc, "pattern_unique": true}))
            .unwrap();
        assert_ne!(clip(&mut d, &pc)["pattern"], json!(pat));
        // No clip plays it now, so it can go.
        d.call("edit_pattern", json!({"pattern": pat, "delete": true}))
            .unwrap();
        let left = d.call("get_patterns", json!({})).unwrap();
        assert!(!left.to_string().contains(&format!("\"id\":\"{pat}\"")));
    }
}
