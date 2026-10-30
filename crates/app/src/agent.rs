// SPDX-License-Identifier: GPL-3.0-or-later
//! The agent interface's tools (roadmap 2.11, ARCHITECTURE §7a): what an
//! AI assistant may read and do in the app, by name, with JSON in and
//! out. Every command in the command table is one tool call away
//! (`run_command`), any track setting can be set by value
//! (`set_parameter`), and the read side gives the project, the
//! transport, the selection and the settings.
//!
//! Requests come in through [`Agent::inbox`] and are answered between
//! frames on the UI thread, exactly as the same buttons would act. Each
//! request that changes the project becomes one undo step, labelled
//! with what the assistant did. A command the timeline runs (Split,
//! say) acts while the next frame is drawn, so a request is answered
//! one frame after it starts, and the project waits to be written
//! until then.
//!
//! The local server that assistants connect to is still to come; the
//! test [`driver`] makes the same calls without it.

use std::sync::mpsc::{self, Receiver, Sender};

use daw_engine::engine::Command;
use daw_engine::time::{Frames, Ticks};
use eframe::egui;
use serde_json::{Value, json};

use crate::App;
use crate::commands::{COMMANDS, Cmd};

/// What an assistant has been allowed to do, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Access {
    /// Look at the project, the transport and the settings.
    Read,
    /// Change the project and run commands.
    Edit,
    /// Also save, and write files out of the app.
    EditExport,
}

impl Access {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Access::Read => "read only",
            Access::Edit => "edit",
            Access::EditExport => "edit and export",
        }
    }
}

/// One tool: its name, what it does, what it needs, and its arguments
/// as a JSON schema.
pub(crate) struct Tool {
    pub(crate) name: &'static str,
    // This and `schema` are read by the server, still to come.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) description: &'static str,
    pub(crate) access: Access,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) schema: fn() -> Value,
}

fn no_args() -> Value {
    json!({"type": "object", "properties": {}})
}

/// Every tool, read side first.
pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "list_commands",
        description: "Every command in the app (the same as the menus, keys and command \
                      search), with its id, name, menu, keys, and whether it can run now.",
        access: Access::Read,
        schema: no_args,
    },
    Tool {
        name: "get_project",
        description: "The project: tempo and time signature, loop, markers, and each track \
                      with its mix, routing, effects and clips. Times are given in frames \
                      at the project's rate and in seconds; note positions in ticks (960 \
                      to a quarter note).",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "notes": {"type": "boolean",
                          "description": "Include every note of every note clip."}
            }})
        },
    },
    Tool {
        name: "get_transport",
        description: "Where playback is (frames, seconds, bar and beat), whether it is \
                      playing or recording, and the loop, metronome and count-in.",
        access: Access::Read,
        schema: no_args,
    },
    Tool {
        name: "get_selection",
        description: "The selected clips, by track and clip id, and the selected note clip.",
        access: Access::Read,
        schema: no_args,
    },
    Tool {
        name: "list_parameters",
        description: "Every setting of a track that can be set by value (volume, pan, \
                      mute, solo, sends, and each effect's settings), with its value, \
                      range and how it reads. Without a track, every track's.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string", "description": "A track's id."}
            }})
        },
    },
    Tool {
        name: "run_command",
        description: "Runs a command by its id, as its menu entry or key would. \
                      list_commands gives the ids. Commands that save or write files need \
                      the edit and export permission; commands that open a file chooser \
                      need a person and are refused.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "id": {"type": "string", "description": "The command's id, such as \"split\"."}
            }, "required": ["id"]})
        },
    },
    Tool {
        name: "set_parameter",
        description: "Sets one track setting to a value. list_parameters gives each \
                      setting's param name and range. Mute and solo take 0 or 1.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string", "description": "The track's id."},
                "param": {"type": "string", "description": "The setting, as list_parameters names it."},
                "value": {"type": "number"}
            }, "required": ["track", "param", "value"]})
        },
    },
    Tool {
        name: "seek",
        description: "Moves the playhead to a bar (counted from 1) or to a time in seconds.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "bar": {"type": "number"},
                "seconds": {"type": "number"}
            }})
        },
    },
    Tool {
        name: "select_clips",
        description: "Selects these clips, by id, in place of the selection. An empty list \
                      clears it. Commands such as split, delete and crossfade act on it.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "clips": {"type": "array", "items": {"type": "string"}}
            }, "required": ["clips"]})
        },
    },
];

