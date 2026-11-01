// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for takes, timing and clip processing (roadmap
//! 2.11): comping, fix timing, clips that follow tempo and their stretch
//! markers, tempo from a performance, freezing and rendering a track,
//! normalising, reversing and changing the gain of clips, a clip's fade
//! shapes and volume line, edit groups, snapshots, and a track's input,
//! arming and monitoring.
//!
//! Several of these act on the timeline's selection when a person uses
//! them; the tools take the clips instead, select them for the moment,
//! and put the person's selection back afterwards. Each edit is one
//! undo step, as every edit by an assistant is (see `agent.rs`).
//!
//! Times are given in seconds on the timeline, or as a bar (from 1,
//! fractions within it); takes are counted from 1, as the Take lanes
//! pane shows them.

use std::collections::HashSet;

use daw_engine::fade::Curve;
use daw_engine::process::Process;
use daw_engine::time::{TempoMap, Ticks};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::comping;
use crate::stretch::{Quantise, TempoFrom, Unit};
use crate::timeline::{self, Grid};

type Done = Result<(Value, Option<String>), String>;

const GRIDS: &str = "bar, beat, 1/2, 1/3, 1/4, 1/6 or 1/8 (of a beat; 1/3 and 1/6 are triplets)";

fn clips_prop() -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": "Audio clips' ids."})
}

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_takes",
        description: "A track's recorded takes (counted from 1) with their clips, its \
                      comps and the parts of each (which take plays from when to when), \
                      the comp that plays, and the take that plays when no comp does.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"}
            }, "required": ["track"]})
        },
    },
    Tool {
        name: "edit_comp",
        description: "Comps a track's takes, as the Take lanes pane does. new: a comp of \
                      what plays now (or, if a comp plays, of every take, newest on top). \
                      duplicate: a copy of the comp that plays. choose: play a comp by \
                      id, or the takes as recorded without one. rename, delete. flatten: \
                      keep the comp that plays as plain clips and let the takes go. swipe: \
                      take n plays from one time to another (starting a comp if none \
                      plays). clear: nothing plays there. use_take: take n plays in the \
                      part of the comp at a time. choose_take: play take n where it was \
                      recorded, leaving the comp (which is kept). Answers as get_takes.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "action": {"type": "string", "enum": ["new", "duplicate", "choose", "rename",
                           "delete", "flatten", "swipe", "clear", "use_take", "choose_take"]},
                "comp": {"type": "string", "description": "A comp's id: for choose, rename and delete."},
                "name": {"type": "string", "description": "For rename."},
                "take": {"type": "integer", "description": "From 1."},
                "from_seconds": {"type": "number"},
                "from_bar": {"type": "number", "description": "From 1; 2.5 is halfway through bar 2."},
                "to_seconds": {"type": "number"},
                "to_bar": {"type": "number"},
                "at_seconds": {"type": "number", "description": "For use_take."},
                "at_bar": {"type": "number"}
            }, "required": ["track", "action"]})
        },
    },
    Tool {
        name: "fix_timing",
        description: "Fix timing: finds the notes in audio clips and moves them toward a \
                      grid, cutting the clips at the notes, without stretching. Grouped \
                      tracks (with group editing on) get the same cuts. Give preview to \
                      see what it would do without doing it. Answers how many notes it \
                      found, how far they move, and the new pieces.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clips": clips_prop(),
                "grid": {"type": "string", "description": GRIDS},
                "strength": {"type": "number", "description": "0 leaves notes where they are, 1 puts them on the grid."},
                "sensitivity": {"type": "number", "description": "0 to 1: higher finds softer notes."},
                "guide": {"type": "string", "description": "In a group, find the notes on this track only (an id); all its tracks if left out."},
                "preview": {"type": "boolean"}
            }, "required": ["clips"]})
        },
    },
    Tool {
        name: "stretch_clips",
        description: "Audio that follows tempo, and its stretch markers. Done in this \
                      order, each if given: follow_tempo on or off; the clips' own tempo \
                      (in bpm, the tempo they were played at; makes them follow tempo); \
                      clear their markers; put a marker on every note found; add markers \
                      at times; move markers (by index, as the answer lists them) to \
                      times; remove markers by index; quantise (move the markers' notes \
                      toward a grid, finding notes first where there are no markers). \
                      Times are seconds on the timeline. Answers each clip's tempo and \
                      markers.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clips": clips_prop(),
                "follow_tempo": {"type": "boolean"},
                "tempo": {"type": "number", "description": "20 to 400 bpm."},
                "clear_markers": {"type": "boolean"},
                "markers_from_notes": {"type": "boolean"},
                "sensitivity": {"type": "number", "description": "0 to 1, for finding notes; 0.5 if left out."},
                "add_markers": {"type": "array", "items": {"type": "number"}, "description": "Seconds on the timeline."},
                "move_markers": {"type": "array", "items": {"type": "object", "properties": {
                    "index": {"type": "integer"}, "to_seconds": {"type": "number"}
                }, "required": ["index", "to_seconds"]}},
                "remove_markers": {"type": "array", "items": {"type": "integer"}},
                "quantise": {"type": "object", "properties": {
                    "grid": {"type": "string", "description": GRIDS},
                    "strength": {"type": "number", "description": "0 to 1."},
                    "window_ms": {"type": "number", "description": "Notes further from the grid than this stay put; 60 if left out."}
                }}
            }, "required": ["clips"]})
        },
    },
    Tool {
        name: "tempo_from_clip",
        description: "Sets the song's tempo map from how a performance was played: a \
                      tempo change on each beat (or bar) the clip's notes (or its stretch \
                      markers) fall on. Replaces the tempo changes there were; the clip \
                      stops following tempo, since it sets it.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clip": {"type": "string"},
                "unit": {"type": "string", "enum": ["beat", "bar"], "description": "What each note is: beat if left out."},
                "sensitivity": {"type": "number", "description": "0 to 1; 0.5 if left out."},
                "use_markers": {"type": "boolean", "description": "Take its stretch markers as the beats, if it has any; true if left out."}
            }, "required": ["clip"]})
        },
    },
    Tool {
        name: "freeze_track",
        description: "Freezes a track (it plays audio rendered through its effects, with \
                      them switched off, to save computer power), or unfreezes it. The \
                      frozen audio is made once the project has been saved.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "frozen": {"type": "boolean"}
            }, "required": ["track", "frozen"]})
        },
    },
    Tool {
        name: "render_track",
        description: "Renders a track (a bus or folder with what goes into it) through \
                      its effects, before its fader, to a new track beside it, and mutes \
                      the original. With clips given, renders just those clips (bounce). \
                      Runs in the background: the new track comes when it is done.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "clips": clips_prop()
            }})
        },
    },
    Tool {
        name: "process_clips",
        description: "Normalises, reverses or changes the gain of audio clips: each \
                      clip's audio is processed into a new file in the background, and \
                      the clip then plays it. The original file is kept, so undo brings it \
                      back. get_project shows the clips once done.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clips": clips_prop(),
                "how": {"type": "string", "enum": ["normalise", "reverse", "gain"]},
                "gain_db": {"type": "number", "description": "For gain."}
            }, "required": ["clips", "how"]})
        },
    },
    Tool {
        name: "edit_clip_volume",
        description: "An audio clip's fade shapes and its volume line. The points replace \
                      the line's points: each a time in seconds from the clip's start and \
                      a level in dB (-48 to 12); the line stays with the audio when the \
                      clip is trimmed. Fade lengths are set with edit_clip.",
        access: Access::Edit,
        schema: || {
            let curves = json!(["linear", "equal-power", "s-curve", "fast", "slow"]);
            json!({"type": "object", "properties": {
                "clip": {"type": "string"},
                "fade_in_curve": {"type": "string", "enum": curves},
                "fade_out_curve": {"type": "string", "enum": curves},
                "points": {"type": "array", "items": {"type": "object", "properties": {
                    "seconds": {"type": "number"}, "db": {"type": "number"}
                }, "required": ["seconds", "db"]}},
                "clear_points": {"type": "boolean"}
            }, "required": ["clip"]})
        },
    },
    Tool {
        name: "edit_groups",
        description: "Edit groups: clips on a group's tracks are selected, moved, split, \
                      trimmed and faded together. Adds groups of tracks, changes a group's \
                      name, tracks or whether it is on, removes groups, and turns group \
                      editing on or off. A track is in one group at most. Answers the \
                      groups.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "add": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"},
                    "tracks": {"type": "array", "items": {"type": "string"}}
                }}},
                "change": {"type": "array", "items": {"type": "object", "properties": {
                    "id": {"type": "string"},
                    "name": {"type": "string"},
                    "tracks": {"type": "array", "items": {"type": "string"}, "description": "Its tracks, all of them."},
                    "on": {"type": "boolean"}
                }, "required": ["id"]}},
                "remove": {"type": "array", "items": {"type": "string"}},
                "group_editing": {"type": "boolean", "description": "Off, every clip is edited on its own."}
            }})
        },
    },
    Tool {
        name: "list_snapshots",
        description: "The project's snapshots: named copies of the project to come back \
                      to, with when and by whom each was taken.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "edit_snapshot",
        description: "Takes a snapshot of the project as it is now (with a name), \
                      restores one (an ordinary edit, which undo takes back), renames one \
                      or deletes one.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "action": {"type": "string", "enum": ["take", "restore", "rename", "delete"]},
                "snapshot": {"type": "string", "description": "Its id, for all but take."},
                "name": {"type": "string"}
            }, "required": ["action"]})
        },
    },
    Tool {
        name: "set_track_input",
        description: "A track's recording settings on this computer (not saved with the \
                      project): armed to record, monitoring its input, which device input \
                      it records from (a channel from 1, stereo taking that and the next), \
                      and for an instrument track the MIDI output its notes also go to. \
                      Only what is given changes. Answers the settings, the device's \
                      input count and the MIDI outputs there are.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "arm": {"type": "boolean"},
                "monitor": {"type": "boolean"},
                "input": {"type": "integer", "description": "A device input channel, from 1; 0 for none."},
                "stereo": {"type": "boolean"},
                "midi_out": {"type": "string", "description": "An output's name, or \"none\"."}
            }, "required": ["track"]})
        },
    },
];

