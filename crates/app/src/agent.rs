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
//! Assistants reach the tools through the local server
//! ([`crate::assistants`]); the test [`driver`] makes the same calls
//! without it.

use std::sync::mpsc::{self, Receiver, Sender};

use daw_engine::engine::Command;
use daw_engine::time::{Frames, Ticks};
use daw_project::Pick;
use eframe::egui;
use serde_json::{Value, json};

use crate::App;
use crate::commands::{COMMANDS, Cmd};
use crate::import_project::StemOptions;

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
    pub(crate) description: &'static str,
    pub(crate) access: Access,
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
                      search), with its id, name, menu, keys, and whether it can run now. In the \
                      simpler mode, \"offered\" is false for the advanced ones the person \
                      can't see; they still run by id.",
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
                      playing or recording, the loop, metronome and count-in, and the \
                      practice speed and transpose.",
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
        name: "get_fretboard",
        description: "What the fretboard pane shows: the tuning (open pitches from string 1, \
                      the highest), capo, the scale or chord shown and its places, the \
                      notes playing at the playhead and the selected notes, each as a \
                      string and a fret counted from the capo.",
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
        name: "list_presets",
        description: "Every track and effect chain preset: the user's and those that come \
                      with the app, each with its name, kind (track or chain), tags and \
                      description.",
        access: Access::Read,
        schema: no_args,
    },
    Tool {
        name: "get_meters",
        description: "The meters now: peak and RMS of every track and the master, left \
                      and right, in dBFS. They move only while playing.",
        access: Access::Read,
        schema: no_args,
    },
    Tool {
        name: "export_report",
        description: "What exporting the project for other DAWs (a .dawproject file) would \
                      leave out, item by item, without writing anything.",
        access: Access::Read,
        schema: no_args,
    },
    Tool {
        name: "analyse",
        description: "Renders a stretch offline and measures it: sample and true peak, \
                      RMS, integrated and short-term loudness, loudness range, stereo \
                      correlation, and the level in each third-octave band from 20 Hz to \
                      20 kHz. Of one track or bus on its own (after its fader), or of the \
                      mix, maybe without one track. Without times: the loop if it is on, \
                      else the whole song (at most 10 minutes).",
        access: Access::Read,
        schema: region_args,
    },
    Tool {
        name: "render_audio",
        description: "Renders a stretch to a 16-bit WAV file, as analyse chooses it, and \
                      gives its path; up to 10 seconds is sent in the answer as audio too.",
        access: Access::Read,
        schema: region_args,
    },
    Tool {
        name: "get_picture",
        description: "A picture (PNG) of the app's window as it is drawn next, or of one \
                      pane: timeline, mixer, comments, notes, lessons, tuner, takes, \
                      monitor, notes-editor, automation, browser, patterns or video.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "pane": {"type": "string", "description": "A pane's id; the whole window if left out."}
            }})
        },
    },
    Tool {
        name: "get_history",
        description: "The undo list (this user's steps Undo would take back, next first, \
                      each with what it changed), the redo list, and who changed what in \
                      the project and when, newest first.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "runs": {"type": "integer",
                         "description": "How many runs of activity to give; 20 if not said."}
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
        name: "add_preset",
        description: "Adds a preset, by name, as a new track, or with a track id puts it \
                      on that track: an effect chain replaces the track's effects; a track \
                      preset replaces its instrument, effects, sends and settings, keeping \
                      its clips and notes. Says which routes it couldn't find.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "name": {"type": "string", "description": "The preset's name, as list_presets gives it."},
                "track": {"type": "string", "description": "A track's id, to put it on that track."}
            }, "required": ["name"]})
        },
    },
    Tool {
        name: "save_preset",
        description: "Saves a track, or only its effect chain, as one of the user's \
                      presets, replacing one of that name.",
        access: Access::EditExport,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string", "description": "The track's id."},
                "kind": {"type": "string", "enum": ["track", "chain"]},
                "name": {"type": "string"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "about": {"type": "string", "description": "What it is for."}
            }, "required": ["track", "kind", "name"]})
        },
    },
    Tool {
        name: "undo_steps",
        description: "Undoes several steps at once (undo), or redoes them (redo), as \
                      choosing a step on the undo list does. Each stays a step of its own.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "undo": {"type": "integer", "minimum": 0},
                "redo": {"type": "integer", "minimum": 0}
            }})
        },
    },
    Tool {
        name: "switch_version",
        description: "Plays another version of a track, by the version's id as get_project \
                      lists it. Commands such as duplicate-track-version make versions.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string", "description": "The track's id."},
                "version": {"type": "string", "description": "The version's id."}
            }, "required": ["track", "version"]})
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
    Tool {
        name: "export_project_file",
        description: "Writes the project as a .dawproject file, which several other DAWs \
                      open, at a full path. Answers with what was left out.",
        access: Access::EditExport,
        schema: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Where to write it, ending .dawproject."}
            }, "required": ["path"]})
        },
    },
    Tool {
        name: "export_sections",
        description: "Exports a practice file of the mix for each song section (the bars \
                      from one marker to the next), cut at its bars, into a new numbered \
                      'Sections' folder. Starts now, or waits in the export queue behind the \
                      exports already going. Follow it with get_exports.",
        access: Access::EditExport,
        schema: || {
            json!({"type": "object", "properties": {
                "sections": {"type": "array", "items": {"type": "string"},
                             "description": "Only these sections, by marker id or name; every section if left out."},
                "play_along": {"type": "boolean",
                               "description": "Leave out the tracks muted for practice, to play along with. As the Export window is set if left out."},
                "format": {"type": "string", "enum": ["wav", "flac", "mp3", "opus"],
                           "description": "The file type; as the Export window is set if left out."},
                "folder": {"type": "string",
                           "description": "A full path to make the numbered folder in; the project's Exports folder if left out."}
            }})
        },
    },
    Tool {
        name: "get_exports",
        description: "The exports: how far the one running has got, those waiting in the \
                      queue, and each one finished since the queue last started from empty, \
                      with its folder and how each file went.",
        access: Access::Read,
        schema: no_args,
    },
    Tool {
        name: "import_project_file",
        description: "Opens a .dawproject file from another DAW, at a full path, as a new \
                      unsaved project in place of this one. Refused while this one has \
                      unsaved work. Answers with what didn't come in.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "The file's full path."}
            }, "required": ["path"]})
        },
    },
    Tool {
        name: "import_from_project",
        description: "Brings tracks in from another project, with their audio, effects and \
                      routing, as one undo step; and if asked its sections (markers), tempo \
                      and time signature, key, tuning and loop, and lesson notes. Without \
                      tracks, every track. get_project on the other project isn't possible, \
                      so the answer lists the new tracks' ids.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "project": {"type": "string",
                            "description": "The other project's folder: a full path, or a name in the projects folder."},
                "tracks": {"type": "array", "items": {"type": "string"},
                           "description": "Track names or ids in the other project."},
                "routing": {"type": "boolean", "description": "Also the buses and folders they play through (default true)."},
                "markers": {"type": "boolean"},
                "tempo": {"type": "boolean"},
                "settings": {"type": "boolean"},
                "notes": {"type": "boolean"}
            }, "required": ["project"]})
        },
    },
    Tool {
        name: "export_as_project",
        description: "Saves chosen tracks (every track without a list), or bars of the \
                      song, as a new project in the projects folder, with their audio and \
                      the tempo. Bars run from the start of from_bar to the end of to_bar.",
        access: Access::EditExport,
        schema: || {
            json!({"type": "object", "properties": {
                "name": {"type": "string", "description": "The new project's name."},
                "tracks": {"type": "array", "items": {"type": "string"}, "description": "Track ids."},
                "from_bar": {"type": "integer"},
                "to_bar": {"type": "integer"},
                "routing": {"type": "boolean"},
                "markers": {"type": "boolean"},
                "settings": {"type": "boolean"},
                "notes": {"type": "boolean"}
            }, "required": ["name"]})
        },
    },
    Tool {
        name: "export_stem_package",
        description: "Writes chosen tracks (every track without a list) as a stem \
                      package: each as audio, instrument tracks' notes as MIDI, and a page \
                      with the tempo, in a new numbered folder beside the other exports, \
                      or packed into one .tar file. Runs in the background.",
        access: Access::EditExport,
        schema: || {
            json!({"type": "object", "properties": {
                "tracks": {"type": "array", "items": {"type": "string"}, "description": "Track ids."},
                "from_bar": {"type": "integer"},
                "to_bar": {"type": "integer"},
                "audio": {"type": "boolean"},
                "notes": {"type": "boolean"},
                "mix": {"type": "boolean"},
                "flac": {"type": "boolean"},
                "archive": {"type": "boolean"}
            }})
        },
    },
];

