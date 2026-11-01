// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for what the timeline, note editor and comments
//! panes do by hand (roadmap 2.11): clips, notes, markers, tempo, key,
//! the loop and comments. Each is one undo step, as every edit by an
//! assistant is (see `agent.rs`); they edit the project as the document
//! has it, as `agent_edit.rs` does.
//!
//! Times are given as a bar (counted from 1, fractions within it) or in
//! seconds; notes in beats (quarter notes) from their clip's start.

use daw_engine::time::{Frames, PPQ, TempoMap, Ticks};
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
        description: "Changes an audio clip: where it starts (bar or seconds), the track \
                      it is on, its length or offset into its file (seconds), its gain, \
                      fades (seconds), mute or name. Only what is given changes.",
        access: Access::Edit,
        schema: || {
            with_time(
                json!({
                    "clip": {"type": "string"},
                    "track": {"type": "string", "description": "Move it to this track."},
                    "len_seconds": {"type": "number"},
                    "offset_seconds": {"type": "number"},
                    "gain_db": {"type": "number"},
                    "fade_in_seconds": {"type": "number"},
                    "fade_out_seconds": {"type": "number"},
                    "muted": {"type": "boolean"},
                    "name": {"type": "string"}
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
        name: "add_note_clip",
        description: "Adds an empty note clip to an instrument track, starting on a bar, \
                      some bars long. Gives its id; edit_notes fills it.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "bar": {"type": "integer", "description": "From 1."},
                "bars": {"type": "integer", "description": "How long; 1 if left out."},
                "name": {"type": "string"}
            }, "required": ["track", "bar"]})
        },
    },
    Tool {
        name: "edit_notes",
        description: "Adds notes to a note clip, removes some by id, or clears it first. \
                      Notes are placed in beats (quarter notes) from the clip's start; \
                      pitch is a MIDI number (60 is middle C). get_project with notes \
                      gives their ids.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clip": {"type": "string"},
                "clear": {"type": "boolean"},
                "remove": {"type": "array", "items": {"type": "string"}},
                "add": {"type": "array", "items": {"type": "object", "properties": {
                    "beat": {"type": "number"},
                    "beats": {"type": "number", "description": "How long."},
                    "pitch": {"type": "integer"},
                    "velocity": {"type": "integer", "description": "1 to 127; 100 if left out."}
                }, "required": ["beat", "beats", "pitch"]}}
            }, "required": ["clip"]})
        },
    },
    Tool {
        name: "edit_markers",
        description: "Adds section markers on bars, renames or moves them by id, or \
                      removes them.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "add": {"type": "array", "items": {"type": "object", "properties": {
                    "bar": {"type": "integer"}, "name": {"type": "string"}
                }, "required": ["bar", "name"]}},
                "change": {"type": "array", "items": {"type": "object", "properties": {
                    "id": {"type": "string"}, "bar": {"type": "integer"}, "name": {"type": "string"}
                }, "required": ["id"]}},
                "remove": {"type": "array", "items": {"type": "string"}}
            }})
        },
    },
    Tool {
        name: "set_tempo",
        description: "Sets the song's starting tempo and time signature, and, if given, \
                      the tempo changes after the start (replacing those there). Clips \
                      that follow tempo move with it; others stay where they are.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "bpm": {"type": "number"},
                "beats_per_bar": {"type": "integer"},
                "beat_unit": {"type": "integer", "description": "4 for quarter notes, 8 for eighths."},
                "changes": {"type": "array", "items": {"type": "object", "properties": {
                    "bar": {"type": "integer"}, "beat": {"type": "integer"},
                    "bpm": {"type": "number"}, "glide": {"type": "boolean"}
                }, "required": ["bar", "bpm"]}}
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
                      playhead if left out), or replies to a comment.",
        access: Access::Edit,
        schema: || {
            with_time(
                json!({
                    "text": {"type": "string"},
                    "track": {"type": "string"},
                    "reply_to": {"type": "string", "description": "A comment's id."}
                }),
                &["text"],
            )
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
fn parse_key(s: &str) -> Result<Option<Scale>, String> {
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
            "add_note_clip" => self.add_note_clip_tool(args),
            "edit_notes" => self.edit_notes_tool(args),
            "edit_markers" => self.edit_markers_tool(args),
            "set_tempo" => self.set_tempo_tool(args),
            "set_key" => self.set_key_tool(args),
            "set_loop" => self.set_loop_tool(args),
            "add_comment" => self.add_comment_tool(args),
            "resolve_comment" => self.resolve_comment_tool(args),
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
        let start = self.time_arg(args);
        let rate = self.rate as f64;
        let map = self.tempo_map();
        let frames = |k: &str| args[k].as_f64().map(|s| (s.max(0.0) * rate).round() as i64);
        self.edit_model(|p| {
            let Found::Audio(mut i, j) = find_clip(p, &id)? else {
                return Err(format!(
                    "{id} is a note clip; edit_notes changes its notes, delete_clip deletes it."
                ));
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
            if let Some(s) = start {
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
            Ok(json!({"clip": id, "start_seconds": c.start as f64 / rate,
                      "len_seconds": c.len as f64 / rate}))
        })
        .map(|v| (v, Some("Change a clip".into())))
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
            t.note_clips.push(daw_project::NoteClip::new(
                id.clone(),
                name,
                start,
                end - start,
            ));
            t.note_clips.sort_by_key(|c| c.start);
            Ok(json!({"clip": id, "start_ticks": start, "len_ticks": end - start}))
        })
        .map(|v| (v, Some("Add a note clip".into())))
    }

    fn edit_notes_tool(&mut self, args: &Value) -> Done {
        let id = text(args, "clip", "the clip's id")?.to_string();
        let mut add = Vec::new();
        for n in args["add"].as_array().into_iter().flatten() {
            let (Some(beat), Some(beats), Some(pitch)) =
                (n["beat"].as_f64(), n["beats"].as_f64(), n["pitch"].as_u64())
            else {
                return Err("Each note needs a beat, beats (how long) and a pitch.".into());
            };
            if pitch > 127 {
                return Err(format!("Pitch {pitch} is past 127."));
            }
            let ticks = |b: f64| (b * PPQ as f64).round() as i64;
            let velocity = n["velocity"].as_u64().unwrap_or(100).clamp(1, 127) as u8;
            add.push(daw_project::Note::new(
                ticks(beat.max(0.0)),
                ticks(beats).max(1),
                pitch as u8,
                velocity,
            ));
        }
        let remove: Vec<String> = (args["remove"].as_array().into_iter().flatten())
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        let clear = args["clear"].as_bool() == Some(true);
        self.edit_model(|p| {
            let Found::Notes(i, j) = find_clip(p, &id)? else {
                return Err(format!("{id} is an audio clip, which has no notes."));
            };
            let c = &mut p.tracks[i].note_clips[j];
            if c.pattern.is_some() {
                return Err("The clip plays a pattern; its notes come from the pattern.".into());
            }
            if clear {
                c.notes.clear();
            }
            let before = c.notes.len();
            c.notes.retain(|n| !remove.contains(&n.id));
            let removed = before - c.notes.len();
            let added: Vec<String> = add.iter().map(|n| n.id.clone()).collect();
            c.notes.extend(add);
            c.notes.sort_by_key(|n| (n.start, n.pitch));
            Ok(json!({"added": added, "removed": removed, "notes": c.notes.len()}))
        })
        .map(|v| (v, Some("Edit notes".into())))
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
            Ok(
                json!({"tempo_bpm": p.tempo_bpm, "beats_per_bar": p.beats_per_bar,
                      "beat_unit": p.beat_unit, "changes": p.tempo_changes.len()}),
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
        let start = (self.time_arg(args)).unwrap_or_else(|| self.ctl.status().position().0);
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
                end: None,
                text,
                resolved: false,
                created: now_ms(),
            });
            Ok(json!({"comment": id}))
        })
        .map(|v| (v, Some("Add a comment".into())))
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
}