pub(crate) fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Commands that open the system's file chooser, which waits for a
/// person.
const NEEDS_A_PERSON: &[Cmd] = &[
    Cmd::ExportBundle,
    Cmd::ImportBundle,
    Cmd::ImportMidi,
    Cmd::ExportMidi,
    Cmd::ImportMusicXml,
    Cmd::ExportMusicXml,
    Cmd::ExportTextTab,
    Cmd::PrintNotation,
    Cmd::ImportKit,
    Cmd::ExportKit,
];

/// What a command needs of an assistant.
pub(crate) fn command_access(cmd: Cmd) -> Access {
    match cmd {
        Cmd::Save
        | Cmd::SaveAs
        | Cmd::SaveAsTemplate
        | Cmd::Export
        | Cmd::ExportVideo
        | Cmd::ExportBundle
        | Cmd::ExportMidi
        | Cmd::ExportMusicXml
        | Cmd::ExportTextTab
        | Cmd::PrintNotation
        | Cmd::ExportKit => Access::EditExport,
        _ => Access::Edit,
    }
}

/// A tool call waiting to be answered.
pub(crate) struct Request {
    pub(crate) tool: String,
    pub(crate) args: Value,
    /// What the assistant asking may do.
    pub(crate) access: Access,
    pub(crate) reply: Sender<Result<Value, String>>,
}

/// A request that has acted and is answered after the next frame.
struct Running {
    reply: Sender<Result<Value, String>>,
    result: Result<Value, String>,
    /// The undo step's name, if it changes the project.
    label: Option<String>,
    /// The frame it acted in has to be followed by one more.
    frames: u8,
}

/// The tools' side of the app: requests waiting, and those answered
/// after the next frame.
pub(crate) struct Agent {
    /// Only the test driver sends here until the server comes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) inbox: Sender<Request>,
    requests: Receiver<Request>,
    running: Vec<Running>,
}

impl Default for Agent {
    fn default() -> Agent {
        let (inbox, requests) = mpsc::channel();
        Agent {
            inbox,
            requests,
            running: Vec::new(),
        }
    }
}

impl Agent {
    /// A request has acted and its edit is yet to be written: the
    /// project waits, so the edit is one undo step of its own.
    pub(crate) fn holding(&self) -> bool {
        self.running.iter().any(|r| r.label.is_some())
    }
}

impl App {
    /// Called once a frame: answers requests that have had their frame,
    /// then starts those that came since.
    pub(crate) fn agent_tick(&mut self, ctx: &egui::Context) {
        for r in &mut self.agent.running {
            r.frames = r.frames.saturating_sub(1);
        }
        let (done, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.running)
            .into_iter()
            .partition(|r| r.frames == 0);
        self.agent.running = wait;
        for r in done {
            if let Some(label) = r.label {
                self.proj.label = Some(format!("Assistant: {label}"));
                self.sync_doc(ctx, true);
                self.proj.label = None;
            }
            let _ = r.reply.send(r.result);
        }
        while let Ok(req) = self.agent.requests.try_recv() {
            let Some(t) = tool(&req.tool) else {
                let _ = req
                    .reply
                    .send(Err(format!("There is no tool {}.", req.tool)));
                continue;
            };
            if req.access < t.access {
                let _ = req.reply.send(Err(format!(
                    "{} needs the {} permission; this assistant has {}.",
                    t.name,
                    t.access.name(),
                    req.access.name()
                )));
                continue;
            }
            if t.access == Access::Read {
                let _ = req.reply.send(self.read_tool(t.name, &req.args));
                continue;
            }
            // The user's own edits so far are theirs, not the assistant's.
            self.sync_doc(ctx, true);
            let (result, label) = match self.edit_tool(ctx, t.name, &req.args, req.access) {
                Ok((v, label)) => (Ok(v), label),
                Err(e) => (Err(e), None),
            };
            self.agent.running.push(Running {
                reply: req.reply,
                result,
                label,
                frames: 1,
            });
            ctx.request_repaint();
        }
    }

