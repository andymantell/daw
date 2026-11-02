// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for what the timeline, note editor and comments
//! panes do by hand (roadmap 2.11): clips, notes, markers, tempo, key,
//! the loop and comments. Each is one undo step, as every edit by an
//! assistant is (see `agent.rs`); they edit the project as the document
//! has it, as `agent_edit.rs` does.
//!
//! Times are given as a bar (counted from 1, fractions within it) or in
//! seconds; notes in beats (quarter notes) from their clip's start.

use daw_engine::time::{Frames, TempoMap, Ticks};
use daw_project::{Project, Scale, ScaleKind};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};

type Done = Result<(Value, Option<String>), String>;

fn time_props() -> Value {
    json!({
        "bar": {"type": "number", "description": "A bar, from 1; 2.5 is halfway through bar 2."},
        "seconds": {"type": "number"}
    })
}

fn with_time(mut props: Value, required: &[&str]) -> Value {
    for (k, v) in time_props().as_object().unwrap() {
        props[k] = v.clone();
    }
    json!({"type": "object", "properties": props, "required": required})
}

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_comments",
        description: "Every comment pinned to the timeline, with its id, track, time, \
                      text, replies and whether it is resolved.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "edit_clip",
        description: "Changes a clip. Either kind: where it starts (bar or seconds), the \
                      track it is on, mute or name. An audio clip: its length and offset \
                      into its file, in seconds of the file's own audio (so for a \
                      clip that follows tempo or has stretch markers, not the time \
                      it takes on the timeline: the answer's end_seconds gives where \
                      it ends there), gain and fades (seconds), and its \
                      transpose (semitones, fractions for cents, within 12 either way; \
                      0 for its own pitch), which keeps its timing and works with \
                      follow tempo; its formants stay put unless move_formants. A note clip: its \
                      length and offset into its notes (beats), its own key (\"none\" \
                      for the song's), the pattern it plays, pattern_unique (a copy of \
                      its pattern of its own) or pattern_to_notes (its notes, no longer \
                      following the pattern). A pattern clip longer than its pattern plays \
                      the pattern over and over. A note clip loops its notes with loop \
                      true (the notes from its start to its end as it is now) or \
                      loop_beats (that much of its start); then a longer len_beats plays \
                      them again and again, and editing the notes changes every time \
                      round (a 4-bar bass part looped to 128 beats plays 8 times, one \
                      clip). loop false plays the notes once again. An audio clip can also \
                      be moved by a point in it: anchor_seconds (timeline seconds, as \
                      analyse gives note starts and beats) lands on the bar or seconds \
                      given, or on the nearest beat or bar with snap. Only what is given \
                      changes; duplicate_clip copies one.",
        access: Access::Edit,
        schema: || {
            with_time(
                json!({
                    "clip": {"type": "string"},
                    "track": {"type": "string", "description": "Move it to this track."},
                    "len_seconds": {"type": "number", "description": "Audio clips: how much of the file plays, in the file's own seconds (before any stretch)."},
                    "offset_seconds": {"type": "number", "description": "Audio clips: where in the file it starts playing, in the file's own seconds."},
                    "gain_db": {"type": "number"},
                    "fade_in_seconds": {"type": "number"},
                    "fade_out_seconds": {"type": "number"},
                    "transpose_semitones": {"type": "number", "description": "Audio clips: -12 to 12, such as -6 or 2.5 (2 semitones 50 cents)."},
                    "move_formants": {"type": "boolean", "description": "Audio clips: true moves the formants with the pitch (like tape sped up); false (the default) keeps a voice sounding like the same person."},
                    "anchor_seconds": {"type": "number", "description": "Audio clips: a point inside the clip, in timeline seconds where it plays now (such as a note start from analyse). The clip moves so this point lands on bar (2.5 is halfway through bar 2), on seconds, or with snap on the nearest beat or bar, instead of its start landing there."},
                    "snap": {"type": "string", "enum": ["beat", "bar"], "description": "With anchor_seconds and no bar or seconds: the anchor goes to the nearest beat or bar line."},
                    "muted": {"type": "boolean"},
                    "name": {"type": "string"},
                    "len_beats": {"type": "number", "description": "Note clips."},
                    "loop": {"type": "boolean", "description": "Note clips: true loops its notes, false stops."},
                    "loop_beats": {"type": "number", "description": "Note clips: loop this many beats of its notes from its content start."},
                    "offset_beats": {"type": "number", "description": "Note clips."},
                    "key": {"type": "string", "description": "Note clips: such as \"A minor\", or \"none\"."},
                    "pattern": {"type": "string", "description": "Note clips: a pattern's id."},
                    "pattern_unique": {"type": "boolean"},
                    "pattern_to_notes": {"type": "boolean"}
                }),
                &["clip"],
            )
        },
    },
    Tool {
        name: "delete_clip",
        description: "Deletes an audio clip or a note clip.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clip": {"type": "string"}
            }, "required": ["clip"]})
        },
    },
    Tool {
        name: "import_audio",
        description: "Brings a sound file on this computer in as a new track, its clip \
                      starting at the time given (the start if left out). The file opens \
                      in the background: get_project shows the track once it has.",
        access: Access::Edit,
        schema: || {
            with_time(
                json!({"path": {"type": "string", "description": "The file's full path."}}),
                &["path"],
            )
        },
    },
    Tool {
        name: "edit_markers",
        description: "Adds section markers on bars, renames or moves them by id, or \
                      removes them. play_order sets the sections' play order (get_project \
                      shows it), by their markers' ids in the order to play, repeats allowed; \
                      commands play-order and write-play-order play it or lay the song out so.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "add": {"type": "array", "items": {"type": "object", "properties": {
                    "bar": {"type": "integer"}, "name": {"type": "string"}
                }, "required": ["bar", "name"]}},
                "change": {"type": "array", "items": {"type": "object", "properties": {
                    "id": {"type": "string"}, "bar": {"type": "integer"}, "name": {"type": "string"}
                }, "required": ["id"]}},
                "remove": {"type": "array", "items": {"type": "string"}},
                "play_order": {"type": "array", "items": {"type": "string"}}
            }})
        },
    },
    Tool {
        name: "set_tempo",
        description: "Sets the song's starting tempo and time signature, and, if given, \
                      the tempo changes after the start, or the time signature changes \
                      (each list replacing those there). Clips that follow tempo move \
                      with it; others stay where they are.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "bpm": {"type": "number"},
                "beats_per_bar": {"type": "integer"},
                "beat_unit": {"type": "integer", "description": "4 for quarter notes, 8 for eighths."},
                "changes": {"type": "array", "items": {"type": "object", "properties": {
                    "bar": {"type": "integer"}, "beat": {"type": "integer"},
                    "bpm": {"type": "number"}, "glide": {"type": "boolean"}
                }, "required": ["bar", "bpm"]}},
                "meters": {"type": "array", "description": "Time signature changes, each on a bar after the first.",
                           "items": {"type": "object", "properties": {
                    "bar": {"type": "integer"}, "beats_per_bar": {"type": "integer"},
                    "beat_unit": {"type": "integer"}
                }, "required": ["bar", "beats_per_bar"]}}
            }})
        },
    },
    Tool {
        name: "set_key",
        description: "Sets the song's key and scale, such as \"A minor\" or \"D dorian\", or \
                      \"none\" to clear it. The note editor shades it.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "key": {"type": "string"}
            }, "required": ["key"]})
        },
    },
    Tool {
        name: "set_loop",
        description: "Sets the loop to run from the start of one bar to the start of \
                      another, and turns it on or off.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "from_bar": {"type": "integer"},
                "to_bar": {"type": "integer"},
                "on": {"type": "boolean"}
            }})
        },
    },
    Tool {
        name: "add_comment",
        description: "Pins a comment to the timeline, or to a track, at a time (the \
                      playhead if left out) or over a stretch (to an end, or on the loop), \
                      or replies to a comment.",
        access: Access::Edit,
        schema: || {
            with_time(
                json!({
                    "text": {"type": "string"},
                    "track": {"type": "string"},
                    "reply_to": {"type": "string", "description": "A comment's id."},
                    "to_bar": {"type": "number", "description": "Where a comment on a stretch ends, as a bar (from 1, fractions within it)."},
                    "to_seconds": {"type": "number", "description": "Where it ends, in seconds."},
                    "on_loop": {"type": "boolean", "description": "Pin it on the loop, from its start to its end."}
                }),
                &["text"],
            )
        },
    },
    Tool {
        name: "edit_comment",
        description: "Changes a comment's or reply's text, or deletes it for good (a \
                      comment that starts a thread goes with its replies). A commenter \
                      may change only their own.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "comment": {"type": "string", "description": "A comment's or reply's id."},
                "text": {"type": "string"},
                "delete": {"type": "boolean"}
            }, "required": ["comment"]})
        },
    },
    Tool {
        name: "resolve_comment",
        description: "Marks a comment as resolved, or not.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "comment": {"type": "string"},
                "resolved": {"type": "boolean", "description": "true if left out."}
            }, "required": ["comment"]})
        },
    },
];