fn text<'a>(args: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("Give {what}."))
}

/// A grid by name, if one is given.
fn grid_arg(v: &Value) -> Result<Option<Grid>, String> {
    let Some(s) = v.as_str() else {
        return Ok(None);
    };
    Ok(Some(match s.trim() {
        "bar" => Grid::Bar,
        "beat" => Grid::Beat,
        "1/2" => Grid::Part(2),
        "1/3" => Grid::Part(3),
        "1/4" => Grid::Part(4),
        "1/6" => Grid::Part(6),
        "1/8" => Grid::Part(8),
        _ => return Err(format!("There is no grid {s}; give {GRIDS}.")),
    }))
}

fn grid_name(g: Grid) -> String {
    match g {
        Grid::Bar => "bar".into(),
        Grid::Beat | Grid::Auto => "beat".into(),
        Grid::Part(n) => format!("1/{n}"),
    }
}

fn curve_arg(v: &Value) -> Result<Option<Curve>, String> {
    let Some(s) = v.as_str() else {
        return Ok(None);
    };
    Ok(Some(match s {
        "linear" => Curve::Linear,
        "equal-power" => Curve::EqualPower,
        "s-curve" => Curve::SCurve,
        "fast" => Curve::Fast,
        "slow" => Curve::Slow,
        _ => return Err(format!("There is no fade shape {s}.")),
    }))
}

fn curve_name(c: Curve) -> &'static str {
    match c {
        Curve::Linear => "linear",
        Curve::EqualPower => "equal-power",
        Curve::SCurve => "s-curve",
        Curve::Fast => "fast",
        Curve::Slow => "slow",
    }
}

/// A 0-to-1 setting, if given.
fn unit_arg(args: &Value, key: &str) -> Result<Option<f64>, String> {
    match args[key].as_f64() {
        None => Ok(None),
        Some(x) if (0.0..=1.0).contains(&x) => Ok(Some(x)),
        Some(_) => Err(format!("{key} is from 0 to 1.")),
    }
}

/// Frame of a bar, from 1, fractions within it.
fn bar_frames(map: &TempoMap, bar: f64) -> i64 {
    let bar = bar.max(1.0);
    let whole = bar.floor() as i64;
    let from = map.bar_start(whole);
    let len = map.bar_start(whole + 1).0 - from.0;
    let t = Ticks(from.0 + ((bar - bar.floor()) * len as f64) as i64);
    map.frames_at(t).0
}

impl App {
    /// Runs one of the editing tools here, or `None` if `name` isn't one.
    pub(crate) fn takes_tool(
        &mut self,
        ctx: &eframe::egui::Context,
        name: &str,
        args: &Value,
    ) -> Option<Done> {
        Some(match name {
            "edit_comp" => self.edit_comp_tool(args),
            "fix_timing" => self.fix_timing_tool(args),
            "stretch_clips" => self.stretch_clips_tool(args),
            "tempo_from_clip" => self.tempo_from_clip_tool(args),
            "freeze_track" => self.freeze_track_tool(ctx, args),
            "render_track" => self.render_track_tool(args),
            "process_clips" => self.process_clips_tool(args),
            "edit_clip_volume" => self.edit_clip_volume_tool(args),
            "edit_groups" => self.edit_groups_tool(args),
            "edit_snapshot" => self.edit_snapshot_tool(ctx, args),
            "set_track_input" => self.set_track_input_tool(args),
            _ => return None,
        })
    }

    /// Answers one of the looking tools here, or `None` if `name` isn't one.
    pub(crate) fn takes_read(&mut self, name: &str, args: &Value) -> Option<Result<Value, String>> {
        Some(match name {
            "get_takes" => text(args, "track", "the track's id")
                .and_then(|k| self.track_at(k))
                .map(|i| self.takes_value(i)),
            "list_snapshots" => Ok(self.snapshots_value()),
            _ => return None,
        })
    }

    fn track_at(&self, key: &str) -> Result<usize, String> {
        self.track_index(key)
            .ok_or_else(|| format!("There is no track {key}."))
    }

    /// The frame a time is at: `{p}seconds` or `{p}bar`, if given.
    fn frame_arg(&self, args: &Value, p: &str) -> Option<i64> {
        match (
            args[format!("{p}bar")].as_f64(),
            args[format!("{p}seconds")].as_f64(),
        ) {
            (Some(bar), _) => Some(bar_frames(&self.tempo_map(), bar)),
            (None, Some(s)) => Some((s.max(0.0) * self.rate as f64).round() as i64),
            (None, None) => None,
        }
    }

    fn seconds(&self, f: i64) -> f64 {
        f as f64 / self.rate.max(1) as f64
    }