fn region_args() -> Value {
    json!({"type": "object", "properties": {
        "track": {"type": "string", "description": "A track or bus id: it alone. Left out: the mix."},
        "without": {"type": "string", "description": "A track id to leave out of the mix."},
        "from_seconds": {"type": "number"},
        "to_seconds": {"type": "number"}
    }})
}

/// Every tool: these, then those for what the panes do.
pub(crate) fn all_tools() -> impl Iterator<Item = &'static Tool> {
    (TOOLS.iter())
        .chain(crate::agent_edit::TOOLS)
        .chain(crate::agent_arrange::TOOLS)
        .chain(crate::agent_files::TOOLS)
}

pub(crate) fn tool(name: &str) -> Option<&'static Tool> {
    all_tools().find(|t| t.name == name)
}

/// The bars a call gives (from the start of `from_bar` to the end of
/// `to_bar`), or `None` for the whole song.
fn bars_of(args: &Value) -> Result<Option<(i64, i64)>, String> {
    match (args["from_bar"].as_i64(), args["to_bar"].as_i64()) {
        (None, None) => Ok(None),
        (Some(a), Some(b)) if a >= 1 && b >= a => Ok(Some((a, b + 1))),
        _ => Err("Give from_bar and to_bar, from 1, with to_bar not before from_bar.".into()),
    }
}