/// Frame of a bar, from 1, fractions within it.
fn bar_frames(map: &TempoMap, bar: f64) -> i64 {
    let whole = bar.floor() as i64;
    let from = map.bar_start(whole);
    let len = map.bar_start(whole + 1).0 - from.0;
    let t = Ticks(from.0 + ((bar - bar.floor()) * len as f64) as i64);
    map.frames_at(t).0
}

fn text<'a>(args: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("Give {what}."))
}

/// A key from words such as "A minor" or "F# harmonic minor".
pub(crate) fn parse_key(s: &str) -> Result<Option<Scale>, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") || s.is_empty() {
        return Ok(None);
    }
    let bad = || format!("{s} isn't a key; say a root and a scale, such as \"A minor\".");
    let (root, kind) = s.split_once(' ').ok_or_else(bad)?;
    let flats = [("Db", 1), ("Eb", 3), ("Gb", 6), ("Ab", 8), ("Bb", 10)];
    let root = (daw_project::ROOTS.iter())
        .position(|r| r.eq_ignore_ascii_case(root))
        .or_else(|| {
            (flats.iter())
                .find(|(f, _)| f.eq_ignore_ascii_case(root))
                .map(|&(_, n)| n)
        })
        .ok_or_else(bad)?;
    let kind = kind.trim();
    let kind = (ScaleKind::ALL.into_iter())
        .find(|k| k.label().eq_ignore_ascii_case(kind) || k.name().eq_ignore_ascii_case(kind))
        .ok_or_else(bad)?;
    Ok(Some(Scale::new(root as u8, kind)))
}