    /// The audio clip with this id: its track and place there.
    fn audio_clip(&self, key: &str) -> Result<(usize, usize), String> {
        for (i, t) in self.tracks.iter().enumerate() {
            if let Some(j) = t.clips.iter().position(|c| c.key == key) {
                return Ok((i, j));
            }
        }
        if (self.tracks.iter().flat_map(|t| &t.note_clips)).any(|c| c.id == key) {
            return Err(format!("{key} is a note clip; this works on audio clips."));
        }
        Err(format!("There is no clip {key}."))
    }

    /// The audio clips a call names, by their ids for the timeline.
    fn clip_ids(&self, args: &Value) -> Result<HashSet<u64>, String> {
        let list = args["clips"].as_array().ok_or("Give a list of clip ids.")?;
        if list.is_empty() {
            return Err("Give at least one clip.".into());
        }
        let mut ids = HashSet::new();
        for k in list {
            let k = k.as_str().ok_or("Clip ids are text.")?;
            let (i, j) = self.audio_clip(k)?;
            ids.insert(self.tracks[i].clips[j].id);
        }
        Ok(ids)
    }

    /// Runs `f` with `ids` selected, then puts the selection back as it
    /// was, less any clips gone.
    fn with_selected<R>(&mut self, ids: HashSet<u64>, f: impl FnOnce(&mut App) -> R) -> R {
        let before = std::mem::replace(&mut self.timeline.selected, ids);
        let r = f(self);
        let now: HashSet<u64> = (self.tracks.iter().flat_map(|t| &t.clips))
            .map(|c| c.id)
            .collect();
        self.timeline.selected = before.into_iter().filter(|i| now.contains(i)).collect();
        r
    }

    fn takes_value(&self, i: usize) -> Value {
        let t = &self.tracks[i];
        let takes: Vec<Value> = (comping::takes(&t.clips).into_iter())
            .map(|n| {
                let clips: Vec<Value> = (t.clips.iter())
                    .filter(|c| !c.comp && c.take == Some(n))
                    .map(|c| {
                        json!({"clip": c.key, "from_seconds": self.seconds(c.start),
                               "to_seconds": self.seconds(c.end()), "muted": c.muted})
                    })
                    .collect();
                json!({"take": n + 1, "clips": clips})
            })
            .collect();
        let playing = t.comp.as_deref();
        let part = |p: &daw_project::CompPart| {
            json!({"take": p.take + 1, "from_seconds": self.seconds(p.start),
                   "to_seconds": self.seconds(p.end)})
        };
        let comps: Vec<Value> = (t.comps.iter())
            .map(|c| {
                let on = playing == Some(c.id.as_str());
                // The one playing reads its parts from its pieces, which
                // may have been edited on the timeline.
                let parts = if on {
                    comping::parts_of(&t.clips)
                } else {
                    c.parts.clone()
                };
                json!({"id": c.id, "name": c.name, "playing": on,
                       "parts": parts.iter().map(part).collect::<Vec<_>>()})
            })
            .collect();
        let take = (playing.is_none())
            .then(|| timeline::active_take(&t.clips))
            .flatten();
        json!({
            "track": t.key,
            "takes": takes,
            "comps": comps,
            "comp_playing": playing,
            "take_playing": take.map(|n| n + 1),
        })
    }