/// Commands that open the system's file chooser, which waits for a
/// person.
const NEEDS_A_PERSON: &[Cmd] = &[
    Cmd::ExportDawProject,
    Cmd::ImportDawProject,
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
    Cmd::ImportPreset,
];

/// What a command needs of an assistant.
pub(crate) fn command_access(cmd: Cmd) -> Access {
    match cmd {
        Cmd::Save
        | Cmd::SaveAs
        | Cmd::SaveAsTemplate
        | Cmd::Export
        | Cmd::ExportVideo
        | Cmd::ExportSections
        | Cmd::ExportThisSection
        | Cmd::ExportDawProject
        | Cmd::ExportBundle
        | Cmd::ExportMidi
        | Cmd::ExportMusicXml
        | Cmd::ExportTextTab
        | Cmd::PrintNotation
        | Cmd::ExportKit
        | Cmd::ExportParts
        | Cmd::ExportStemPackage => Access::EditExport,
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
    /// A seek waits for the engine to get there (or a second to pass),
    /// so what is asked next sees the new place.
    until: Option<(Frames, std::time::Instant)>,
}

/// The tools' side of the app: requests waiting, and those answered
/// after the next frame.
pub(crate) struct Agent {
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

/// How long a seek waits for the engine at most.
const SEEK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

impl App {
    /// Called once a frame: answers requests that have had their frame,
    /// then starts those that came since.
    pub(crate) fn agent_tick(&mut self, ctx: &egui::Context) {
        for r in &mut self.agent.running {
            r.frames = r.frames.saturating_sub(1);
        }
        let (done, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.running)
            .into_iter()
            .partition(|r| r.frames == 0 && !self.waiting(r));
        self.agent.running = wait;
        if !self.agent.running.is_empty() {
            ctx.request_repaint();
        }
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
            if crate::listen::LATER.contains(&t.name) {
                self.listen_tool(ctx, t.name, &req.args, req.reply);
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
            let until = (t.name == "seek")
                .then(|| result.as_ref().ok()?["frames"].as_i64())
                .flatten()
                .map(|f| (Frames(f), std::time::Instant::now() + SEEK_WAIT));
            self.agent.running.push(Running {
                reply: req.reply,
                result,
                label,
                frames: 1,
                until,
            });
            ctx.request_repaint();
        }
    }

    /// The engine hasn't reached a seek's place yet.
    fn waiting(&self, r: &Running) -> bool {
        let Some((at, by)) = r.until else {
            return false;
        };
        // Playing moves on from it, so near is there.
        let near = (self.ctl.status().position().0 - at.0).abs() < self.rate as i64 / 4;
        !near && std::time::Instant::now() < by
    }

    fn read_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "list_commands" => Ok(self.commands_value()),
            "get_project" => Ok(self.project_value(args["notes"].as_bool().unwrap_or(false))),
            "get_transport" => Ok(self.transport_value()),
            "get_selection" => Ok(self.selection_value()),
            "get_fretboard" => Ok(self.fretboard_value()),
            "list_parameters" => self.parameters_value(args["track"].as_str()),
            "list_presets" => {
                let (list, _) = crate::presets::all_presets(self.preset_ui.dir.as_deref());
                Ok(Value::Array(
                    (list.iter())
                        .map(|e| {
                            json!({
                                "name": e.preset.name,
                                "kind": e.preset.kind.id(),
                                "tags": e.preset.tags,
                                "about": e.preset.about,
                                "included": e.file.is_none(),
                            })
                        })
                        .collect(),
                ))
            }
            "export_report" => Ok(crate::interchange::report_value(&self.interchange_report())),
            "get_meters" => Ok(self.meters_value()),
            "get_history" => Ok(self.history_value(args["runs"].as_u64().unwrap_or(20) as usize)),
            "get_exports" => Ok(self.exports_value()),
            "list_effect_kinds" => Ok(self.effect_kinds_value()),
            "get_comments" => Ok(self.comments_value()),
            "list_projects" => Ok(self.projects_value()),
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
                    let instead = match OWN_TOOL.iter().find(|(c, _)| *c == cmd) {
                        Some((_, t)) => format!("{t} does it with a file given"),
                        None => "file_command runs it with a file given".into(),
                    };
                    return Err(format!(
                        "{id} opens a file chooser, which needs a person at the computer; \
                         {instead}."
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
            "add_preset" => {
                let name = args["name"].as_str().ok_or("Give the preset's name.")?;
                let e = self.find_preset(name)?;
                match args["track"].as_str() {
                    Some(key) => {
                        let i = (self.tracks.iter())
                            .position(|t| t.key == key)
                            .ok_or_else(|| format!("There is no track {key}."))?;
                        let note = self.apply_preset(ctx, i, &e)?;
                        Ok((
                            json!({"track": key, "note": note.trim()}),
                            Some(format!("Put the preset {name} on a track")),
                        ))
                    }
                    None => {
                        let (i, note) = self.add_preset_track(ctx, &e)?;
                        Ok((
                            json!({"track": self.tracks[i].key, "note": note.trim()}),
                            Some(format!("Added the preset {name}")),
                        ))
                    }
                }
            }
            "save_preset" => {
                let key = args["track"].as_str().ok_or("Give the track's id.")?;
                let i = (self.tracks.iter())
                    .position(|t| t.key == key)
                    .ok_or_else(|| format!("There is no track {key}."))?;
                let kind = (args["kind"].as_str())
                    .and_then(daw_project::PresetKind::from_id)
                    .ok_or("Give the kind: track or chain.")?;
                let name = args["name"].as_str().ok_or("Give the preset's name.")?;
                let tags: Vec<&str> = (args["tags"].as_array().into_iter().flatten())
                    .filter_map(|t| t.as_str())
                    .collect();
                let tags = daw_project::parse_tags(&tags.join(","));
                let about = args["about"].as_str().unwrap_or("");
                let (_, left_out) = self.save_preset(ctx, i, kind, name, &tags, about)?;
                Ok((json!({"saved": name, "files_left_out": left_out}), None))
            }
            "undo_steps" => {
                let undo = args["undo"].as_u64().unwrap_or(0) as usize;
                let redo = args["redo"].as_u64().unwrap_or(0) as usize;
                if (undo == 0) == (redo == 0) {
                    return Err("Give a number of steps to undo or to redo.".into());
                }
                let done = match undo {
                    0 => self.redo_steps(ctx, redo),
                    n => self.undo_steps(ctx, n),
                };
                // Undoing is no new step: it moves steps between the lists.
                Ok((json!({"done": done}), None))
            }
            "switch_version" => {
                let track = args["track"].as_str().ok_or("Give the track's id.")?;
                let version = args["version"].as_str().ok_or("Give the version's id.")?;
                let ti = (self.tracks.iter())
                    .position(|t| t.key == track)
                    .ok_or_else(|| format!("There is no track {track}."))?;
                if !self.tracks[ti].versions.iter().any(|v| v.id == version) {
                    return Err(format!("The track has no version {version}."));
                }
                if !self.switch_version(ctx, ti, version) {
                    return Err(self
                        .message
                        .take()
                        .unwrap_or("That version already plays.".into()));
                }
                Ok((
                    json!({"playing": version}),
                    Some("Switch track version".into()),
                ))
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
            "export_project_file" => {
                let path = args["path"].as_str().ok_or("Give the file's full path.")?;
                let r = self.write_dawproject(std::path::Path::new(path))?;
                Ok((
                    json!({"written": path, "left_out": crate::interchange::report_value(&r)}),
                    None,
                ))
            }
            "export_sections" => {
                let only: Option<Vec<String>> = match &args["sections"] {
                    Value::Null => None,
                    Value::Array(a) => Some(
                        (a.iter())
                            .map(|v| v.as_str().map(String::from))
                            .collect::<Option<_>>()
                            .ok_or("Sections are named by marker id or name, as text.")?,
                    ),
                    _ => return Err("Give the sections as a list.".into()),
                };
                use daw_engine::export::Format;
                let format = match args["format"].as_str() {
                    None => None,
                    Some("wav") => Some(Format::Wav),
                    Some("flac") => Some(Format::Flac),
                    Some("mp3") => Some(Format::Mp3(320)),
                    Some("opus") => Some(Format::Opus(160)),
                    Some(f) => return Err(format!("There is no file type {f}.")),
                };
                let along = (args["play_along"].as_bool()).unwrap_or(self.export.play_along());
                // A folder given is for this export only.
                let before = self.export.chosen().map(std::path::Path::to_path_buf);
                if let Some(f) = args["folder"].as_str() {
                    self.export.set_chosen(Some(f.into()));
                }
                self.message = None;
                let queued = self.export.busy();
                let r = self.export_sections(only.as_deref(), along, format);
                if args["folder"].is_string() {
                    self.export.set_chosen(before);
                }
                let label = r.ok_or_else(|| self.message.clone().unwrap_or_default())?;
                Ok((json!({"export": label, "queued": queued}), None))
            }
            "import_project_file" => {
                let path = args["path"].as_str().ok_or("Give the file's full path.")?;
                if self.proj.dirty && self.proj.folder.is_none() && !self.tracks.is_empty() {
                    return Err(
                        "This project has unsaved work: save it, or start a new project, first."
                            .into(),
                    );
                }
                let r = self.import_dawproject(ctx, std::path::Path::new(path))?;
                Ok((
                    json!({"imported": path, "left_out": crate::interchange::report_value(&r)}),
                    None,
                ))
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
            "import_from_project" => self.import_tool(ctx, args),
            "export_as_project" => {
                let name = args["name"].as_str().unwrap_or("").trim();
                let pick = Pick {
                    tracks: self.track_list(args)?,
                    routing: args["routing"].as_bool().unwrap_or(true),
                    markers: args["markers"].as_bool().unwrap_or(true),
                    tempo: true,
                    settings: args["settings"].as_bool().unwrap_or(true),
                    notes: args["notes"].as_bool().unwrap_or(true),
                };
                if name.is_empty() || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
                {
                    return Err("Give a name without / \\ : * ? \" < > |.".into());
                }
                let dir = crate::project::projects_dir().join(name);
                let bars = bars_of(args)?;
                self.export_as_project(ctx, &dir, &pick, bars)?;
                Ok((
                    json!({"saved": dir.display().to_string(), "message": self.message.clone()}),
                    None,
                ))
            }
            "export_stem_package" => {
                let keys = self.track_list(args)?;
                let d = StemOptions::default();
                let opts = StemOptions {
                    audio: args["audio"].as_bool().unwrap_or(d.audio),
                    notes: args["notes"].as_bool().unwrap_or(d.notes),
                    mix: args["mix"].as_bool().unwrap_or(d.mix),
                    flac: args["flac"].as_bool().unwrap_or(d.flac),
                    archive: args["archive"].as_bool().unwrap_or(d.archive),
                };
                let into = self.export.dir().to_path_buf();
                self.export_stem_package(&into, &keys, bars_of(args)?, opts)?;
                Ok((
                    json!({"into": into.display().to_string(), "started": true}),
                    None,
                ))
            }
            _ => match self.pane_tool(ctx, name, args, access) {
                Some(done) => done,
                None => Err(format!("There is no tool {name}.")),
            },
        }
    }

    /// The tracks a call names by id, or every track.
    fn track_list(&self, args: &Value) -> Result<Vec<String>, String> {
        let Some(list) = args["tracks"].as_array() else {
            return Ok(self.tracks.iter().map(|t| t.key.clone()).collect());
        };
        (list.iter())
            .map(|k| {
                let k = k.as_str().ok_or("Track ids are text.")?;
                (self.tracks.iter().any(|t| t.key == k))
                    .then(|| k.to_string())
                    .ok_or_else(|| format!("There is no track {k}."))
            })
            .collect()
    }

    fn import_tool(
        &mut self,
        ctx: &egui::Context,
        args: &Value,
    ) -> Result<(Value, Option<String>), String> {
        let name = args["project"]
            .as_str()
            .ok_or("Give the other project's folder.")?;
        let path = match std::path::Path::new(name) {
            p if p.is_absolute() => p.to_path_buf(),
            _ => crate::project::projects_dir().join(name),
        };
        let (_, doc, _) = daw_collab::open_doc(&path, daw_project::new_peer())?;
        let src = doc.get();
        let tracks = match args["tracks"].as_array() {
            None => src.tracks.iter().map(|t| t.id.clone()).collect(),
            Some(list) => (list.iter())
                .map(|k| {
                    let k = k.as_str().ok_or("Tracks are given as text.")?;
                    (src.tracks.iter())
                        .find(|t| t.id == k || t.name == k)
                        .map(|t| t.id.clone())
                        .ok_or_else(|| format!("That project has no track {k}."))
                })
                .collect::<Result<Vec<_>, String>>()?,
        };
        let pick = Pick {
            tracks,
            routing: args["routing"].as_bool().unwrap_or(true),
            markers: args["markers"].as_bool().unwrap_or(false),
            tempo: args["tempo"].as_bool().unwrap_or(false),
            settings: args["settings"].as_bool().unwrap_or(false),
            notes: args["notes"].as_bool().unwrap_or(false),
        };
        let label = "Assistant: Import from project".to_string();
        self.proj.label = Some(label);
        let brought = self.import_parts(ctx, &path, &pick);
        self.proj.label = None;
        let brought = brought?;
        Ok((
            json!({"tracks": brought.tracks, "markers": brought.markers, "message": self.message.clone()}),
            None,
        ))
    }

    fn exports_value(&self) -> Value {
        let done: Vec<Value> = (self.export.results().iter())
            .map(|(folder, lines)| {
                let files: Vec<Value> = (lines.iter())
                    .map(|(name, r)| match r {
                        Ok(w) => json!({"name": name, "clipped": w.clipped()}),
                        Err(e) => json!({"name": name, "error": e}),
                    })
                    .collect();
                json!({"folder": folder.display().to_string(), "files": files})
            })
            .collect();
        let queue: Vec<&str> = self.export.queue().map(|q| q.label.as_str()).collect();
        json!({
            "running": self.export.progress().map(|p| json!({"done": p})),
            "queue": queue,
            "finished": done,
        })
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
                    "everyday": i.everyday,
                    "offered": self.offered(i.cmd),
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
                            "tempo": c.tempo,
                            "beat": c.tempo.is_some().then_some(c.beat),
                            "markers": c.markers.iter().map(|m| json!({"frame": m.frame, "at": m.at})).collect::<Vec<_>>(),
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
                                        "id": n.id,
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
                    "note_effects": t.note_fx.iter().map(|f| json!({
                        "id": f.id, "kind": f.kind, "bypass": f.bypass,
                    })).collect::<Vec<_>>(),
                    "frozen": t.freeze,
                    "automated": t.automation.iter().map(|l| &l.param).collect::<Vec<_>>(),
                    "clips": clips,
                    "note_clips": note_clips,
                    "versions": t.versions.iter().map(|v| json!({
                        "id": v.id, "name": v.name,
                        "playing": t.version.as_ref() == Some(&v.id),
                    })).collect::<Vec<_>>(),
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
            "speed_pct": self.speed_pct,
            "semitones": self.semitones,
            "cents": self.cents,
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
            params.extend(crate::notefx::parameters(&t.note_fx));
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
            p if p.starts_with(crate::notefx::PARAM_PREFIX) => {
                crate::notefx::set_parameter(&t.name, &mut t.note_fx, p, v)?
            }
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
            // Renders answer from a thread of their own, so allow time.
            let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while std::time::Instant::now() < until {
                self.h.step();
                if let Ok(a) = answer.try_recv() {
                    if tool == "seek"
                        && let Ok(v) = &a
                    {
                        self.wait_for_playhead(v["frames"].as_i64().unwrap_or(0));
                    }
                    return a;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("{tool} was not answered");
        }

        /// Waits for the engine to move the playhead: it does so on its
        /// own thread, which a slow machine may not have run yet.
        fn wait_for_playhead(&mut self, at: i64) {
            for _ in 0..200 {
                // Playing, it may have moved on a little already.
                let pos = self.h.state().ctl.status().position().0;
                if (at..at + 4800).contains(&pos) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
                self.h.step();
            }
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
        for t in all_tools() {
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
    fn practice_speed_is_set_and_seen_through_the_commands() {
        let mut d = Driver::new();
        for _ in 0..6 {
            d.command("slower-speed");
        }
        let t = d.call("get_transport", json!({})).unwrap();
        assert_eq!(t["speed_pct"], 70.0);
        d.command("faster-speed");
        assert_eq!(
            d.call("get_transport", json!({})).unwrap()["speed_pct"],
            75.0
        );
        d.command("normal-speed");
        let t = d.call("get_transport", json!({})).unwrap();
        assert_eq!(
            (t["speed_pct"].as_f64(), t["semitones"].as_i64()),
            (Some(100.0), Some(0))
        );
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
        // The engine moves on its own thread: give it a moment, as a
        // slow machine may not have run a block yet.
        let mut t = Value::Null;
        for _ in 0..100 {
            d.h.run();
            t = d.call("get_transport", json!({})).unwrap();
            if t["bar"] == json!(3) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!((t["bar"].clone(), t["beat"].clone()), (json!(3), json!(1)));
    }
}

/// Commands that ask for a file and have a tool of their own, which
/// assistants use instead.
pub(crate) const OWN_TOOL: &[(Cmd, &str)] = &[
    (Cmd::ExportDawProject, "export_project_file"),
    (Cmd::ImportDawProject, "import_project_file"),
];

/// The checks that keep the interface whole as features are added
/// (roadmap 2.11): every command in the table works from an assistant.
#[cfg(test)]
mod coverage {
    use serde_json::json;

    use super::driver::Driver;
    use super::*;
    use crate::agent_files::FILE_COMMANDS;

    /// Runs every `parts`th command from `part` from an assistant: none
    /// may panic or leave the request unanswered (`call` would time out).
    /// A fresh app every few, as windows piling up make frames slow.
    fn run_commands(part: usize, parts: usize) {
        let mut d = Driver::new();
        for (n, i) in COMMANDS.iter().skip(part).step_by(parts).enumerate() {
            if n > 0 && n % 12 == 0 {
                d = Driver::new();
            }
            let id = i.id();
            match d.call("run_command", json!({"id": id})) {
                Ok(v) => assert_eq!(v["ran"], id),
                Err(e) => assert!(
                    e.contains("can't run now")
                        || e.contains("file chooser")
                        || e.contains("permission"),
                    "{id}: {e}"
                ),
            }
        }
    }

    // In four, so they run side by side.
    #[test]
    fn every_command_runs_from_an_assistant_1() {
        run_commands(0, 4);
    }

    #[test]
    fn every_command_runs_from_an_assistant_2() {
        run_commands(1, 4);
    }

    #[test]
    fn every_command_runs_from_an_assistant_3() {
        run_commands(2, 4);
    }

    #[test]
    fn every_command_runs_from_an_assistant_4() {
        run_commands(3, 4);
    }

    #[test]
    fn every_command_that_asks_for_a_file_has_another_way_in() {
        for &cmd in NEEDS_A_PERSON {
            let by_file = FILE_COMMANDS.iter().any(|(c, _)| *c == cmd);
            let own = (OWN_TOOL.iter()).find(|(c, _)| *c == cmd).map(|(_, t)| *t);
            assert!(
                by_file || own.is_some_and(|t| tool(t).is_some()),
                "{}: an assistant has no way to run it with a file given; add it to \
                 FILE_COMMANDS or OWN_TOOL",
                cmd.info().id()
            );
        }
        for (c, _) in FILE_COMMANDS {
            assert!(NEEDS_A_PERSON.contains(c), "{}", c.info().id());
        }
    }

    #[test]
    fn a_file_command_runs_with_its_file() {
        let mut d = Driver::new();
        d.access = Access::Edit;
        let dir = std::env::temp_dir().join(format!("daw-file-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mid = dir.join("notes.mid");
        let e = d
            .call("file_command", json!({"id": "export-midi", "path": mid}))
            .unwrap_err();
        assert!(e.contains("permission"), "{e}");
        d.access = Access::EditExport;
        d.command("add-instrument-track");
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        let c = d
            .call("add_note_clip", json!({"track": t, "bar": 1}))
            .unwrap();
        d.call(
            "edit_notes",
            json!({"clip": c["clip"], "add": [{"beat": 0, "beats": 1, "pitch": 60}]}),
        )
        .unwrap();
        d.call("file_command", json!({"id": "export-midi", "path": mid}))
            .unwrap();
        assert!(mid.is_file());
        d.call("file_command", json!({"id": "import-midi", "path": mid}))
            .unwrap();
        assert!(d.project()["tracks"].as_array().unwrap().len() >= 2);
        assert!(
            d.call("file_command", json!({"id": "play", "path": mid}))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