/// Where the clip with this id is: track, and audio clip or note clip.
enum Found {
    Audio(usize, usize),
    Notes(usize, usize),
}

fn find_clip(p: &Project, id: &str) -> Result<Found, String> {
    for (i, t) in p.tracks.iter().enumerate() {
        if let Some(j) = t.clips.iter().position(|c| c.id == id) {
            return Ok(Found::Audio(i, j));
        }
        if let Some(j) = t.note_clips.iter().position(|c| c.id == id) {
            return Ok(Found::Notes(i, j));
        }
    }
    Err(format!("There is no clip {id}."))
}

fn now_ms() -> i64 {
    (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
        .map_or(0, |d| d.as_millis() as i64)
}

impl App {
    /// Runs one of the tools here, or `None` if `name` isn't one.
    pub(crate) fn arrange_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        Some(match name {
            "edit_clip" => self.edit_clip_tool(args),
            "delete_clip" => self.delete_clip_tool(args),
            "import_audio" => self.import_audio_tool(args),
            "edit_markers" => self.edit_markers_tool(args),
            "set_tempo" => self.set_tempo_tool(args),
            "set_key" => self.set_key_tool(args),
            "set_loop" => self.set_loop_tool(args),
            "add_comment" => self.add_comment_tool(args),
            "resolve_comment" => self.resolve_comment_tool(args),
            "edit_comment" => self.edit_comment_tool(args),
            _ => return None,
        })
    }

    /// The frame `args` gives as a bar or seconds, if it gives one.
    fn time_arg(&self, args: &Value) -> Option<i64> {
        match (args["bar"].as_f64(), args["seconds"].as_f64()) {
            (Some(bar), _) => Some(bar_frames(&self.tempo_map(), bar.max(1.0))),
            (None, Some(s)) => Some((s.max(0.0) * self.rate as f64).round() as i64),
            (None, None) => None,
        }
    }

    pub(crate) fn comments_value(&self) -> Value {
        let rate = self.rate.max(1) as f64;
        let map = self.tempo_map();
        let tracks: std::collections::HashMap<&str, &str> = (self.tracks.iter())
            .map(|t| (t.key.as_str(), t.name.as_str()))
            .collect();
        let one = |c: &daw_project::Comment| {
            let bb = map.bar_beat(map.ticks_at(Frames(c.start)));
            json!({
                "id": c.id,
                "text": c.text,
                "track": c.track,
                "track_name": c.track.as_deref().and_then(|t| tracks.get(t)),
                "seconds": c.start as f64 / rate,
                "end_seconds": c.end.map(|e| e as f64 / rate),
                "bar": bb.bar,
                "beat": bb.beat,
                "resolved": c.resolved,
                "author": c.author,
                "created_ms": c.created,
            })
        };
        let threads: Vec<Value> = (self.comments.iter())
            .filter(|c| c.parent.is_none())
            .map(|c| {
                let mut v = one(c);
                v["replies"] = (self.comments.iter())
                    .filter(|r| r.parent.as_deref() == Some(&c.id))
                    .map(|r| json!({"id": r.id, "text": r.text, "author": r.author}))
                    .collect();
                v
            })
            .collect();
        json!({"comments": threads})
    }

    fn edit_clip_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        if let Ok(Found::Notes(..)) = find_clip(&self.project(), &id) {
            return self.edit_note_clip(args);
        }
        if args["loop"].is_boolean() || args["loop_beats"].is_number() {
            return Err("Only note clips loop; duplicate_clip repeats an audio clip.".into());
        }
        let start = self.time_arg(args);
        let rate = self.rate as f64;
        let map = self.tempo_map();
        let frames = |k: &str| args[k].as_f64().map(|s| (s.max(0.0) * rate).round() as i64);
        let anchor = match args.get("anchor_seconds").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => Some(
                v.as_f64()
                    .filter(|x| x.is_finite() && *x >= 0.0)
                    .map(|x| (x * rate).round() as i64)
                    .ok_or("anchor_seconds is a time in seconds, 0 or more.")?,
            ),
        };
        let snap = match args["snap"].as_str() {
            None => None,
            Some("beat") => Some(false),
            Some("bar") => Some(true),
            Some(x) => return Err(format!("snap is \"beat\" or \"bar\", not {x:?}.")),
        };
        if snap.is_some() && anchor.is_none() {
            return Err("snap moves a clip by a point in it: give anchor_seconds too.".into());
        }
        if snap.is_some() && start.is_some() {
            return Err("Give a bar or seconds for the anchor, or snap, not both.".into());
        }
        if anchor.is_some() && start.is_none() && snap.is_none() {
            return Err("Say where the anchor goes: a bar, seconds, or snap.".into());
        }
        if let Some(a) = anchor {
            // Where the clip plays now, stretched or not.
            let live = (self.tracks.iter().flat_map(|t| &t.clips)).find(|c| c.key == id);
            if let Some(c) = live
                && (a < c.start || a > c.end())
            {
                return Err(format!(
                    "anchor_seconds {:.3} isn't in the clip: it plays from {:.3} to {:.3} seconds.",
                    a as f64 / rate,
                    c.start as f64 / rate,
                    c.end() as f64 / rate
                ));
            }
        }
        self.edit_model(|p| {
            let Found::Audio(mut i, j) = find_clip(p, &id)? else {
                unreachable!("note clips are handed on first");
            };
            if let Some(to) = args["track"].as_str() {
                let ti = (p.tracks.iter())
                    .position(|t| t.id == to)
                    .ok_or_else(|| format!("There is no track {to}."))?;
                if p.tracks[ti].kind.is_bus() {
                    return Err(format!("{to} is a bus, which has no clips."));
                }
                if ti != i {
                    let c = p.tracks[i].clips.remove(j);
                    p.tracks[ti].clips.push(c);
                    i = ti;
                }
            }
            let j = (p.tracks[i].clips.iter())
                .position(|c| c.id == id)
                .expect("still there");
            let c = &mut p.tracks[i].clips[j];
            let mut moved = None;
            if let Some(a) = anchor {
                let to = match snap {
                    Some(bar) => crate::stretch::nearest_line(&map, a, bar),
                    None => start.expect("checked above"),
                };
                let (from, beat) = crate::stretch::moved_by_anchor(
                    &map,
                    c.start,
                    c.beat,
                    c.tempo.is_some(),
                    a,
                    to,
                )?;
                moved = Some((to - a) as f64 / rate);
                c.start = from;
                if c.tempo.is_some() {
                    c.beat = beat;
                }
            } else if let Some(s) = start {
                c.start = s;
                if c.tempo.is_some() {
                    c.beat = map.ticks_at(Frames(s)).0;
                }
            }
            if let Some(l) = frames("len_seconds") {
                c.len = l.max(1);
            }
            if let Some(o) = frames("offset_seconds") {
                c.offset = o;
            }
            if let Some(g) = args["gain_db"].as_f64() {
                c.gain_db = g.clamp(-60.0, 24.0);
            }
            if let Some(f) = frames("fade_in_seconds") {
                c.fade_in.len = f.min(c.len);
            }
            if let Some(f) = frames("fade_out_seconds") {
                c.fade_out.len = f.min(c.len);
            }
            if let Some(m) = args["muted"].as_bool() {
                c.muted = m;
            }
            if let Some(n) = args["name"].as_str() {
                c.name = n.into();
            }
            if let Some(st) = args.get("transpose_semitones").filter(|v| !v.is_null()) {
                let st = st
                    .as_f64()
                    .filter(|s| s.is_finite())
                    .ok_or("transpose_semitones is a number.")?;
                let cents = (st * 100.0).round() as i64;
                if cents.abs() > daw_project::MAX_TRANSPOSE_CENTS {
                    return Err("Transpose a clip by at most 12 semitones either way.".into());
                }
                c.cents = cents;
            }
            if let Some(m) = args["move_formants"].as_bool() {
                c.move_formants = m;
            }
            let mut v = json!({"clip": id, "start_seconds": c.start as f64 / rate,
                      "len_seconds": c.len as f64 / rate,
                      "offset_seconds": c.offset as f64 / rate,
                      "transpose_semitones": c.cents as f64 / 100.0,
                      "move_formants": c.move_formants});
            if let (Some(a), Some(by)) = (anchor, moved) {
                v["anchor_now_seconds"] = json!(a as f64 / rate + by);
                v["moved_seconds"] = json!(by);
            }
            Ok(v)
        })
        .map(|mut v| {
            v["end_seconds"] = json!(self.clip_end(&id).map(|e| e as f64 / rate));
            (v, Some("Change a clip".into()))
        })
    }

    fn delete_clip_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        self.edit_model(|p| {
            match find_clip(p, &id)? {
                Found::Audio(i, j) => {
                    p.tracks[i].clips.remove(j);
                }
                Found::Notes(i, j) => {
                    p.tracks[i].note_clips.remove(j);
                }
            }
            Ok(json!({"deleted": id}))
        })
        .map(|v| (v, Some("Delete a clip".into())))
    }

    fn import_audio_tool(&mut self, args: &Value) -> Done {
        let path = std::path::PathBuf::from(text(args, "path", "the file's full path")?);
        if !path.is_file() {
            return Err(format!("There is no file {}.", path.display()));
        }
        let at = self.time_arg(args).unwrap_or(0);
        self.add_file_at(&path, at);
        // The track comes when the file has opened, as a step of its own.
        Ok((json!({"importing": path.display().to_string()}), None))
    }

    fn edit_markers_tool(&mut self, args: &Value) -> Done {
        self.edit_model(|p| {
            let mut added = Vec::new();
            for m in args["add"].as_array().into_iter().flatten() {
                let bar = m["bar"]
                    .as_i64()
                    .ok_or("Each new marker needs a bar.")?
                    .max(1);
                let name = m["name"].as_str().unwrap_or("").to_string();
                let id = daw_project::new_id();
                added.push(id.clone());
                p.markers.push(daw_project::Marker { id, bar, name });
            }
            for m in args["change"].as_array().into_iter().flatten() {
                let id = m["id"].as_str().ok_or("Each change needs a marker's id.")?;
                let mk = (p.markers.iter_mut())
                    .find(|k| k.id == id)
                    .ok_or_else(|| format!("There is no marker {id}."))?;
                if let Some(b) = m["bar"].as_i64() {
                    mk.bar = b.max(1);
                }
                if let Some(n) = m["name"].as_str() {
                    mk.name = n.into();
                }
            }
            let remove: Vec<&str> = (args["remove"].as_array().into_iter().flatten())
                .filter_map(|v| v.as_str())
                .collect();
            if let Some(id) = remove
                .iter()
                .find(|id| !p.markers.iter().any(|m| m.id == **id))
            {
                return Err(format!("There is no marker {id}."));
            }
            p.markers.retain(|m| !remove.contains(&m.id.as_str()));
            if let Some(order) = args["play_order"].as_array() {
                let mut ids = Vec::new();
                for id in order {
                    let id = id.as_str().ok_or("The play order is markers' ids.")?;
                    if !p.markers.iter().any(|m| m.id == id) {
                        return Err(format!("There is no marker {id}."));
                    }
                    ids.push(id.to_string());
                }
                p.arrangement = ids;
            }
            p.arrangement
                .retain(|id| p.markers.iter().any(|m| &m.id == id));
            p.markers.sort_by_key(|m| m.bar);
            Ok(json!({"added": added, "markers": p.markers.len()}))
        })
        .map(|v| (v, Some("Change markers".into())))
    }

    fn set_tempo_tool(&mut self, args: &Value) -> Done {
        self.edit_model(|p| {
            if let Some(b) = args["bpm"].as_f64() {
                if !(10.0..=999.0).contains(&b) {
                    return Err("A tempo is between 10 and 999 bpm.".into());
                }
                p.tempo_bpm = b;
            }
            if let Some(n) = args["beats_per_bar"].as_u64() {
                p.beats_per_bar = (n as u32).clamp(1, 32);
            }
            if let Some(u) = args["beat_unit"].as_u64() {
                if ![1, 2, 4, 8, 16, 32].contains(&u) {
                    return Err("A beat unit is 1, 2, 4, 8, 16 or 32.".into());
                }
                p.beat_unit = u as u32;
            }
            if let Some(changes) = args["changes"].as_array() {
                let mut v = Vec::new();
                for c in changes {
                    let (Some(bar), Some(bpm)) = (c["bar"].as_i64(), c["bpm"].as_f64()) else {
                        return Err("Each tempo change needs a bar and a bpm.".into());
                    };
                    v.push(daw_project::TempoChange {
                        id: daw_project::new_id(),
                        bar: bar.max(1),
                        beat: c["beat"].as_u64().unwrap_or(1).max(1) as u32,
                        bpm: bpm.clamp(10.0, 999.0),
                        glide: c["glide"].as_bool().unwrap_or(false),
                    });
                }
                v.sort_by_key(|c| (c.bar, c.beat));
                p.tempo_changes = v;
            }
            if let Some(meters) = args["meters"].as_array() {
                let mut v: Vec<daw_project::MeterChange> = Vec::new();
                for m in meters {
                    let (Some(bar), Some(beats)) = (m["bar"].as_i64(), m["beats_per_bar"].as_u64())
                    else {
                        return Err(
                            "Each time signature change needs a bar and beats_per_bar.".into()
                        );
                    };
                    let unit = m["beat_unit"].as_u64().unwrap_or(4);
                    if ![1, 2, 4, 8, 16, 32].contains(&unit) {
                        return Err("A beat unit is 1, 2, 4, 8, 16 or 32.".into());
                    }
                    if bar < 2 || v.iter().any(|o| o.bar == bar) {
                        return Err(format!(
                            "A time signature change goes on a bar after the first, one to a \
                             bar (bar {bar})."
                        ));
                    }
                    v.push(daw_project::MeterChange {
                        id: daw_project::new_id(),
                        bar,
                        beats: (beats as u32).clamp(1, 32),
                        unit: unit as u32,
                    });
                }
                v.sort_by_key(|m| m.bar);
                p.meter_changes = v;
            }
            Ok(
                json!({"tempo_bpm": p.tempo_bpm, "beats_per_bar": p.beats_per_bar,
                      "beat_unit": p.beat_unit, "changes": p.tempo_changes.len(),
                      "meters": p.meter_changes.len()}),
            )
        })
        .map(|v| (v, Some("Change the tempo".into())))
    }

    fn set_key_tool(&mut self, args: &Value) -> Done {
        let scale = parse_key(text(args, "key", "the key")?)?;
        self.edit_model(|p| {
            p.scale = scale;
            Ok(json!({"key": scale.map(|s| s.label())}))
        })
        .map(|v| (v, Some("Set the key".into())))
    }

    fn set_loop_tool(&mut self, args: &Value) -> Done {
        let (from, to) = self.loop_bars;
        let from = args["from_bar"].as_i64().unwrap_or(from).max(1);
        let to = args["to_bar"].as_i64().unwrap_or(to);
        if to <= from {
            return Err("The loop ends on a later bar than it starts.".into());
        }
        let v = self.edit_model(|p| {
            p.loop_bars = (from, to);
            Ok(json!({"from_bar": from, "to_bar": to}))
        })?;
        if let Some(on) = args["on"].as_bool() {
            self.looping = on;
            self.send_loop();
        }
        Ok((
            json!({"from_bar": v["from_bar"], "to_bar": v["to_bar"], "on": self.looping}),
            Some("Set the loop".into()),
        ))
    }

    fn add_comment_tool(&mut self, args: &Value) -> Done {
        let text = text(args, "text", "the comment's text")?.trim().to_string();
        if text.is_empty() {
            return Err("The comment is empty.".into());
        }
        let mut start = (self.time_arg(args)).unwrap_or_else(|| self.ctl.status().position().0);
        let to = |bar: Option<f64>, secs: Option<f64>| {
            self.time_arg(&json!({"bar": bar, "seconds": secs}))
        };
        let mut end = to(args["to_bar"].as_f64(), args["to_seconds"].as_f64());
        if args["on_loop"].as_bool() == Some(true) {
            if !self.looping {
                return Err(
                    "The loop is off: turn it on (set_loop) to pin a comment on it.".into(),
                );
            }
            let map = self.tempo_map();
            start = map.frames_at(map.bar_start(self.loop_bars.0)).0;
            end = Some(map.frames_at(map.bar_start(self.loop_bars.1)).0);
        }
        if end.is_some_and(|e| e <= start) {
            return Err("A comment's end comes after its start.".into());
        }
        let track = args["track"].as_str().map(String::from);
        let parent = args["reply_to"].as_str().map(String::from);
        let author = self.my_author_id();
        self.edit_model(|p| {
            if let Some(t) = &track
                && !p.tracks.iter().any(|k| &k.id == t)
            {
                return Err(format!("There is no track {t}."));
            }
            if let Some(r) = &parent
                && !p.comments.iter().any(|c| &c.id == r && c.parent.is_none())
            {
                return Err(format!("There is no comment {r} to reply to."));
            }
            let id = daw_project::new_id();
            p.comments.push(daw_project::Comment {
                id: id.clone(),
                author,
                parent,
                track,
                start: start.max(0),
                end,
                text,
                resolved: false,
                created: now_ms(),
            });
            Ok(json!({"comment": id}))
        })
        .map(|v| (v, Some("Add a comment".into())))
    }

    fn edit_comment_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "comment", "the comment's id")?.to_string();
        let new = match args["text"].as_str().map(str::trim) {
            Some("") => return Err("The comment is empty.".into()),
            t => t.map(String::from),
        };
        let delete = args["delete"].as_bool() == Some(true);
        if new.is_none() && !delete {
            return Err("Give the new text, or delete: true.".into());
        }
        let me = self.my_author_id();
        let any = self.may_change_any_comment();
        self.edit_model(|p| {
            let c = (p.comments.iter_mut())
                .find(|c| c.id == id)
                .ok_or_else(|| format!("There is no comment {id}."))?;
            if !crate::comments::may_change(c, &me, any) {
                return Err("You may change only your own comments.".into());
            }
            if delete {
                crate::comments::delete(&mut p.comments, &id);
                return Ok(json!({"comment": id, "deleted": true}));
            }
            c.text = new.clone().unwrap_or_default();
            Ok(json!({"comment": id, "text": c.text}))
        })
        .map(|v| {
            let what = if delete {
                "Delete a comment"
            } else {
                "Edit a comment"
            };
            (v, Some(what.into()))
        })
    }

    fn resolve_comment_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "comment", "the comment's id")?.to_string();
        let resolved = args["resolved"].as_bool().unwrap_or(true);
        self.edit_model(|p| {
            let c = (p.comments.iter_mut())
                .find(|c| c.id == id && c.parent.is_none())
                .ok_or_else(|| format!("There is no comment {id}."))?;
            c.resolved = resolved;
            Ok(json!({"comment": id, "resolved": resolved}))
        })
        .map(|v| (v, Some("Resolve a comment".into())))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::parse_key;
    use crate::agent::driver::Driver;

    fn id(v: Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    #[test]
    fn keys_are_read_from_words() {
        assert_eq!(parse_key("A minor").unwrap().unwrap().label(), "A minor");
        assert_eq!(parse_key("bb major").unwrap().unwrap().label(), "A# major");
        assert_eq!(
            parse_key("F# harmonic minor").unwrap().unwrap().label(),
            "F# harmonic minor"
        );
        assert_eq!(parse_key("none").unwrap(), None);
        assert!(parse_key("H minor").is_err());
        assert!(parse_key("C").is_err());
    }

    #[test]
    fn notes_are_written_into_a_new_clip_and_taken_out_again() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        let clip = id(
            d.call("add_note_clip", json!({"track": t, "bar": 2, "bars": 2}))
                .unwrap(),
            "clip",
        );
        let added = d
            .call(
                "edit_notes",
                json!({"clip": clip, "add": [
                    {"beat": 0, "beats": 1, "pitch": 60},
                    {"beat": 1, "beats": 0.5, "pitch": 64, "velocity": 80}
                ]}),
            )
            .unwrap();
        let p = d.call("get_project", json!({"notes": true})).unwrap();
        let c = &p["tracks"][0]["note_clips"][0];
        // Bar 2 at 4/4 starts four beats in.
        assert_eq!(c["start_ticks"], 4 * 960);
        assert_eq!(c["len_ticks"], 8 * 960);
        assert_eq!(c["notes"][1]["start_ticks"], 960);
        assert_eq!(c["notes"][1]["len_ticks"], 480);
        assert_eq!(c["notes"][1]["velocity"], 80);
        let first = added["added"][0].as_str().unwrap();
        assert_eq!(c["notes"][0]["id"], first);

        d.call("edit_notes", json!({"clip": clip, "remove": [first]}))
            .unwrap();
        let p = d.call("get_project", json!({"notes": true})).unwrap();
        assert_eq!(
            p["tracks"][0]["note_clips"][0]["notes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        d.command("undo");
        let p = d.call("get_project", json!({"notes": true})).unwrap();
        assert_eq!(
            p["tracks"][0]["note_clips"][0]["notes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let audio = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        assert!(
            d.call("add_note_clip", json!({"track": audio, "bar": 1}))
                .is_err()
        );
        d.call("delete_clip", json!({"clip": clip})).unwrap();
        assert_eq!(d.project()["tracks"][0]["note_clips"], json!([]));
    }

    #[test]
    fn a_file_comes_in_and_its_clip_is_moved_trimmed_and_faded() {
        let dir = std::env::temp_dir().join(format!("daw-arrange-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("two-seconds.wav");
        let mut d = Driver::new();
        let rate = d.app().rate;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..2 * rate {
            w.write_sample(((i % 100) as i16 - 50) * 10).unwrap();
        }
        w.finalize().unwrap();
        assert!(
            d.call("import_audio", json!({"path": dir.join("none.wav")}))
                .is_err()
        );
        d.call("import_audio", json!({"path": path, "seconds": 1.0}))
            .unwrap();
        let mut p = d.project();
        for _ in 0..500 {
            if !p["tracks"].as_array().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            p = d.project();
        }
        let clip = p["tracks"][0]["clips"][0].clone();
        assert_eq!(clip["start_seconds"], 1.0);
        let c = clip["id"].as_str().unwrap().to_string();
        let other = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );

        d.call(
            "edit_clip",
            json!({"clip": c, "bar": 2, "track": other, "len_seconds": 1.5,
                   "fade_in_seconds": 0.1, "gain_db": -3, "muted": true}),
        )
        .unwrap();
        let p = d.project();
        assert_eq!(p["tracks"][0]["clips"], json!([]));
        let moved = &p["tracks"][1]["clips"][0];
        // Bar 2 at 120 bpm in 4/4 is two seconds in.
        assert_eq!(moved["start_seconds"], 2.0);
        assert_eq!(moved["len_seconds"], 1.5);
        assert_eq!(
            (moved["gain_db"].clone(), moved["muted"].clone()),
            (json!(-3.0), json!(true))
        );
        d.command("undo");
        assert_eq!(d.project()["tracks"][0]["clips"][0]["start_seconds"], 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markers_tempo_key_and_loop_are_set() {
        let mut d = Driver::new();
        let r = d
            .call(
                "edit_markers",
                json!({"add": [{"bar": 9, "name": "Chorus"}, {"bar": 1, "name": "Verse"}]}),
            )
            .unwrap();
        let chorus = r["added"][0].as_str().unwrap().to_string();
        d.call(
            "edit_markers",
            json!({"change": [{"id": chorus, "bar": 17}]}),
        )
        .unwrap();
        d.call(
            "set_tempo",
            json!({"bpm": 90, "beats_per_bar": 3, "changes": [{"bar": 5, "bpm": 120}]}),
        )
        .unwrap();
        d.call("set_key", json!({"key": "E minor"})).unwrap();
        d.call("set_loop", json!({"from_bar": 2, "to_bar": 4, "on": true}))
            .unwrap();
        let p = d.project();
        let names: Vec<_> = (p["markers"].as_array().unwrap().iter())
            .map(|m| (m["bar"].clone(), m["name"].clone()))
            .collect();
        assert_eq!(
            names,
            [(json!(1), json!("Verse")), (json!(17), json!("Chorus"))]
        );
        assert_eq!(
            (p["tempo_bpm"].clone(), p["beats_per_bar"].clone()),
            (json!(90.0), json!(3))
        );
        assert_eq!(p["tempo_changes"][0]["bpm"], 120.0);
        d.call(
            "set_tempo",
            json!({"meters": [{"bar": 9, "beats_per_bar": 7, "beat_unit": 8}]}),
        )
        .unwrap();
        assert_eq!(
            d.project()["meter_changes"],
            json!([{"bar": 9, "beats_per_bar": 7, "beat_unit": 8}])
        );
        assert!(
            d.call(
                "set_tempo",
                json!({"meters": [{"bar": 1, "beats_per_bar": 3}]})
            )
            .is_err()
        );
        assert_eq!(p["scale"], "E minor");
        assert_eq!(p["loop_bars"], json!([2, 4]));
        assert_eq!(d.call("get_transport", json!({})).unwrap()["loop"], true);
        assert!(d.call("set_tempo", json!({"bpm": 5000})).is_err());
        assert!(
            d.call("set_loop", json!({"from_bar": 4, "to_bar": 2}))
                .is_err()
        );
        d.call("edit_markers", json!({"remove": [chorus]})).unwrap();
        assert_eq!(d.project()["markers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn comments_are_added_replied_to_and_resolved() {
        let mut d = Driver::new();
        let c = id(
            d.call("add_comment", json!({"text": "Too loud here", "bar": 3}))
                .unwrap(),
            "comment",
        );
        d.call(
            "add_comment",
            json!({"text": "Turned it down", "reply_to": c}),
        )
        .unwrap();
        d.call("resolve_comment", json!({"comment": c})).unwrap();
        let list = d.call("get_comments", json!({})).unwrap();
        let first = &list["comments"][0];
        assert_eq!(
            (first["text"].clone(), first["bar"].clone()),
            (json!("Too loud here"), json!(3))
        );
        assert_eq!(first["resolved"], true);
        assert_eq!(first["replies"][0]["text"], "Turned it down");
        assert!(d.call("add_comment", json!({"text": " "})).is_err());
    }

    #[test]
    fn comments_span_a_stretch_and_are_edited_and_deleted() {
        let mut d = Driver::new();
        let c = id(
            d.call(
                "add_comment",
                json!({"text": "Drums drag", "bar": 2, "to_bar": 4}),
            )
            .unwrap(),
            "comment",
        );
        let r = id(
            d.call("add_comment", json!({"text": "Agreed", "reply_to": c}))
                .unwrap(),
            "comment",
        );
        let list = d.call("get_comments", json!({})).unwrap();
        assert!(list["comments"][0]["end_seconds"].as_f64().unwrap() > 0.0);
        assert!(
            d.call("add_comment", json!({"text": "x", "bar": 4, "to_bar": 2}))
                .is_err(),
            "the end comes after the start"
        );
        assert!(
            d.call("add_comment", json!({"text": "x", "on_loop": true}))
                .is_err(),
            "the loop is off"
        );
        d.call("set_loop", json!({"from_bar": 5, "to_bar": 7, "on": true}))
            .unwrap();
        let l = id(
            d.call("add_comment", json!({"text": "Loop this", "on_loop": true}))
                .unwrap(),
            "comment",
        );

        d.call(
            "edit_comment",
            json!({"comment": c, "text": "Drums drag a bit"}),
        )
        .unwrap();
        let list = d.call("get_comments", json!({})).unwrap();
        assert_eq!(list["comments"][0]["text"], "Drums drag a bit");
        assert_eq!(list["comments"][1]["text"], "Loop this");
        assert_eq!(list["comments"][1]["bar"], 5);
        assert!(
            d.call("edit_comment", json!({"comment": c, "text": " "}))
                .is_err()
        );
        assert!(
            d.call("edit_comment", json!({"comment": "nope", "delete": true}))
                .is_err()
        );

        // Deleting a thread takes its replies with it.
        d.call("edit_comment", json!({"comment": c, "delete": true}))
            .unwrap();
        let p = d.app().proj.doc.get();
        assert!(p.comments.iter().all(|k| k.id != c && k.id != r));
        assert!(p.comments.iter().any(|k| k.id == l));
    }
}