    fn read_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "list_commands" => Ok(self.commands_value()),
            "get_project" => Ok(self.project_value(args["notes"].as_bool().unwrap_or(false))),
            "get_transport" => Ok(self.transport_value()),
            "get_selection" => Ok(self.selection_value()),
            "list_parameters" => self.parameters_value(args["track"].as_str()),
            _ => Err(format!("There is no tool {name}.")),
        }
    }

    /// Runs a tool that may change things: its answer, and the undo
    /// step's name if it changes the project.
    fn edit_tool(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        args: &Value,
        access: Access,
    ) -> Result<(Value, Option<String>), String> {
        match name {
            "run_command" => {
                let id = args["id"].as_str().ok_or("Give the command's id.")?;
                let cmd = Cmd::from_id(id).ok_or_else(|| format!("There is no command {id}."))?;
                let need = command_access(cmd);
                if access < need {
                    return Err(format!(
                        "{id} needs the {} permission; this assistant has {}.",
                        need.name(),
                        access.name()
                    ));
                }
                if NEEDS_A_PERSON.contains(&cmd) {
                    return Err(format!(
                        "{id} opens a file chooser, which needs a person at the computer."
                    ));
                }
                if !self.can_run(cmd) {
                    return Err(format!("{id} can't run now."));
                }
                self.run(ctx, cmd);
                let name = self.commands.name(cmd).to_string();
                Ok((
                    json!({"ran": id, "message": self.message.clone()}),
                    Some(name),
                ))
            }
            "set_parameter" => {
                let track = args["track"].as_str().ok_or("Give the track's id.")?;
                let param = args["param"].as_str().ok_or("Give the setting's name.")?;
                let value = args["value"]
                    .as_f64()
                    .ok_or("Give the value as a number.")?;
                let label = self.set_parameter(track, param, value)?;
                Ok((json!({"set": param, "value": value}), Some(label)))
            }
            "seek" => {
                let map = self.tempo_map();
                let at = match (args["bar"].as_f64(), args["seconds"].as_f64()) {
                    (Some(bar), _) => {
                        let whole = bar.floor() as i64;
                        let from = map.bar_start(whole);
                        let len = map.bar_start(whole + 1).0 - from.0;
                        let t = Ticks(from.0 + ((bar - bar.floor()) * len as f64) as i64);
                        map.frames_at(t)
                    }
                    (None, Some(s)) => Frames((s * self.rate as f64).round() as i64),
                    (None, None) => return Err("Give a bar or a time in seconds.".into()),
                };
                let at = Frames(at.0.max(0));
                self.send(Command::Seek(at));
                Ok((json!({"frames": at.0}), None))
            }
            "select_clips" => {
                let keys = args["clips"].as_array().ok_or("Give a list of clip ids.")?;
                let mut ids = std::collections::HashSet::new();
                for k in keys {
                    let k = k.as_str().ok_or("Clip ids are text.")?;
                    let c = (self.tracks.iter().flat_map(|t| &t.clips))
                        .find(|c| c.key == k)
                        .ok_or_else(|| format!("There is no clip {k}."))?;
                    ids.insert(c.id);
                }
                self.timeline.selected = ids;
                Ok((json!({"selected": keys.len()}), None))
            }
            _ => Err(format!("There is no tool {name}.")),
        }
    }

    fn commands_value(&self) -> Value {
        let list: Vec<Value> = (COMMANDS.iter())
            .map(|i| {
                let ready = self.can_run(i.cmd) && !NEEDS_A_PERSON.contains(&i.cmd);
                json!({
                    "id": i.id(),
                    "name": self.commands.name(i.cmd),
                    "menu": i.group,
                    "hint": i.hint_text(),
                    "keys": self.commands.keymap.text(i.cmd),
                    "can_run": ready,
                    "needs": command_access(i.cmd).name(),
                })
            })
            .collect();
        Value::Array(list)
    }

    fn project_value(&self, notes: bool) -> Value {
        let p = self.project();
        let rate = p.rate.max(1) as f64;
        let secs = |f: i64| f as f64 / rate;
        let effect = |e: &daw_project::Effect| {
            json!({
                "id": e.id,
                "name": e.name,
                "builtin": (!e.builtin.is_empty()).then_some(&e.builtin),
                "bypass": e.bypass,
                "container": e.container,
            })
        };
        let tracks: Vec<Value> = (p.tracks.iter())
            .map(|t| {
                let clips: Vec<Value> = (t.clips.iter())
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "name": c.name,
                            "start": c.start,
                            "start_seconds": secs(c.start),
                            "len": c.len,
                            "len_seconds": secs(c.len),
                            "offset": c.offset,
                            "muted": c.muted,
                            "gain_db": c.gain_db,
                            "take": c.take,
                        })
                    })
                    .collect();
                let note_clips: Vec<Value> = (t.note_clips.iter())
                    .map(|c| {
                        let mut v = json!({
                            "id": c.id,
                            "name": c.name,
                            "start_ticks": c.start,
                            "len_ticks": c.len,
                            "muted": c.muted,
                            "pattern": c.pattern,
                            "note_count": c.notes.len(),
                        });
                        if notes {
                            v["notes"] = (c.notes.iter())
                                .map(|n| {
                                    json!({
                                        "start_ticks": n.start,
                                        "len_ticks": n.len,
                                        "pitch": n.pitch,
                                        "velocity": n.velocity,
                                        "channel": n.channel,
                                    })
                                })
                                .collect();
                        }
                        v
                    })
                    .collect();
                json!({
                    "id": t.id,
                    "name": t.name,
                    "kind": t.kind.name(),
                    "gain_db": t.gain_db,
                    "pan": t.pan,
                    "mute": t.mute,
                    "solo": t.solo,
                    "folder": t.parent,
                    "output": t.output,
                    "sends": t.sends.iter().map(|s| json!({
                        "to": s.to, "gain_db": s.gain_db,
                        "pre_fader": s.pre_fader, "mute": s.mute,
                    })).collect::<Vec<_>>(),
                    "instrument": t.instrument.as_ref().map(effect),
                    "effects": t.effects.iter().map(effect).collect::<Vec<_>>(),
                    "frozen": t.freeze,
                    "automated": t.automation.iter().map(|l| &l.param).collect::<Vec<_>>(),
                    "clips": clips,
                    "note_clips": note_clips,
                })
            })
            .collect();
        json!({
            "name": self.proj.folder.as_ref().map(|f| f.name()),
            "rate": p.rate,
            "tempo_bpm": p.tempo_bpm,
            "beats_per_bar": p.beats_per_bar,
            "beat_unit": p.beat_unit,
            "tempo_changes": p.tempo_changes.iter().map(|c| json!({
                "bar": c.bar, "beat": c.beat, "bpm": c.bpm, "glide": c.glide,
            })).collect::<Vec<_>>(),
            "meter_changes": p.meter_changes.iter().map(|m| json!({
                "bar": m.bar, "beats_per_bar": m.beats, "beat_unit": m.unit,
            })).collect::<Vec<_>>(),
            "loop_bars": [p.loop_bars.0, p.loop_bars.1],
            "scale": p.scale.map(|s| s.label()),
            "markers": p.markers.iter().map(|m| json!({
                "id": m.id, "bar": m.bar, "name": m.name,
            })).collect::<Vec<_>>(),
            "tracks": tracks,
            "comments": p.comments.len(),
            "lesson_notes": p.notes,
        })
    }

    fn transport_value(&self) -> Value {
        let st = self.ctl.status();
        let at = st.position();
        let map = self.tempo_map();
        let bb = map.bar_beat(map.ticks_at(at));
        json!({
            "playing": st.playing(),
            "recording": self.recording,
            "frames": at.0,
            "seconds": at.0 as f64 / self.rate as f64,
            "bar": bb.bar,
            "beat": bb.beat,
            "tick": bb.tick,
            "tempo_bpm": map.tempo_at(map.ticks_at(at)).as_f64(),
            "rate": self.rate,
            "loop": self.looping,
            "loop_bars": [self.loop_bars.0, self.loop_bars.1],
            "metronome": self.metronome.enabled,
            "count_in": self.count_in,
        })
    }

    fn selection_value(&self) -> Value {
        let clips: Vec<Value> = (self.tracks.iter())
            .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
            .filter(|(_, c)| self.timeline.selected.contains(&c.id))
            .map(|(t, c)| json!({"track": t.key, "clip": c.key}))
            .collect();
        json!({
            "clips": clips,
            "note_clip": self.timeline.note_sel,
        })
    }

    fn parameters_value(&self, only: Option<&str>) -> Result<Value, String> {
        if let Some(k) = only
            && self.track_index(k).is_none()
        {
            return Err(format!("There is no track {k}."));
        }
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let mut out = Vec::new();
        for t in self
            .tracks
            .iter()
            .filter(|t| only.is_none_or(|k| k == t.key))
        {
            let mut params = vec![
                json!({"param": MUTE, "name": "Mute", "value": t.mix.mute as u8, "min": 0, "max": 1}),
                json!({"param": SOLO, "name": "Solo", "value": t.mix.solo as u8, "min": 0, "max": 1}),
            ];
            for c in crate::automation::choices(t, &buses) {
                let (Some(v), Some(r)) = (
                    crate::automation::control(t, &c.param),
                    crate::automation::range(t, &c.param),
                ) else {
                    continue;
                };
                params.push(json!({
                    "param": c.param,
                    "name": c.name,
                    "value": v,
                    "text": crate::automation::text_of(t, &c.param, v),
                    "min": r.min,
                    "max": r.max,
                    "automated": t.automation.iter().any(|l| l.param == c.param),
                }));
            }
            out.push(json!({"track": t.key, "name": t.name, "params": params}));
        }
        Ok(Value::Array(out))
    }

    /// Sets a track's setting. The undo step's name, or why it can't.
    fn set_parameter(&mut self, key: &str, param: &str, v: f64) -> Result<String, String> {
        if !v.is_finite() {
            return Err("The value has to be a number.".into());
        }
        let i = self
            .track_index(key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let t = &mut self.tracks[i];
        let label = match param {
            MUTE | SOLO => {
                let on = v >= 0.5;
                if param == MUTE {
                    t.mix.mute = on;
                } else {
                    t.mix.solo = on;
                }
                let what = if param == MUTE { "Mute" } else { "Solo" };
                format!("{what} {} {}", t.name, if on { "on" } else { "off" })
            }
            _ => {
                let r = crate::automation::range(t, param)
                    .filter(|_| crate::automation::control(t, param).is_some())
                    .ok_or_else(|| format!("{} has no setting {param}.", t.name))?;
                let v = v.clamp(r.min, r.max);
                crate::automation::set_control(t, param, v);
                format!(
                    "Set {} {} to {}",
                    t.name,
                    crate::automation::name_of(t, &buses, param),
                    crate::automation::text_of(t, param, v)
                )
            }
        };
        self.rebuild();
        Ok(label)
    }
}