    fn edit_comp_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?;
        let ti = self.track_at(key)?;
        let action = text(args, "action", "what to do")?;
        if !self.may_edit_song() {
            return Err("This project can be commented on, not changed.".into());
        }
        let have = comping::takes(&self.tracks[ti].clips);
        let comp = |app: &App| -> Result<usize, String> {
            let id = text(args, "comp", "the comp's id")?;
            (app.tracks[ti].comps.iter())
                .position(|c| c.id == id)
                .ok_or_else(|| format!("The track has no comp {id}."))
        };
        let take = || -> Result<usize, String> {
            let n = args["take"].as_u64().ok_or("Give the take, from 1.")? as usize;
            (n >= 1 && have.contains(&(n - 1)))
                .then_some(n - 1)
                .ok_or_else(|| format!("The track has no take {n}."))
        };
        let range = |app: &App| -> Result<(i64, i64), String> {
            let a = app.frame_arg(args, "from_");
            let b = app.frame_arg(args, "to_");
            match (a, b) {
                (Some(a), Some(b)) if b > a => Ok((a, b)),
                _ => Err("Give a time it goes from and a later one it goes to.".into()),
            }
        };
        if have.is_empty() && !matches!(action, "rename" | "delete") {
            return Err(format!(
                "{} has no takes: record in a loop, or several times over, first.",
                self.tracks[ti].name
            ));
        }
        let rate = self.rate;
        let label = match action {
            "new" => {
                self.tracks[ti].takes(rate).new_comp();
                "Make a comp"
            }
            "duplicate" => {
                if self.tracks[ti].takes(rate).duplicate().is_none() {
                    return Err("No comp plays to copy.".into());
                }
                "Duplicate a comp"
            }
            "choose" => {
                let which = match args["comp"].is_null() {
                    true => None,
                    false => Some(comp(self)?),
                };
                self.tracks[ti].takes(rate).choose(which);
                "Choose a comp"
            }
            "rename" => {
                let i = comp(self)?;
                let name = text(args, "name", "the new name")?.trim();
                if name.is_empty() {
                    return Err("Give a name that isn't empty.".into());
                }
                self.tracks[ti].takes(rate).rename(i, name);
                "Rename a comp"
            }
            "delete" => {
                let i = comp(self)?;
                self.tracks[ti].takes(rate).delete(i);
                "Delete a comp"
            }
            "flatten" => {
                if !self.tracks[ti].takes(rate).flatten() {
                    return Err("No comp plays to flatten: choose one first.".into());
                }
                "Flatten a comp"
            }
            "swipe" => {
                let n = take()?;
                let (a, b) = range(self)?;
                self.tracks[ti].takes(rate).swipe(n, a, b);
                "Comp takes"
            }
            "clear" => {
                let (a, b) = range(self)?;
                if self.tracks[ti].comp.is_none() {
                    return Err("No comp plays to clear a part of.".into());
                }
                self.tracks[ti].takes(rate).clear(a, b);
                "Comp takes"
            }
            "use_take" => {
                let n = take()?;
                let at = self
                    .frame_arg(args, "at_")
                    .ok_or("Give the time of the part.")?;
                if !self.tracks[ti].takes(rate).use_at(n, at) {
                    return Err(format!("Take {} has nothing there.", n + 1));
                }
                "Comp takes"
            }
            "choose_take" => {
                let n = take()?;
                self.tracks[ti].takes(rate).choose_take(Some(n));
                "Choose a take"
            }
            other => return Err(format!("There is no comp action {other}.")),
        };
        // Pieces are made again: the old ones can't stay selected.
        let now: HashSet<u64> = (self.tracks.iter().flat_map(|t| &t.clips))
            .map(|c| c.id)
            .collect();
        self.timeline.selected.retain(|id| now.contains(id));
        self.rebuild();
        Ok((self.takes_value(ti), Some(label.into())))
    }

    fn fix_timing_tool(&mut self, args: &Value) -> Done {
        let ids = self.clip_ids(args)?;
        let mut s = self.timing.settings;
        if let Some(g) = grid_arg(&args["grid"])? {
            s.grid = g;
        }
        if let Some(x) = unit_arg(args, "strength")? {
            s.strength = x as f32;
        }
        if let Some(x) = unit_arg(args, "sensitivity")? {
            s.sensitivity = x as f32;
        }
        s.guide = match args["guide"].as_str() {
            Some(k) => Some(self.track_at(k)?),
            None => None,
        };
        if !self.may_edit_song() {
            return Err("This project can be commented on, not changed.".into());
        }
        let preview = args["preview"].as_bool() == Some(true);
        let keep = std::mem::replace(&mut self.timing.settings, s);
        let (plan, problem, pieces) = self.with_selected(ids, |app| {
            let plan = app.timing_plan();
            let problem = app.timing.message.take();
            let mut pieces = Vec::new();
            if !preview && !plan.clips.is_empty() {
                app.apply_timing(&plan);
                pieces = (app.tracks.iter().flat_map(|t| &t.clips))
                    .filter(|c| app.timeline.selected.contains(&c.id))
                    .map(|c| c.key.clone())
                    .collect();
            }
            (plan, problem, pieces)
        });
        self.timing.settings = keep;
        if let Some(e) = problem
            && plan.notes == 0
        {
            return Err(e);
        }
        let ms = |f: i64| f as f64 * 1000.0 / self.rate.max(1) as f64;
        let changed = !preview && !plan.clips.is_empty();
        Ok((
            json!({
                "notes_found": plan.notes,
                "clips_cut": plan.clips.len(),
                "most_moved_ms": ms(plan.most),
                "average_moved_ms": ms(plan.average),
                "comp_pieces_left_alone": plan.skipped_comp,
                "grid": grid_name(s.grid),
                "fixed": changed,
                "pieces": pieces,
            }),
            changed.then(|| "Fix timing".into()),
        ))
    }

    fn stretch_value(&self, ids: &HashSet<u64>) -> Value {
        let map = self.tempo_map();
        let clips: Vec<Value> = (self.tracks.iter().flat_map(|t| &t.clips))
            .filter(|c| ids.contains(&c.id))
            .map(|c| {
                let markers: Vec<Value> = (c.markers.iter().enumerate())
                    .map(|(i, m)| {
                        json!({"index": i, "frame": m.frame,
                               "seconds": self.seconds(c.out_of_frame(&map, m.frame as f64))})
                    })
                    .collect();
                json!({"clip": c.key, "follows_tempo": c.tempo.is_some(), "tempo": c.tempo,
                       "from_seconds": self.seconds(c.start), "to_seconds": self.seconds(c.end()),
                       "markers": markers})
            })
            .collect();
        json!({"clips": clips})
    }

    fn stretch_clips_tool(&mut self, args: &Value) -> Done {
        let ids = self.clip_ids(args)?;
        if !self.may_edit_song() {
            return Err("This project can be commented on, not changed.".into());
        }
        let tempo = args["tempo"].as_f64();
        if let Some(b) = tempo
            && !(20.0..=400.0).contains(&b)
        {
            return Err("A clip's tempo is from 20 to 400 bpm.".into());
        }
        let sensitivity = unit_arg(args, "sensitivity")?.unwrap_or(0.5) as f32;
        let quantise = match &args["quantise"] {
            Value::Null => None,
            q => {
                let d = Quantise::default();
                Some(Quantise {
                    grid: grid_arg(&q["grid"])?.unwrap_or(d.grid),
                    strength: unit_arg(q, "strength")?.unwrap_or(d.strength),
                    window_ms: q["window_ms"].as_f64().unwrap_or(d.window_ms).max(0.0),
                })
            }
        };
        let rate = self.rate as f64;
        let to_frame = |s: f64| (s.max(0.0) * rate).round() as i64;
        let add: Vec<i64> = (args["add_markers"].as_array().into_iter().flatten())
            .map(|v| v.as_f64().map(to_frame).ok_or("Marker times are numbers."))
            .collect::<Result<_, _>>()?;
        let moves: Vec<(usize, i64)> = (args["move_markers"].as_array().into_iter().flatten())
            .map(|m| match (m["index"].as_u64(), m["to_seconds"].as_f64()) {
                (Some(i), Some(s)) => Ok((i as usize, to_frame(s))),
                _ => Err("Each marker moved needs an index and to_seconds."),
            })
            .collect::<Result<_, _>>()?;
        let mut remove: Vec<usize> = (args["remove_markers"].as_array().into_iter().flatten())
            .map(|v| {
                v.as_u64()
                    .map(|i| i as usize)
                    .ok_or("Marker indexes are numbers.")
            })
            .collect::<Result<_, _>>()?;
        remove.sort_unstable();
        remove.dedup();
        let marked = !add.is_empty() || !moves.is_empty() || !remove.is_empty();
        if ids.len() > 1 && (!moves.is_empty() || !remove.is_empty()) {
            return Err("Move or remove markers on one clip at a time.".into());
        }
        let before = self.project();
        let keep_sens = self.stretch.sensitivity;
        let mut problem: Option<String> = None;
        self.with_selected(ids.clone(), |app| {
            if let Some(on) = args["follow_tempo"].as_bool() {
                let any_off = (app.tracks.iter().flat_map(|t| &t.clips))
                    .any(|c| ids.contains(&c.id) && c.tempo.is_none());
                let any_on = (app.tracks.iter().flat_map(|t| &t.clips))
                    .any(|c| ids.contains(&c.id) && c.tempo.is_some());
                // The command turns them all on if any is off, else off.
                if on && any_off {
                    app.toggle_follow_tempo();
                }
                if !on && any_on {
                    if any_off {
                        app.toggle_follow_tempo();
                    }
                    app.toggle_follow_tempo();
                }
            }
            if let Some(b) = tempo {
                app.set_clip_tempo(b);
            }
            if args["clear_markers"].as_bool() == Some(true) {
                app.clear_stretch_markers();
            }
            app.stretch.sensitivity = sensitivity;
            if args["markers_from_notes"].as_bool() == Some(true) {
                app.markers_from_attacks();
                problem = app.stretch.message.take();
            }
            if marked {
                let map = app.tempo_map();
                for c in (app.tracks.iter_mut().flat_map(|t| &mut t.clips))
                    .filter(|c| ids.contains(&c.id))
                {
                    for &at in &add {
                        if at <= c.start || at >= c.end() {
                            problem = Some(format!("{}s isn't inside the clip.", at as f64 / rate));
                            continue;
                        }
                        let frame = c.frame_of_out(&map, at).round() as i64;
                        c.add_marker(frame);
                        c.retime(&map);
                    }
                    for &(i, to) in &moves {
                        if i >= c.markers.len() {
                            problem = Some(format!("The clip has no marker {i}."));
                            continue;
                        }
                        let at = c.at_of_out(&map, to).round() as i64;
                        if !c.move_marker(i, at) {
                            problem = Some(format!(
                                "Marker {i} can't go to {}s: past its neighbours, or stretched \
                                 too far.",
                                to as f64 / rate
                            ));
                        }
                        c.retime(&map);
                    }
                    for &i in remove.iter().rev() {
                        if i < c.markers.len() {
                            c.markers.remove(i);
                        } else {
                            problem = Some(format!("The clip has no marker {i}."));
                        }
                    }
                    c.retime(&map);
                }
                app.rebuild();
            }
            if let Some(q) = quantise {
                let keep = std::mem::replace(&mut app.stretch.quantise, q);
                app.quantise_audio();
                problem = problem.take().or(app.stretch.message.take());
                app.stretch.quantise = keep;
            }
        });
        self.stretch.sensitivity = keep_sens;
        let changed = self.project() != before;
        if let Some(e) = &problem
            && !changed
        {
            return Err(e.clone());
        }
        let mut v = self.stretch_value(&ids);
        v["problem"] = json!(problem);
        Ok((v, changed.then(|| "Stretch clips".into())))
    }

    fn tempo_from_clip_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "clip", "the clip's id")?;
        let (i, j) = self.audio_clip(key)?;
        if !self.may_edit_song() {
            return Err("This project can be commented on, not changed.".into());
        }
        let d = TempoFrom::default();
        let settings = TempoFrom {
            unit: match args["unit"].as_str() {
                None | Some("beat") => Unit::Beat,
                Some("bar") => Unit::Bar,
                Some(u) => return Err(format!("There is no unit {u}: beat or bar.")),
            },
            sensitivity: unit_arg(args, "sensitivity")?.map_or(d.sensitivity, |x| x as f32),
            use_markers: args["use_markers"].as_bool().unwrap_or(d.use_markers),
        };
        let id = self.tracks[i].clips[j].id;
        let keep = std::mem::replace(&mut self.stretch.tempo_from, settings);
        let before = self.project();
        self.stretch.message = None;
        self.with_selected(HashSet::from([id]), |app| app.tempo_from_clip());
        self.stretch.tempo_from = keep;
        if let Some(e) = self.stretch.message.take() {
            return Err(e);
        }
        if self.project() == before {
            return Err("No tempo could be found from the clip.".into());
        }
        let p = self.project();
        Ok((
            json!({"tempo_bpm": p.tempo_bpm, "tempo_changes": p.tempo_changes.len()}),
            Some("Tempo from a clip".into()),
        ))
    }

    fn freeze_track_tool(&mut self, ctx: &eframe::egui::Context, args: &Value) -> Done {
        let i = self.track_at(text(args, "track", "the track's id")?)?;
        let on = args["frozen"]
            .as_bool()
            .ok_or("Say whether it is frozen.")?;
        if !self.may_edit_song() {
            return Err("This project can be commented on, not changed.".into());
        }
        let t = &self.tracks[i];
        if t.freeze == on {
            return Ok((json!({"track": t.key, "frozen": on}), None));
        }
        if on && !crate::render::can_freeze(t) {
            return Err(format!("{} has no effects switched on to freeze.", t.name));
        }
        self.message = None;
        // Freezing writes its own step, as the project is saved with it.
        let label = if on {
            "Freeze a track"
        } else {
            "Unfreeze a track"
        };
        self.proj.label = Some(format!("Assistant: {label}"));
        self.toggle_freeze(ctx, i);
        self.proj.label = None;
        Ok((
            json!({"track": self.tracks[i].key, "frozen": self.tracks[i].freeze,
                   "message": self.message.take()}),
            None,
        ))
    }

    fn render_track_tool(&mut self, args: &Value) -> Done {
        let busy = self.renders.busy();
        self.message = None;
        if !args["clips"].is_null() {
            let ids = self.clip_ids(args)?;
            self.with_selected(ids, |app| app.bounce_selection());
        } else {
            let i = self.track_at(text(args, "track", "the track's id, or clips")?)?;
            self.render_in_place(i);
        }
        if self.renders.busy() == busy {
            return Err(self
                .message
                .take()
                .unwrap_or("Nothing was rendered.".into()));
        }
        Ok((
            json!({"rendering": self.renders.busy(), "message": self.message.take()}),
            None,
        ))
    }

    fn process_clips_tool(&mut self, args: &Value) -> Done {
        let ids = self.clip_ids(args)?;
        let how = match text(args, "how", "how to process them")? {
            "normalise" => Process::Normalise,
            "reverse" => Process::Reverse,
            "gain" => {
                let db = args["gain_db"].as_f64().ok_or("Give gain_db.")?;
                if !(-48.0..=48.0).contains(&db) || db == 0.0 {
                    return Err("Give a gain from -48 to 48 dB, not 0.".into());
                }
                Process::Gain(db as f32)
            }
            other => return Err(format!("There is no processing {other}.")),
        };
        if !self.may_edit_song() {
            return Err("This project can be commented on, not changed.".into());
        }
        let n = self.processing.busy();
        self.processing.start(&self.tracks, &ids, how);
        Ok((
            json!({"processing": self.processing.busy() - n, "how": how.label()}),
            None,
        ))
    }

    fn edit_clip_volume_tool(&mut self, args: &Value) -> Done {
        let (i, j) = self.audio_clip(text(args, "clip", "the clip's id")?)?;
        let fin = curve_arg(&args["fade_in_curve"])?;
        let fout = curve_arg(&args["fade_out_curve"])?;
        let mut points = Vec::new();
        for p in args["points"].as_array().into_iter().flatten() {
            let (Some(s), Some(db)) = (p["seconds"].as_f64(), p["db"].as_f64()) else {
                return Err("Each point needs seconds and db.".into());
            };
            points.push(((s.max(0.0) * self.rate as f64).round() as i64, db as f32));
        }
        let map = self.tempo_map();
        let c = &mut self.tracks[i].clips[j];
        if let Some(k) = fin {
            c.fade_in.curve = k;
        }
        if let Some(k) = fout {
            c.fade_out.curve = k;
        }
        if args["clear_points"].as_bool() == Some(true) || args["points"].is_array() {
            c.envelope.clear();
        }
        for (rel, db) in points {
            // From the clip's start on the timeline to its audio.
            let frame = c.frame_of_out(&map, c.start + rel).round() as i64;
            c.add_point(frame - c.offset as i64, db);
        }
        let v = json!({
            "clip": c.key,
            "fade_in_curve": curve_name(c.fade_in.curve),
            "fade_out_curve": curve_name(c.fade_out.curve),
            "points": c.envelope.iter().map(|p| json!({
                "seconds": (c.out_of_frame(&map, p.at as f64) - c.start) as f64 / self.rate.max(1) as f64,
                "db": p.db,
            })).collect::<Vec<_>>(),
        });
        self.rebuild();
        Ok((v, Some("Change a clip's volume".into())))
    }

    fn edit_groups_tool(&mut self, args: &Value) -> Done {
        let v = self.edit_model(|p| {
            let has = |p: &daw_project::Project, k: &str| -> Result<(), String> {
                match p.tracks.iter().any(|t| t.id == k) {
                    true => Ok(()),
                    false => Err(format!("There is no track {k}.")),
                }
            };
            let ids = |v: &Value| -> Result<Vec<String>, String> {
                (v.as_array().into_iter().flatten())
                    .map(|k| {
                        k.as_str()
                            .map(String::from)
                            .ok_or("Track ids are text.".into())
                    })
                    .collect()
            };
            let mut added = Vec::new();
            for g in args["add"].as_array().into_iter().flatten() {
                let tracks = ids(&g["tracks"])?;
                for k in &tracks {
                    has(p, k)?;
                }
                let name = match g["name"].as_str() {
                    Some(n) if !n.trim().is_empty() => n.trim().to_string(),
                    _ => format!("Group {}", p.edit_groups.len() + 1),
                };
                added.push(crate::groups::add_group(&mut p.edit_groups, name, &tracks));
            }
            for g in args["change"].as_array().into_iter().flatten() {
                let id = g["id"].as_str().ok_or("Each change needs a group's id.")?;
                let gi = (p.edit_groups.iter().position(|e| e.id == id))
                    .ok_or_else(|| format!("There is no edit group {id}."))?;
                if let Some(n) = g["name"].as_str() {
                    p.edit_groups[gi].name = n.trim().into();
                }
                if let Some(on) = g["on"].as_bool() {
                    p.edit_groups[gi].active = on;
                }
                if g["tracks"].is_array() {
                    let tracks = ids(&g["tracks"])?;
                    for k in &tracks {
                        has(p, k)?;
                    }
                    for k in p.edit_groups[gi].tracks.clone() {
                        crate::groups::assign(&mut p.edit_groups, &k, None);
                    }
                    for k in &tracks {
                        crate::groups::assign(&mut p.edit_groups, k, Some(id));
                    }
                }
            }
            let remove = ids(&args["remove"])?;
            if let Some(id) = remove
                .iter()
                .find(|id| !p.edit_groups.iter().any(|g| &g.id == *id))
            {
                return Err(format!("There is no edit group {id}."));
            }
            p.edit_groups.retain(|g| !remove.contains(&g.id));
            Ok(json!({"added": added}))
        })?;
        let changed =
            args["add"].is_array() || args["change"].is_array() || args["remove"].is_array();
        if let Some(on) = args["group_editing"].as_bool() {
            self.timeline.group_edit = on;
        }
        let groups: Vec<Value> = (self.edit_groups.iter())
            .map(|g| json!({"id": g.id, "name": g.name, "tracks": g.tracks, "on": g.active}))
            .collect();
        Ok((
            json!({"added": v["added"], "groups": groups, "group_editing": self.timeline.group_edit}),
            changed.then(|| "Change edit groups".into()),
        ))
    }

    fn snapshots_value(&self) -> Value {
        let doc = &self.proj.doc;
        let list: Vec<Value> = (doc.snapshots().into_iter())
            .map(|s| {
                json!({"id": s.id, "name": s.name, "created_ms": s.created,
                       "by": doc.name_of(&s.author), "restorable": s.restorable})
            })
            .collect();
        json!({"snapshots": list})
    }

    fn edit_snapshot_tool(&mut self, ctx: &eframe::egui::Context, args: &Value) -> Done {
        if !self.may_edit_song() {
            return Err("Only the owner and editors can take or restore snapshots.".into());
        }
        let action = text(args, "action", "what to do")?;
        let known = |app: &App| -> Result<String, String> {
            let id = text(args, "snapshot", "the snapshot's id")?;
            match app.proj.doc.snapshots().iter().any(|s| s.id == id) {
                true => Ok(id.to_string()),
                false => Err(format!("There is no snapshot {id}.")),
            }
        };
        let name = || -> Result<String, String> {
            let n = text(args, "name", "a name")?.trim();
            match n.is_empty() {
                true => Err("Give a name that isn't empty.".into()),
                false => Ok(n.chars().take(daw_project::MAX_SNAPSHOT_NAME).collect()),
            }
        };
        match action {
            "take" => {
                let n = match args["name"].as_str().map(str::trim) {
                    None | Some("") => format!("Snapshot {}", self.proj.doc.snapshots().len() + 1),
                    Some(_) => name()?,
                };
                let id = self.take_snapshot(ctx, &n);
                Ok((json!({"snapshot": id, "name": n}), None))
            }
            "restore" => {
                let id = known(self)?;
                self.write_plugin_states();
                let p = (self.proj.doc.snapshot_project(&id))
                    .ok_or("This snapshot can't be restored from this copy of the project.")?;
                self.edit_model(|now| {
                    *now = p;
                    Ok(json!({"restored": id}))
                })
                .map(|v| (v, Some("Restore a snapshot".into())))
            }
            "rename" => {
                let id = known(self)?;
                let n = name()?;
                self.proj.doc.rename_snapshot(&id, &n);
                self.proj.dirty = true;
                Ok((json!({"snapshot": id, "name": n}), None))
            }
            "delete" => {
                let id = known(self)?;
                self.proj.doc.delete_snapshot(&id);
                self.proj.dirty = true;
                Ok((json!({"deleted": id}), None))
            }
            other => Err(format!("There is no snapshot action {other}.")),
        }
    }

    fn set_track_input_tool(&mut self, args: &Value) -> Done {
        let i = self.track_at(text(args, "track", "the track's id")?)?;
        let inputs = self.input_count();
        let outputs = self.midi.ports.outputs.clone();
        let t = &self.tracks[i];
        if t.kind.is_bus() {
            return Err(format!(
                "{} is a bus or folder, which records nothing.",
                t.name
            ));
        }
        let notes = t.kind.plays_notes();
        let input = match args["input"].as_u64() {
            None => t.input,
            Some(0) => None,
            Some(ch) => {
                if notes {
                    return Err("An instrument track plays its notes, not a device input.".into());
                }
                let stereo = args["stereo"]
                    .as_bool()
                    .unwrap_or(t.input.is_some_and(|x| x.stereo));
                let first = ch as usize - 1;
                let need = first + if stereo { 2 } else { 1 };
                if need > inputs {
                    return Err(format!("The audio device has {inputs} inputs."));
                }
                Some(match stereo {
                    true => daw_engine::graph::Input::stereo(first),
                    false => daw_engine::graph::Input::mono(first),
                })
            }
        };
        let midi_out = match args["midi_out"].as_str() {
            None => t.midi_out,
            Some(_) if !notes => {
                return Err("Only an instrument track sends notes to a MIDI output.".into());
            }
            Some("none") => None,
            Some(name) => Some(
                (outputs.iter().position(|o| o.eq_ignore_ascii_case(name)))
                    .ok_or_else(|| format!("There is no MIDI output {name}."))?,
            ),
        };
        let t = &mut self.tracks[i];
        t.input = input;
        t.midi_out = midi_out;
        if let Some(a) = args["arm"].as_bool() {
            t.arm = a;
        }
        if let Some(m) = args["monitor"].as_bool() {
            t.monitor = m;
        }
        let v = json!({
            "track": t.key,
            "arm": t.arm,
            "monitor": t.monitor,
            "input": t.input.map(|x| x.first + 1),
            "stereo": t.input.is_some_and(|x| x.stereo),
            "midi_out": t.midi_out.and_then(|p| outputs.get(p)),
            "device_inputs": inputs,
            "midi_outputs": outputs,
        });
        self.rebuild();
        // This computer's settings, not the project's: no undo step.
        Ok((v, None))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Track;
    use crate::agent::driver::Driver;
    use crate::timeline::{self, tests::clip};
    use crate::timing::tests::take;

    /// A driver with a track of three takes of the same two seconds
    /// (at 48 kHz), and the track's id.
    fn three_takes() -> (Driver, String) {
        let mut d = Driver::new();
        let app = d.app();
        app.tracks.push(Track::new("Guitar".into()));
        for _ in 0..3 {
            timeline::add_take(&mut app.tracks[0].clips, clip(0, 96_000));
        }
        app.rebuild();
        let key = app.tracks[0].key.clone();
        (d, key)
    }

    fn comps(v: &Value) -> Vec<Value> {
        v["comps"].as_array().unwrap().clone()
    }

    #[test]
    fn takes_are_comped_switched_and_flattened() {
        let (mut d, t) = three_takes();
        assert_eq!(d.app().rate, 48_000);
        let v = d.call("get_takes", json!({"track": t})).unwrap();
        assert_eq!(v["takes"].as_array().unwrap().len(), 3);
        assert_eq!(v["take_playing"], 3, "the newest plays");
        assert!(
            d.call(
                "edit_comp",
                json!({"track": t, "action": "swipe", "take": 9,
                                       "from_seconds": 0, "to_seconds": 1})
            )
            .is_err()
        );

        let v = d
            .call(
                "edit_comp",
                json!({"track": t, "action": "swipe", "take": 1, "from_seconds": 0.5, "to_seconds": 1.0}),
            )
            .unwrap();
        let c = comps(&v);
        assert_eq!(c.len(), 1);
        assert_eq!(v["comp_playing"], c[0]["id"]);
        let parts: Vec<(Value, Value)> = (c[0]["parts"].as_array().unwrap().iter())
            .map(|p| (p["take"].clone(), p["from_seconds"].clone()))
            .collect();
        assert_eq!(
            parts,
            [
                (json!(3), json!(0.0)),
                (json!(1), json!(0.5)),
                (json!(3), json!(1.0))
            ]
        );
        // In the project too, as one undo step.
        let p = d.app().project();
        assert_eq!(p.tracks[0].comps.len(), 1);
        let first = c[0]["id"].as_str().unwrap().to_string();

        let v = d
            .call("edit_comp", json!({"track": t, "action": "duplicate"}))
            .unwrap();
        let second = comps(&v)[1]["id"].as_str().unwrap().to_string();
        assert_eq!(v["comp_playing"], second);
        d.call(
            "edit_comp",
            json!({"track": t, "action": "rename", "comp": second, "name": "Best"}),
        )
        .unwrap();
        d.call(
            "edit_comp",
            json!({"track": t, "action": "use_take", "take": 2, "at_seconds": 0.2}),
        )
        .unwrap();
        let v = d
            .call(
                "edit_comp",
                json!({"track": t, "action": "choose", "comp": first}),
            )
            .unwrap();
        assert_eq!(v["comp_playing"], first);
        assert_eq!(comps(&v)[1]["name"], "Best");
        let v = d
            .call(
                "edit_comp",
                json!({"track": t, "action": "choose_take", "take": 2}),
            )
            .unwrap();
        assert_eq!(v["comp_playing"], Value::Null);
        assert_eq!(v["take_playing"], 2);
        assert_eq!(comps(&v).len(), 2, "comps are kept");

        d.call(
            "edit_comp",
            json!({"track": t, "action": "delete", "comp": first}),
        )
        .unwrap();
        d.call(
            "edit_comp",
            json!({"track": t, "action": "choose", "comp": second}),
        )
        .unwrap();
        let v = d
            .call("edit_comp", json!({"track": t, "action": "flatten"}))
            .unwrap();
        assert!(comps(&v).is_empty());
        assert!(v["takes"].as_array().unwrap().is_empty());
        let p = d.app().project();
        assert!(p.tracks[0].clips.iter().all(|c| c.take.is_none()));
        // Undo puts the takes and the comp back.
        d.command("undo");
        let v = d.call("get_takes", json!({"track": t})).unwrap();
        assert_eq!(v["takes"].as_array().unwrap().len(), 3);
        assert_eq!(v["comp_playing"], second);
    }

    #[test]
    fn a_sloppy_take_is_fixed_with_the_settings_given() {
        let mut d = Driver::new();
        let c = take(60_000, &[13_900, 22_500, 36_700, 46_100]);
        let key = c.key.clone();
        let app = d.app();
        app.tracks.push(Track::new("Guitar".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        assert!(
            d.call("fix_timing", json!({"clips": [key], "grid": "1/5"}))
                .is_err()
        );
        let v = d
            .call(
                "fix_timing",
                json!({"clips": [key], "grid": "1/2", "preview": true}),
            )
            .unwrap();
        assert_eq!(v["notes_found"], 4);
        assert_eq!(v["fixed"], false);
        assert!(v["most_moved_ms"].as_f64().unwrap() > 20.0, "{v}");
        assert_eq!(d.app().tracks[0].clips.len(), 1);
        let v = d
            .call(
                "fix_timing",
                json!({"clips": [key], "grid": "1/2", "strength": 1}),
            )
            .unwrap();
        assert_eq!(v["fixed"], true);
        assert_eq!(v["pieces"].as_array().unwrap().len(), 5);
        assert_eq!(
            d.project()["tracks"][0]["clips"].as_array().unwrap().len(),
            5
        );
        assert!(
            d.app().timeline.selected.is_empty(),
            "the selection is the person's"
        );
        d.command("undo");
        assert_eq!(
            d.project()["tracks"][0]["clips"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn a_clip_follows_tempo_and_its_markers_are_placed_moved_and_quantised() {
        let mut d = Driver::new();
        let c = take(60_000, &[13_900, 22_500, 36_700, 46_100]);
        let key = c.key.clone();
        let app = d.app();
        app.tracks.push(Track::new("Guitar".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        let v = d
            .call(
                "stretch_clips",
                json!({"clips": [key], "follow_tempo": true, "tempo": 120}),
            )
            .unwrap();
        assert_eq!(v["clips"][0]["follows_tempo"], true);
        assert_eq!(v["clips"][0]["tempo"], 120.0);
        assert!(
            d.call("stretch_clips", json!({"clips": [key], "tempo": 900}))
                .is_err()
        );

        let v = d
            .call(
                "stretch_clips",
                json!({"clips": [key], "markers_from_notes": true}),
            )
            .unwrap();
        assert_eq!(v["clips"][0]["markers"].as_array().unwrap().len(), 4, "{v}");
        let v = d
            .call(
                "stretch_clips",
                json!({"clips": [key], "clear_markers": true, "add_markers": [0.5]}),
            )
            .unwrap();
        let m = &v["clips"][0]["markers"];
        assert_eq!(m.as_array().unwrap().len(), 1, "{v}");
        assert!(
            (m[0]["seconds"].as_f64().unwrap() - 0.5).abs() < 0.001,
            "{v}"
        );
        let v = d
            .call(
                "stretch_clips",
                json!({"clips": [key], "move_markers": [{"index": 0, "to_seconds": 0.55}]}),
            )
            .unwrap();
        let at = v["clips"][0]["markers"][0]["seconds"].as_f64().unwrap();
        assert!((at - 0.55).abs() < 0.001, "{v}");
        let v = d
            .call(
                "stretch_clips",
                json!({"clips": [key], "remove_markers": [0]}),
            )
            .unwrap();
        assert!(v["clips"][0]["markers"].as_array().unwrap().is_empty());

        // Quantised to eighths at 120 bpm: the notes land on 0.25 s steps.
        let v = d
            .call(
                "stretch_clips",
                json!({"clips": [key], "quantise": {"grid": "1/2", "strength": 1}}),
            )
            .unwrap();
        for m in v["clips"][0]["markers"].as_array().unwrap() {
            let s = m["seconds"].as_f64().unwrap() / 0.25;
            assert!((s - s.round()).abs() < 0.01, "{v}");
        }
        d.command("undo");
        let p = d.project();
        assert!(
            p["tracks"][0]["clips"][0]["markers"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_tempo_comes_from_a_steady_performance() {
        let mut d = Driver::new();
        // Notes every 0.6 s: 100 bpm.
        let at: Vec<usize> = (1..9).map(|n| n * 28_800).collect();
        let c = take(9 * 28_800, &at);
        let key = c.key.clone();
        let app = d.app();
        app.tracks.push(Track::new("Drums".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        assert!(
            d.call("tempo_from_clip", json!({"clip": key, "unit": "minute"}))
                .is_err()
        );
        let v = d.call("tempo_from_clip", json!({"clip": key})).unwrap();
        let bpm = v["tempo_bpm"].as_f64().unwrap();
        assert!((bpm - 100.0).abs() < 1.0, "{v}");
        d.command("undo");
        assert_eq!(d.project()["tempo_bpm"], 120.0);
    }

    #[test]
    fn a_clip_is_processed_and_its_volume_line_drawn() {
        let mut d = Driver::new();
        let c = take(48_000, &[12_000]);
        let key = c.key.clone();
        let app = d.app();
        app.tracks.push(Track::new("Guitar".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        let v = d
            .call(
                "edit_clip_volume",
                json!({"clip": key, "fade_in_curve": "s-curve",
                       "points": [{"seconds": 0.5, "db": -6}, {"seconds": 0.1, "db": 99}]}),
            )
            .unwrap();
        assert_eq!(v["fade_in_curve"], "s-curve");
        let pts = v["points"].as_array().unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(
            (pts[0]["seconds"].clone(), pts[0]["db"].clone()),
            (json!(0.1), json!(12.0))
        );
        assert_eq!(pts[1]["db"], -6.0);
        let p = d.app().project();
        assert_eq!(p.tracks[0].clips[0].envelope.len(), 2);
        assert_eq!(
            p.tracks[0].clips[0].fade_in.curve,
            daw_project::Curve::SCurve
        );
        assert!(
            d.call(
                "edit_clip_volume",
                json!({"clip": key, "fade_out_curve": "wobbly"})
            )
            .is_err()
        );

        let asset = d.app().tracks[0].clips[0].asset.clone();
        assert!(
            d.call("process_clips", json!({"clips": [key], "how": "gain"}))
                .is_err()
        );
        let v = d
            .call("process_clips", json!({"clips": [key], "how": "reverse"}))
            .unwrap();
        assert_eq!(v["processing"], 1);
        for _ in 0..500 {
            if d.app().tracks[0].clips[0].asset != asset {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            d.project();
        }
        let c = &d.app().tracks[0].clips[0];
        assert_ne!(c.asset, asset, "the clip plays the new file");
        assert!(c.label.contains("reversed"), "{}", c.label);
    }

    #[test]
    fn a_track_is_rendered_and_frozen() {
        let mut d = Driver::new();
        let c = take(24_000, &[100]);
        let app = d.app();
        app.tracks.push(Track::new("Guitar".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        let e = d
            .call("freeze_track", json!({"track": t, "frozen": true}))
            .unwrap_err();
        assert!(e.contains("no effects"), "{e}");
        d.call("add_effect", json!({"track": t, "effect": "delay"}))
            .unwrap();
        let v = d
            .call("freeze_track", json!({"track": t, "frozen": true}))
            .unwrap();
        assert_eq!(v["frozen"], true);
        assert!(v["message"].as_str().unwrap().contains("saved"), "{v}");
        assert_eq!(d.project()["tracks"][0]["frozen"], true);
        let h = d.call("get_history", json!({})).unwrap();
        assert!(h.to_string().contains("Assistant: Freeze a track"), "{h}");
        d.call("freeze_track", json!({"track": t, "frozen": false}))
            .unwrap();
        assert_eq!(d.project()["tracks"][0]["frozen"], false);

        let v = d.call("render_track", json!({"track": t})).unwrap();
        assert_eq!(v["rendering"], 1);
        for _ in 0..1000 {
            if d.project()["tracks"].as_array().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let p = d.project();
        assert_eq!(p["tracks"][1]["name"], "Guitar (rendered)");
        assert_eq!(p["tracks"][0]["mute"], true);
        assert!(d.call("render_track", json!({"track": "nope"})).is_err());
    }

    #[test]
    fn edit_groups_are_made_changed_and_removed() {
        let mut d = Driver::new();
        let a = d.call("add_track", json!({"kind": "audio"})).unwrap()["track"].clone();
        let b = d.call("add_track", json!({"kind": "audio"})).unwrap()["track"].clone();
        let c = d.call("add_track", json!({"kind": "audio"})).unwrap()["track"].clone();
        let v = d
            .call(
                "edit_groups",
                json!({"add": [{"name": "Drums", "tracks": [a, b]}]}),
            )
            .unwrap();
        let g = v["added"][0].as_str().unwrap().to_string();
        assert_eq!(v["groups"][0]["tracks"].as_array().unwrap().len(), 2);
        assert!(
            d.call("edit_groups", json!({"add": [{"tracks": ["nope"]}]}))
                .is_err()
        );
        let v = d
            .call(
                "edit_groups",
                json!({"change": [{"id": g, "tracks": [b, c], "on": false, "name": "Kit"}],
                       "group_editing": false}),
            )
            .unwrap();
        let grp = &v["groups"][0];
        assert_eq!(grp["name"], "Kit");
        assert_eq!(grp["on"], false);
        assert!(!grp["tracks"].as_array().unwrap().contains(&a));
        assert_eq!(v["group_editing"], false);
        assert_eq!(d.app().project().edit_groups[0].tracks.len(), 2);
        d.call("edit_groups", json!({"remove": [g]})).unwrap();
        assert!(d.app().project().edit_groups.is_empty());
        d.command("undo");
        assert_eq!(d.app().project().edit_groups.len(), 1);
    }

    #[test]
    fn snapshots_are_taken_renamed_restored_and_deleted() {
        let mut d = Driver::new();
        d.call("add_track", json!({"kind": "audio", "name": "Before"}))
            .unwrap();
        let s = d
            .call(
                "edit_snapshot",
                json!({"action": "take", "name": "Lesson 3"}),
            )
            .unwrap()["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        d.call("add_track", json!({"kind": "audio", "name": "After"}))
            .unwrap();
        d.call(
            "edit_snapshot",
            json!({"action": "rename", "snapshot": s, "name": "Lesson 3 end"}),
        )
        .unwrap();
        let list = d.call("list_snapshots", json!({})).unwrap();
        assert_eq!(list["snapshots"][0]["name"], "Lesson 3 end");
        d.call("edit_snapshot", json!({"action": "restore", "snapshot": s}))
            .unwrap();
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), 1);
        d.command("undo");
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), 2);
        d.call("edit_snapshot", json!({"action": "delete", "snapshot": s}))
            .unwrap();
        let list = d.call("list_snapshots", json!({})).unwrap();
        assert!(list["snapshots"].as_array().unwrap().is_empty());
        assert!(
            d.call("edit_snapshot", json!({"action": "restore", "snapshot": s}))
                .is_err()
        );
    }

    #[test]
    fn a_track_is_armed_and_monitored() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "audio"})).unwrap()["track"].clone();
        let v = d
            .call(
                "set_track_input",
                json!({"track": t, "arm": true, "monitor": true}),
            )
            .unwrap();
        assert_eq!(
            (v["arm"].clone(), v["monitor"].clone()),
            (json!(true), json!(true))
        );
        let app = d.app();
        assert!(app.tracks[0].arm && app.tracks[0].monitor);
        let inputs = v["device_inputs"].as_u64().unwrap();
        let e = d
            .call("set_track_input", json!({"track": t, "input": inputs + 1}))
            .unwrap_err();
        assert!(e.contains("inputs"), "{e}");
        let v = d
            .call("set_track_input", json!({"track": t, "input": 0}))
            .unwrap();
        assert_eq!(v["input"], Value::Null);
        assert!(
            d.call("set_track_input", json!({"track": t, "midi_out": "none"}))
                .is_err()
        );
        let i = d.call("add_track", json!({"kind": "instrument"})).unwrap()["track"].clone();
        assert!(
            d.call(
                "set_track_input",
                json!({"track": i, "midi_out": "No such port"})
            )
            .is_err()
        );
        d.call("set_track_input", json!({"track": i, "midi_out": "none"}))
            .unwrap();
    }
}