/// Mute and solo, which aren't automation lanes, by the names
/// `set_parameter` takes.
const MUTE: &str = "mute";
const SOLO: &str = "solo";

/// Drives the app headless through the tools, for end-to-end tests:
/// the calls an assistant makes, without a server or a window.
///
/// ```ignore
/// let mut d = Driver::new();
/// d.call("run_command", json!({"id": "add-instrument-track"})).unwrap();
/// assert_eq!(d.call("get_project", json!({})).unwrap()["tracks"][0]["kind"], "instrument");
/// ```
#[cfg(test)]
pub(crate) mod driver {
    use std::sync::mpsc;

    use egui_kittest::Harness;
    use serde_json::Value;

    use super::{Access, Request};
    use crate::App;

    pub(crate) struct Driver {
        pub(crate) h: Harness<'static, App>,
        /// What the calls are allowed; everything unless changed.
        pub(crate) access: Access,
    }

    impl Driver {
        pub(crate) fn new() -> Driver {
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            h.run();
            Driver {
                h,
                access: Access::EditExport,
            }
        }

        /// Makes a tool call and runs frames until it is answered.
        pub(crate) fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
            let (reply, answer) = mpsc::channel();
            let req = Request {
                tool: tool.into(),
                args,
                access: self.access,
                reply,
            };
            self.h
                .state()
                .agent
                .inbox
                .send(req)
                .expect("the app is there");
            for _ in 0..20 {
                self.h.step();
                if let Ok(a) = answer.try_recv() {
                    return a;
                }
            }
            panic!("{tool} was not answered");
        }

        /// Runs a command by id; panics if it is refused.
        pub(crate) fn command(&mut self, id: &str) -> Value {
            self.call("run_command", serde_json::json!({"id": id}))
                .unwrap_or_else(|e| panic!("{id}: {e}"))
        }

        pub(crate) fn project(&mut self) -> Value {
            self.call("get_project", serde_json::json!({})).unwrap()
        }

        pub(crate) fn app(&mut self) -> &mut App {
            self.h.state_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::driver::Driver;
    use super::*;

    #[test]
    fn every_tool_has_an_object_schema_and_a_unique_name() {
        let mut names = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(names.insert(t.name), "{} twice", t.name);
            assert!(!t.description.is_empty(), "{}", t.name);
            assert_eq!((t.schema)()["type"], "object", "{}", t.name);
        }
    }

    #[test]
    fn commands_are_listed_and_run_by_id() {
        let mut d = Driver::new();
        let list = d.call("list_commands", json!({})).unwrap();
        let ids: Vec<&str> = (list.as_array().unwrap().iter())
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), COMMANDS.len());
        assert!(ids.contains(&"add-instrument-track"));
        d.command("add-instrument-track");
        let p = d.project();
        assert_eq!(p["tracks"][0]["kind"], "instrument");
        assert_eq!(p["tracks"][0]["name"], "Instrument 1");
        assert!(d.call("run_command", json!({"id": "no-such"})).is_err());
    }

    #[test]
    fn a_request_is_one_undo_step() {
        let mut d = Driver::new();
        d.command("add-instrument-track");
        let key = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        d.call(
            "set_parameter",
            json!({"track": key, "param": "gain", "value": -6.0}),
        )
        .unwrap();
        d.call(
            "set_parameter",
            json!({"track": key, "param": "mute", "value": 1}),
        )
        .unwrap();
        let t = &d.project()["tracks"][0];
        assert_eq!(t["gain_db"], -6.0);
        assert_eq!(t["mute"], true);
        // Undo takes back the mute alone, then the volume.
        d.command("undo");
        let t = &d.project()["tracks"][0];
        assert_eq!(
            (t["gain_db"].clone(), t["mute"].clone()),
            (json!(-6.0), json!(false))
        );
        d.command("undo");
        assert_eq!(d.project()["tracks"][0]["gain_db"], 0.0);
        d.command("undo");
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn a_command_the_timeline_runs_has_acted_by_the_answer() {
        let mut d = Driver::new();
        d.call("seek", json!({"bar": 5})).unwrap();
        d.h.run();
        d.command("marker");
        assert_eq!(d.project()["markers"][0]["bar"], 5);
        d.command("undo");
        assert_eq!(d.project()["markers"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn parameters_are_listed_with_their_ranges_and_held_to_them() {
        let mut d = Driver::new();
        d.command("add-instrument-track");
        let key = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        let list = d.call("list_parameters", json!({"track": key})).unwrap();
        let params = list[0]["params"].as_array().unwrap();
        let gain = params.iter().find(|p| p["param"] == "gain").unwrap();
        assert_eq!(
            (gain["min"].clone(), gain["max"].clone()),
            (json!(-60.0), json!(12.0))
        );
        d.call(
            "set_parameter",
            json!({"track": key, "param": "gain", "value": 40.0}),
        )
        .unwrap();
        assert_eq!(d.project()["tracks"][0]["gain_db"], 12.0);
        let e = d
            .call(
                "set_parameter",
                json!({"track": key, "param": "nothing", "value": 1}),
            )
            .unwrap_err();
        assert!(e.contains("no setting"), "{e}");
    }

    #[test]
    fn what_an_assistant_may_do_is_held_to_its_permission() {
        let mut d = Driver::new();
        d.access = Access::Read;
        assert!(d.call("get_transport", json!({})).is_ok());
        let e = d.call("run_command", json!({"id": "loop"})).unwrap_err();
        assert!(e.contains("edit permission"), "{e}");
        d.access = Access::Edit;
        d.command("loop");
        assert_eq!(d.call("get_transport", json!({})).unwrap()["loop"], true);
        let e = d.call("run_command", json!({"id": "save"})).unwrap_err();
        assert!(e.contains("edit and export"), "{e}");
        d.access = Access::EditExport;
        let e = d
            .call("run_command", json!({"id": "import-midi"}))
            .unwrap_err();
        assert!(e.contains("file chooser"), "{e}");
    }

    #[test]
    fn seeking_by_bar_moves_the_playhead() {
        let mut d = Driver::new();
        let at = d.call("seek", json!({"bar": 3})).unwrap()["frames"]
            .as_i64()
            .unwrap();
        // Two bars of 4/4 at 120 bpm: four seconds.
        let rate = d.app().rate as i64;
        assert_eq!(at, 4 * rate);
        d.h.run();
        let t = d.call("get_transport", json!({})).unwrap();
        assert_eq!((t["bar"].clone(), t["beat"].clone()), (json!(3), json!(1)));
    }
}
