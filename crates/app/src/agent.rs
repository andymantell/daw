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
                      to a quarter note). An audio clip's start and end_seconds are on \
                      the timeline; its offset and len are in the file's own audio, so \
                      a clip that follows tempo or has stretch markers can end before or \
                      after start + len.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "notes": {"type": "boolean",
                          "description": "Include every note of every note clip (get_note_clip gives one clip's)."},
                "track": {"type": "string", "description": "Only this track, by id."},
                "summary": {"type": "boolean",
                            "description": "Each track's clips as a count and where they \
                             start and end, rather than one by one."}
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
        description: "The selected clips, by track and clip id, the selected note clip, \
                      the mixer strips selected (by track id; their faders move together \
                      when one is dragged), and whether the mixer shows narrow strips \
                      (the mixer-narrow command switches it).",
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
                      mute, solo, sends, each effect's settings, and its built-in \
                      instrument's, named instrument.<key>), with its value, range and \
                      how it reads, and whether it is automated. The track \"master\" is \
                      the master channel: its volume and its effects' settings, which can be automated too. Without a \
                      track, every track's and the master's; a drum machine has hundreds, \
                      so ask for one track, or give a prefix such as \"instrument.pad0.\". \
                      Every parameter of a plugin is listed: narrow them with effect, \
                      prefix or search, and page through them with offset and limit (each \
                      track's list says how many it has in all). To make a setting change \
                      over time as the song plays, use edit_automation.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string", "description": "A track's id."},
                "prefix": {"type": "string", "description": "Only the settings whose param \
                           starts so, such as \"instrument.pad0.\" or \"fx:\"."},
                "effect": {"type": "string", "description": "Only this effect's settings, by its id."},
                "search": {"type": "string", "description": "Only settings whose name or id has this text in it."},
                "offset": {"type": "integer", "description": "Skip this many of each track's settings (after the filters)."},
                "limit": {"type": "integer", "description": "At most this many of each track's settings; 500 if left out."}
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
                      correlation, the level in each third-octave band from 20 Hz to \
                      20 kHz, and the key and tempo it sounds in, when it can tell. Of one track or bus on its own (after its fader), or of the \
                      mix (no track, or the track \"master\": through the master's \
                      effects and fader), maybe without one track, or of one clip. Over a section, bars or seconds; \
                      without any: the loop if it is on, else the whole song (at most 10 \
                      minutes). get_meters gives each track's level as it plays.",
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
        name: "wait_for_change",
        description: "Waits until the project changes (by the person, another member, or an \
                      assistant) or an export finishes, then answers with its new \
                      revision; or answers at once \
                      with the revision now if none is given. Give the revision last seen \
                      to hear of the next change, and get_history or get_project for what \
                      it was. Waits up to timeout_seconds (30 if left out, at most 60).",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "revision": {"type": "integer"},
                "timeout_seconds": {"type": "number"}
            }})
        },
    },
    Tool {
        name: "run_command",
        description: "Runs a command by its id, as its menu entry or key would. \
                      list_commands gives the ids. Commands that act on an instrument \
                      track or a note clip (note effects, layers, quantise and the other \
                      note tools, patterns) act on the track or clip given, as if it \
                      were open; otherwise on the one open or chosen, or the first. \
                      Commands that save or write files need the edit and export \
                      permission; commands that open a file chooser need a person and \
                      are refused.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "id": {"type": "string", "description": "The command's id, such as \"split\"."},
                "track": {"type": "string", "description": "The track it acts on, by id."},
                "clip": {"type": "string", "description": "The note clip it acts on, by id."}
            }, "required": ["id"]})
        },
    },
    Tool {
        name: "set_parameter",
        description: "Sets one track setting to a value, the same all through the song, \
                      or the master channel's (track \"master\"). list_parameters gives \
                      each setting's param name and range. Mute and solo take 0 or 1. For \
                      a setting that changes as the song plays (a volume ride, fade, filter \
                      sweep or pan move), use edit_automation.",
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
        name: "set_parameters",
        description: "Sets many settings in one call, as one undo step: a synth's whole \
                      voicing, or a mix move across tracks. Each change is a track, a \
                      param and a value, as set_parameter takes them; a change with no \
                      track uses the call's track. All or none: if one can't be set (no \
                      such track or setting), nothing changes and the answer says which. \
                      Values are kept in their ranges.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string", "description": "The track for changes that give none, by id (or \"master\")."},
                "changes": {"type": "array", "items": {"type": "object", "properties": {
                    "track": {"type": "string", "description": "The track's id, if not the call's."},
                    "param": {"type": "string", "description": "The setting, as list_parameters names it."},
                    "value": {"type": "number"}
                }, "required": ["param", "value"]}}
            }, "required": ["changes"]})
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
        description: "Selects these clips, by id, in place of the selection: audio clips, \
                      and at most one note clip. An empty list clears it. Commands such as \
                      split, delete and crossfade act on it.",
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
        description: "The exports: how far the one running has got (with the seconds it \
                      has taken so far and a guess at the seconds left), those waiting in the \
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
        "to_seconds": {"type": "number"},
        "from_bar": {"type": "integer", "description": "From the start of this bar (from 1)."},
        "to_bar": {"type": "integer", "description": "To the start of this bar."},
        "section": {"type": "string", "description": "A section by its marker's name or id: \
                    from its marker to the next."},
        "clip": {"type": "string", "description": "An audio clip's id: it alone, through its \
                 track's effects and fader, from its start to its end unless a stretch is given."},
        "transpose_semitones": {"type": "number", "description": "Hear the track or clip as if \
                 its audio clips were moved by this much more (-12 to 12; fractions are cents), \
                 without changing them: to try a shift before making it."},
        "notes": {"type": "boolean", "description": "analyse only: also read the notes of one \
                 voice or instrument, one at a time: each note with its start, length and cents \
                 off, seconds of each of the 12 notes, the overall tuning, and the key they fit."},
        "beats": {"type": "boolean", "description": "analyse only: also find the beat grid of a \
                 voice alone or another sparse part, from its note starts: tempo candidates with \
                 confidence, where the first beat and first downbeat fall, and how far the beat \
                 drifts."}
    }})
}

/// Every tool: these, then those for what the panes do.
pub(crate) fn all_tools() -> impl Iterator<Item = &'static Tool> {
    (TOOLS.iter())
        .chain(crate::agent_edit::TOOLS)
        .chain(crate::agent_arrange::TOOLS)
        .chain(crate::agent_automation::TOOLS)
        .chain(crate::agent_files::TOOLS)
        .chain(crate::agent_takes::TOOLS)
        .chain(crate::agent_notes::TOOLS)
        .chain(crate::agent_layers::TOOLS)
        .chain(crate::agent_routing::TOOLS)
        .chain(crate::agent_lessons::TOOLS)
        .chain(crate::agent_monitor::TOOLS)
        .chain(crate::agent_practice::TOOLS)
        .chain(crate::agent_pitch::TOOLS)
        .chain(crate::lyrics::TOOLS)
        .chain(crate::agent_plugins::TOOLS)
        .chain(crate::review::TOOLS)
        .chain(crate::agent_midi::TOOLS)
        .chain(crate::spoken::TOOLS)
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
    Cmd::LoadSfz,
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
    /// Counts the project's changes, by anyone, since the app started.
    pub(crate) revision: u64,
    /// `wait_for_change` calls waiting.
    waiting: Vec<Waiting>,
    /// How many exports had finished when last looked.
    exported: usize,
}

/// A `wait_for_change` call: the revision seen, until when, and where
/// its answer goes.
type Waiting = (u64, std::time::Instant, Sender<Result<Value, String>>);

impl Default for Agent {
    fn default() -> Agent {
        let (inbox, requests) = mpsc::channel();
        Agent {
            inbox,
            requests,
            running: Vec::new(),
            revision: 0,
            waiting: Vec::new(),
            exported: 0,
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
        // An export finishing counts as a change, so an assistant can
        // wait for one.
        let exported = self.export.results().len();
        if exported != self.agent.exported {
            self.agent.exported = exported;
            self.agent.revision += 1;
        }
        let now = std::time::Instant::now();
        let rev = self.agent.revision;
        let (done, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.waiting)
            .into_iter()
            .partition(|(seen, until, _)| *seen != rev || now >= *until);
        self.agent.waiting = wait;
        for (seen, _, reply) in done {
            let _ = reply.send(Ok(json!({"revision": rev, "changed": seen != rev})));
        }
        if !self.agent.waiting.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
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
            if t.name == "wait_for_change" {
                let rev = self.agent.revision;
                let Some(seen) = req.args["revision"].as_u64() else {
                    let _ = req
                        .reply
                        .send(Ok(json!({"revision": rev, "changed": false})));
                    continue;
                };
                let secs = req.args["timeout_seconds"].as_f64().unwrap_or(30.0);
                let until = now + std::time::Duration::from_secs_f64(secs.clamp(0.0, 60.0));
                self.agent.waiting.push((seen, until, req.reply));
                ctx.request_repaint();
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
            "get_project" => {
                let mut v = self.project_value(args["notes"].as_bool().unwrap_or(false));
                if let Some(only) = args["track"].as_str() {
                    let tracks = v["tracks"]
                        .as_array_mut()
                        .ok_or("The project has no tracks.")?;
                    tracks.retain(|t| t["id"] == only);
                    if tracks.is_empty() {
                        return Err(format!("There is no track {only}."));
                    }
                }
                if args["summary"].as_bool() == Some(true) {
                    self.summarise_clips(&mut v);
                }
                Ok(v)
            }
            "get_transport" => Ok(self.transport_value()),
            "get_selection" => Ok(self.selection_value()),
            "get_fretboard" => Ok(self.fretboard_value()),
            "list_parameters" => self
                .parameters_value(args["track"].as_str())
                .map(|v| narrow(v, args)),
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
            "get_automation" => self.automation_value(args["track"].as_str()),
            "get_suggestions" => Ok(self.suggestions_value()),
            _ => (self.takes_read(name, args))
                .or_else(|| self.notes_read_tool(name, args))
                .or_else(|| self.layers_read(name, args))
                .or_else(|| self.routing_read(name, args))
                .or_else(|| self.lessons_read(name))
                .or_else(|| self.monitor_read(name))
                .or_else(|| self.plugins_read(name))
                .or_else(|| self.midi_read(name))
                .or_else(|| self.spoken_read(name, args))
                .or_else(|| self.pitch_read(name, args))
                .or_else(|| self.lyrics_read(name, args))
                .unwrap_or_else(|| Err(format!("There is no tool {name}."))),
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
                // What it acts on, when given: a note clip (as if chosen
                // on the timeline and open in the note editor), a track
                // (as if its instrument window were open).
                if let Some(clip) = args["clip"].as_str() {
                    let ti = (self.tracks.iter())
                        .position(|t| t.note_clips.iter().any(|c| c.id == clip))
                        .ok_or_else(|| format!("There is no note clip {clip}."))?;
                    self.timeline.note_sel = Some(clip.to_string());
                    self.notes_editor.edit(clip);
                    if args["track"].is_null() {
                        self.instrument_open = Some(ti);
                    }
                }
                let before = self.instrument_open;
                let mut focus = None;
                if let Some(key) = args["track"].as_str() {
                    let ti = self
                        .track_index(key)
                        .ok_or_else(|| format!("There is no track {key}."))?;
                    self.instrument_open = Some(ti);
                    focus = Some(ti);
                }
                if !self.can_run(cmd) {
                    self.instrument_open = before;
                    return Err(format!("{id} can't run now."));
                }
                self.run(ctx, cmd);
                // Lend the focus for this command only, unless the command
                // opened the window itself.
                if focus.is_some() && cmd != Cmd::InstrumentWindow && self.instrument_open == focus
                {
                    self.instrument_open = before;
                }
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
            "set_parameters" => self.set_parameters_tool(args),
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
                let mut note = None;
                for k in keys {
                    let k = k.as_str().ok_or("Clip ids are text.")?;
                    if let Some(c) =
                        (self.tracks.iter().flat_map(|t| &t.clips)).find(|c| c.key == k)
                    {
                        ids.insert(c.id);
                    } else if (self.tracks.iter().flat_map(|t| &t.note_clips)).any(|c| c.id == k) {
                        if note.is_some() {
                            return Err("Only one note clip can be picked at a time.".into());
                        }
                        note = Some(k.to_string());
                    } else {
                        return Err(format!("There is no clip {k}."));
                    }
                }
                self.timeline.selected = ids;
                self.timeline.note_sel = note;
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
            "running": self.export.progress().map(|p| {
                let so_far = self.export.elapsed().unwrap_or(0.0);
                // A guess from the pace so far, once there is one.
                let left = (p > 0.01).then(|| so_far * (1.0 - p as f64) / p as f64);
                json!({"done": p, "seconds_so_far": so_far, "seconds_left": left})
            }),
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

    /// Puts a count and a span in place of each track's clip lists.
    fn summarise_clips(&self, v: &mut Value) {
        let rate = self.rate.max(1) as f64;
        let map = self.tempo_map();
        let Some(tracks) = v["tracks"].as_array_mut() else {
            return;
        };
        for t in tracks {
            let Some(app) = t["id"].as_str().and_then(|k| self.track_index(k)) else {
                continue;
            };
            let app = &self.tracks[app];
            let from = app.clips.iter().map(|c| c.start).min();
            let to = app.clips.iter().map(|c| c.end()).max();
            let o = t.as_object_mut().expect("a track is an object");
            o.remove("clips");
            o.insert(
                "clips_summary".into(),
                json!({
                    "count": app.clips.len(),
                    "muted": app.clips.iter().filter(|c| c.muted).count(),
                    "transposed": app.clips.iter().filter(|c| c.cents != 0).count(),
                    "follow_tempo": app.clips.iter().filter(|c| c.tempo.is_some()).count(),
                    "from_seconds": from.map(|f| f as f64 / rate),
                    "to_seconds": to.map(|f| f as f64 / rate),
                }),
            );
            let notes = o.remove("note_clips");
            let ticks = |k: &str| {
                (app.note_clips.iter())
                    .map(|c| {
                        if k == "from" {
                            c.start
                        } else {
                            c.start + c.len
                        }
                    })
                    .reduce(if k == "from" { i64::min } else { i64::max })
                    .map(|t| map.frames_at(daw_engine::time::Ticks(t)).0 as f64 / rate)
            };
            if notes.is_some_and(|n| n.as_array().is_some_and(|a| !a.is_empty())) {
                o.insert(
                    "note_clips_summary".into(),
                    json!({
                        "count": app.note_clips.len(),
                        "notes": app.note_clips.iter().map(|c| c.notes.len()).sum::<usize>(),
                        "from_seconds": ticks("from"),
                        "to_seconds": ticks("to"),
                    }),
                );
            }
        }
    }

    /// Where audio clip `id` ends on the timeline, in frames: after
    /// any follow-tempo stretch, so it can differ from start + len.
    pub(crate) fn clip_end(&self, id: &str) -> Option<i64> {
        (self.tracks.iter().flat_map(|t| &t.clips))
            .find(|c| c.key == id)
            .map(|c| c.end())
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
                "sounds": (!e.samples.is_empty()).then(|| {
                    e.samples.iter().map(|(slot, sound)| json!({"slot": slot, "sound": sound}))
                        .collect::<Vec<_>>()
                }),
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
                            "offset_seconds": secs(c.offset),
                            "end_seconds": self.clip_end(&c.id).map(secs),
                            "muted": c.muted,
                            "gain_db": c.gain_db,
                            "take": c.take,
                            "tempo": c.tempo,
                            "beat": c.tempo.is_some().then_some(c.beat),
                            "transpose_semitones": c.cents as f64 / 100.0,
                            "move_formants": c.move_formants,
                            "link": c.link,
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
                            "loop_ticks": c.looped(),
                            "link": c.link,
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
                    "group": t.group,
                    "output": t.output,
                    "sends": t.sends.iter().map(|s| json!({
                        "to": s.to, "gain_db": s.gain_db,
                        "pre_fader": s.pre_fader, "mute": s.mute,
                    })).collect::<Vec<_>>(),
                    "instrument": t.instrument.as_ref().map(effect),
                    "layers": t.layers.iter().map(|l| json!({
                        "id": l.id, "instrument": effect(&l.instrument),
                        "gain_db": l.gain_db, "pan": l.pan, "mute": l.mute,
                        "keys": [l.keys.0, l.keys.1],
                        "velocities": [l.velocities.0, l.velocities.1],
                        "transpose": l.transpose,
                    })).collect::<Vec<_>>(),
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
            // Clips that are instances of one clip: linked copies, and
            // note clips playing the same pattern.
            "linked_groups": daw_project::link_groups(&p).iter().map(|g| json!({
                "link": g.link, "notes": g.notes, "clips": g.clips,
            })).collect::<Vec<_>>(),
            "master": {
                "id": daw_project::Master::KEY,
                "gain_db": p.master.gain_db,
                "effects": p.master.effects.iter().map(effect).collect::<Vec<_>>(),
                "automated": p.master.automation.iter().map(|l| &l.param).collect::<Vec<_>>(),
            },
            "control_groups": p.control_groups.iter().map(|g| json!({
                "id": g.id, "name": g.name, "gain_db": g.gain_db, "mute": g.mute,
            })).collect::<Vec<_>>(),
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
            "mixer_strips": self.desk.selected,
            "mixer_narrow": self.desk.narrow,
        })
    }

    /// The master channel's settings for `list_parameters`: its fader
    /// and its effects'.
    fn master_parameters(&self) -> Value {
        let t = &self.master;
        let params: Vec<Value> = (crate::automation::choices_up_to(t, &[], usize::MAX).iter())
            .filter_map(|c| {
                let v = crate::automation::control(t, &c.param)?;
                let r = crate::automation::range(t, &c.param)?;
                let mut p = json!({
                    "param": c.param,
                    "name": c.name,
                    "value": v,
                    "text": crate::automation::text_of(t, &c.param, v),
                    "min": r.min,
                    "max": r.max,
                    "automated": t.automation.iter().any(|l| l.param == c.param),
                });
                let named = crate::automation::choice_names(t, &c.param);
                if !named.is_empty() {
                    p["choices"] = json!(named);
                }
                Some(p)
            })
            .collect();
        json!({"track": t.key, "name": t.name, "params": params})
    }

    fn parameters_value(&self, only: Option<&str>) -> Result<Value, String> {
        if only == Some(daw_project::Master::KEY) {
            return Ok(Value::Array(vec![self.master_parameters()]));
        }
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
            // The instrument's are listed below, with its own names.
            for c in crate::automation::choices_up_to(t, &buses, usize::MAX)
                .into_iter()
                .filter(|c| !c.param.starts_with(INSTRUMENT))
            {
                let (Some(v), Some(r)) = (
                    crate::automation::control(t, &c.param),
                    crate::automation::range(t, &c.param),
                ) else {
                    continue;
                };
                let mut p = json!({
                    "param": c.param,
                    "name": c.name,
                    "value": v,
                    "text": crate::automation::text_of(t, &c.param, v),
                    "min": r.min,
                    "max": r.max,
                    "automated": t.automation.iter().any(|l| l.param == c.param),
                });
                // A setting that picks one of a few named values (a
                // waveform, a mode, a number of stages) lists their
                // names, by value.
                let named = crate::automation::choice_names(t, &c.param);
                if !named.is_empty() {
                    p["choices"] = json!(named);
                } else if r.min.fract() == 0.0 && r.max.fract() == 0.0 && r.max - r.min <= 16.0 {
                    let names: Vec<String> = ((r.min as i64)..=(r.max as i64))
                        .map(|i| crate::automation::text_of(t, &c.param, i as f64))
                        .collect();
                    if names.iter().all(|n| !n.chars().any(|c| c.is_ascii_digit())) {
                        p["choices"] = json!(names);
                    }
                }
                params.push(p);
            }
            params.extend(crate::notefx::parameters(&t.note_fx));
            if let Some(slot) = &t.instrument {
                for (k, name, v, min, max, text) in slot.settings() {
                    let param = format!("{INSTRUMENT}{k}");
                    let automated = t.automation.iter().any(|l| l.param == param);
                    let mut p = json!({
                        "param": param, "name": name, "value": v,
                        "text": text, "min": min, "max": max, "automated": automated,
                    });
                    let names = slot.choices(&k);
                    if !names.is_empty() {
                        p["choices"] = json!(names);
                    }
                    params.push(p);
                }
            }
            out.push(json!({"track": t.key, "name": t.name, "params": params}));
        }
        if only.is_none() {
            out.push(self.master_parameters());
        }
        Ok(Value::Array(out))
    }

    /// Sets the master channel's fader or one of its effects' settings.
    fn set_master_parameter(&mut self, param: &str, v: f64) -> Result<String, String> {
        let t = &mut self.master;
        let r = crate::automation::range(t, param)
            .filter(|_| param != daw_project::Lane::PAN)
            .filter(|_| crate::automation::control(t, param).is_some())
            .ok_or_else(|| format!("The master has no setting {param}."))?;
        let v = v.clamp(r.min, r.max);
        crate::automation::set_control(t, param, v);
        let label = format!(
            "Set master {} to {}",
            crate::automation::name_of(t, &[], param),
            crate::automation::text_of(t, param, v)
        );
        if param == daw_project::Lane::GAIN {
            let db = self.master.mix.gain_db;
            self.send(Command::SetMasterGain(db));
        } else {
            self.rebuild();
        }
        Ok(label)
    }

    /// Sets one of a track's instrument's settings, through the project
    /// as the document has it, so the instrument takes it as on undo.
    fn set_instrument_setting(&mut self, i: usize, k: &str, v: f64) -> Result<String, String> {
        let t = &self.tracks[i];
        let (name, min, max) = (t.instrument.as_ref())
            .and_then(|s| s.settings().into_iter().find(|x| x.0 == k))
            .map(|x| (x.1, x.3, x.4))
            .ok_or_else(|| format!("{}'s instrument has no setting {k}.", t.name))?;
        let (track, key) = (t.name.clone(), t.key.clone());
        let v = v.clamp(min, max);
        self.edit_model(|p| {
            let e = (p.tracks.iter_mut())
                .find(|t| t.id == key)
                .and_then(|t| t.instrument.as_mut())
                .ok_or("The instrument has gone.")?;
            e.params.retain(|(x, _)| x != k);
            e.params.push((k.to_string(), v));
            Ok(Value::Null)
        })?;
        Ok(format!("Set {track} {name} to {v}"))
    }

    /// `set_parameters`: every change or none, as one undo step.
    fn set_parameters_tool(&mut self, args: &Value) -> Result<(Value, Option<String>), String> {
        let changes = args["changes"]
            .as_array()
            .filter(|c| !c.is_empty())
            .ok_or("Give the changes: a list of {track, param, value}.")?;
        let default = args["track"].as_str();
        let mut list = Vec::with_capacity(changes.len());
        for (n, c) in changes.iter().enumerate() {
            let track = (c["track"].as_str())
                .or(default)
                .ok_or_else(|| format!("Change {n} has no track, and the call gives none."))?;
            let param = (c["param"].as_str()).ok_or_else(|| format!("Change {n} has no param."))?;
            let value = (c["value"].as_f64())
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("Change {n}'s value isn't a number."))?;
            list.push((track, param, value));
        }
        // Undone as a whole if one fails, so a half-set voicing can't stay.
        let before = self.project();
        let committed = self.proj.committed.clone();
        let mut labels = Vec::with_capacity(list.len());
        for (n, (track, param, value)) in list.iter().enumerate() {
            match self.set_parameter(track, param, *value) {
                Ok(l) => labels.push(l),
                Err(e) => {
                    let now = self.project();
                    if now != before {
                        self.reload_changed_fx(&now, &before);
                        self.apply(before);
                        self.proj.committed = committed;
                        self.send(Command::SetMasterGain(self.master.mix.gain_db));
                    }
                    return Err(format!(
                        "Change {n} ({track} {param}): {e} Nothing was changed."
                    ));
                }
            }
        }
        let label = match labels.as_slice() {
            [one] => one.clone(),
            _ => format!("Set {} settings", labels.len()),
        };
        Ok((json!({"set": labels.len(), "changes": labels}), Some(label)))
    }

    /// Sets a track's setting. The undo step's name, or why it can't.
    fn set_parameter(&mut self, key: &str, param: &str, v: f64) -> Result<String, String> {
        if !v.is_finite() {
            return Err("The value has to be a number.".into());
        }
        if key == daw_project::Master::KEY {
            return self.set_master_parameter(param, v);
        }
        let i = self
            .track_index(key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        if let Some(k) = param.strip_prefix(INSTRUMENT) {
            return self.set_instrument_setting(i, k, v);
        }
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

/// `list_parameters`' answer narrowed to what `args` ask for: one
/// effect's, those matching a search, and a page of each track's.
fn narrow(mut v: Value, args: &Value) -> Value {
    let effect = args["effect"].as_str();
    let prefix = args["prefix"].as_str();
    let search = args["search"].as_str().map(str::to_lowercase);
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(500) as usize;
    for t in v.as_array_mut().into_iter().flatten() {
        let all = t["params"].take();
        let kept: Vec<Value> = (all.as_array().into_iter().flatten())
            .filter(|p| {
                let param = p["param"].as_str().unwrap_or_default();
                if prefix.is_some_and(|pre| !param.starts_with(pre)) {
                    return false;
                }
                effect.is_none_or(|e| {
                    let of = |prefix: &str| {
                        (param.strip_prefix(prefix))
                            .and_then(|r| r.strip_prefix(e))
                            .is_some_and(|r| r.starts_with(':'))
                    };
                    of(daw_project::Lane::FX) || of(daw_project::Lane::PLUGIN)
                }) && search.as_ref().is_none_or(|s| {
                    param.to_lowercase().contains(s)
                        || p["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(s)
                })
            })
            .cloned()
            .collect();
        t["total"] = json!(kept.len());
        if offset > 0 {
            t["offset"] = json!(offset);
        }
        t["params"] = kept.into_iter().skip(offset).take(limit).collect();
    }
    v
}

/// Mute and solo, which aren't automation lanes, by the names
/// `set_parameter` takes.
const MUTE: &str = "mute";
/// How an instrument's settings are named to assistants.
const INSTRUMENT: &str = daw_project::Lane::INSTRUMENT;
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

    /// The track's instrument settings, by key, from list_parameters.
    fn instrument_params(d: &mut Driver, key: &str) -> Vec<Value> {
        let list = d.call("list_parameters", json!({"track": key})).unwrap();
        (list[0]["params"].as_array().unwrap().iter())
            .filter(|p| p["param"].as_str().unwrap().starts_with(INSTRUMENT))
            .cloned()
            .collect()
    }

    /// The vintage toolkit through the synth: an assistant picks a
    /// saturating filter by name, sets drive and drift, and the note
    /// sounds (plan docs/plans/classic-synths.md, step 0).
    #[test]
    fn the_vintage_filters_reach_the_assistant() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let params = instrument_params(&mut d, &t);
        let filter = (params.iter())
            .find(|p| p["param"] == "instrument.filter")
            .unwrap();
        let choices = filter["choices"].as_array().unwrap();
        let ladder = choices.iter().position(|c| c == "Ladder 24 dB").unwrap();
        assert!(choices.iter().any(|c| c == "Diode ladder"));
        for key in ["instrument.drive", "instrument.vintage"] {
            assert!(params.iter().any(|p| p["param"] == key), "{key}");
        }
        d.call(
            "set_parameters",
            json!({"track": t, "changes": [
                {"param": "instrument.filter", "value": ladder},
                {"param": "instrument.drive", "value": 0.8},
                {"param": "instrument.vintage", "value": 0.5},
                {"param": "instrument.cutoff", "value": 400.0},
                {"param": "instrument.resonance", "value": 0.7},
            ]}),
        )
        .unwrap();
        d.call(
            "add_note_clip",
            json!({"track": t, "bar": 1, "bars": 1, "notes": [
                {"beat": 0.0, "beats": 2.0, "pitch": 33, "velocity": 110}
            ]}),
        )
        .unwrap();
        let a = d
            .call("analyse", json!({"track": t, "from_bar": 1, "to_bar": 2}))
            .unwrap();
        assert!(a["sample_peak_dbfs"].as_f64().unwrap() > -30.0, "{a}");
        let now = instrument_params(&mut d, &t);
        let f = (now.iter())
            .find(|p| p["param"] == "instrument.filter")
            .unwrap();
        assert_eq!(f["text"], "Ladder 24 dB");
    }

    /// The Oh-One monosynth from the assistant's side (plan
    /// docs/plans/classic-synths.md, S3): add it, voice a house bass
    /// from its settings by name, play a line with a slide, render it,
    /// and read the settings back.
    #[test]
    fn the_oh_one_is_voiced_played_and_read_through_the_assistant() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "oh-one", "name": "House bass"}))
            .unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let params = instrument_params(&mut d, &t);
        let choice = |key: &str, name: &str| -> usize {
            let p = (params.iter())
                .find(|p| p["param"] == key)
                .unwrap_or_else(|| panic!("{key} is a setting"));
            (p["choices"].as_array().unwrap().iter())
                .position(|c| c == name)
                .unwrap_or_else(|| panic!("{key} offers {name}: {p}"))
        };
        for key in [
            "instrument.saw",
            "instrument.pulse",
            "instrument.sub",
            "instrument.noise",
            "instrument.cutoff",
            "instrument.resonance",
            "instrument.filter_env",
            "instrument.attack",
            "instrument.decay",
            "instrument.sustain",
            "instrument.release",
            "instrument.lfo_rate",
            "instrument.glide",
            "instrument.drive",
            "instrument.vintage",
        ] {
            assert!(params.iter().any(|p| p["param"] == key), "{key}");
        }
        let sub = choice("instrument.sub_shape", "2 octaves down");
        let pwm = choice("instrument.pwm_source", "LFO");
        let gate = choice("instrument.amp", "Gate");
        let slide = choice("instrument.portamento", "Overlapping notes only");
        d.call(
            "set_parameters",
            json!({"track": t, "changes": [
                {"param": "instrument.saw", "value": 1.0},
                {"param": "instrument.pulse", "value": 0.4},
                {"param": "instrument.pwm_source", "value": pwm},
                {"param": "instrument.sub", "value": 0.8},
                {"param": "instrument.sub_shape", "value": sub},
                {"param": "instrument.cutoff", "value": 300.0},
                {"param": "instrument.resonance", "value": 0.55},
                {"param": "instrument.filter_env", "value": 2.5},
                {"param": "instrument.decay", "value": 0.35},
                {"param": "instrument.amp", "value": gate},
                {"param": "instrument.portamento", "value": slide},
                {"param": "instrument.glide", "value": 0.08},
                {"param": "instrument.drive", "value": 0.4},
            ]}),
        )
        .unwrap();
        // A bar of eighths with the last two overlapping, so they slide.
        let notes: Vec<Value> = (0..8)
            .map(|i| {
                let beats = if i == 6 { 0.7 } else { 0.4 };
                json!({"beat": i as f64 * 0.5, "beats": beats,
                       "pitch": if i % 4 == 3 { 41 } else { 36 }, "velocity": 110})
            })
            .collect();
        d.call(
            "add_note_clip",
            json!({"track": t, "bar": 1, "bars": 1, "notes": notes}),
        )
        .unwrap();
        let a = d
            .call("analyse", json!({"track": t, "from_bar": 1, "to_bar": 2}))
            .unwrap();
        assert!(a["sample_peak_dbfs"].as_f64().unwrap() > -24.0, "{a}");
        let now = instrument_params(&mut d, &t);
        let text = |key: &str| {
            (now.iter().find(|p| p["param"] == key).unwrap())["text"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(text("instrument.amp"), "Gate");
        assert_eq!(text("instrument.sub_shape"), "2 octaves down");
        assert_eq!(text("instrument.cutoff"), "300 Hz");
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["instrument"]["kind"], "oh-one", "{i}");
        // And by its command, as a person would.
        d.command("add-oh-one-track");
        let p = d.project();
        assert_eq!(p["tracks"][1]["instrument"]["builtin"], "oh-one", "{p}");
    }

    /// The Acid Three bass line synth (plan docs/plans/classic-synths.md,
    /// S2) from the assistant's side: it adds the track by kind or by
    /// command, every setting is listed with its choices and can be
    /// set, and a line of plain notes, an accented note (velocity) and
    /// a slide (overlap) renders with the accent louder and brighter.
    #[test]
    fn the_acid_bass_synth_reaches_the_assistant() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "acid-three"})).unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let params = instrument_params(&mut d, &t);
        let keys: Vec<&str> = params
            .iter()
            .map(|p| p["param"].as_str().unwrap())
            .collect();
        for key in [
            "instrument.wave",
            "instrument.cutoff",
            "instrument.resonance",
            "instrument.env_mod",
            "instrument.decay",
            "instrument.accent",
            "instrument.accent_velocity",
            "instrument.slide",
            "instrument.drive",
        ] {
            assert!(keys.contains(&key), "{key} missing from {keys:?}");
        }
        let wave = (params.iter())
            .find(|p| p["param"] == "instrument.wave")
            .unwrap();
        let square = (wave["choices"].as_array().unwrap().iter())
            .position(|c| c == "Square")
            .unwrap();
        d.call(
            "set_parameters",
            json!({"track": t, "changes": [
                {"param": "instrument.wave", "value": square},
                {"param": "instrument.cutoff", "value": 400.0},
                {"param": "instrument.resonance", "value": 0.7},
                {"param": "instrument.env_mod", "value": 0.4},
                {"param": "instrument.decay", "value": 0.25},
                {"param": "instrument.accent_velocity", "value": 100},
                {"param": "instrument.vintage", "value": 0.0},
            ]}),
        )
        .unwrap();
        // Bar 1: a plain note. Bar 2: the same note accented. Bar 3: a
        // note that overlaps the next, which slides up to it.
        d.call(
            "add_note_clip",
            json!({"track": t, "bar": 1, "bars": 3, "notes": [
                {"beat": 0.0, "beats": 1.0, "pitch": 33, "velocity": 80},
                {"beat": 4.0, "beats": 1.0, "pitch": 33, "velocity": 120},
                {"beat": 8.0, "beats": 1.5, "pitch": 33, "velocity": 80},
                {"beat": 9.0, "beats": 1.0, "pitch": 45, "velocity": 80}
            ]}),
        )
        .unwrap();
        let bar = |d: &mut Driver, b: i64| -> Value {
            d.call(
                "analyse",
                json!({"track": t, "from_bar": b, "to_bar": b + 1}),
            )
            .unwrap()
        };
        let highs = |a: &Value| -> f64 {
            let power: f64 = (a["third_octave_bands"].as_array().unwrap().iter())
                .filter(|b| b["hz"].as_f64().unwrap() >= 1000.0)
                .map(|b| 10f64.powf(b["db"].as_f64().unwrap() / 10.0))
                .sum();
            10.0 * power.max(1e-12).log10()
        };
        let plain = bar(&mut d, 1);
        let accented = bar(&mut d, 2);
        let slid = bar(&mut d, 3);
        let peak = |a: &Value| a["sample_peak_dbfs"].as_f64().unwrap();
        assert!(peak(&plain) > -30.0, "{plain}");
        assert!(peak(&accented) > peak(&plain) + 2.0, "{plain} {accented}");
        assert!(highs(&accented) > highs(&plain) + 3.0, "{plain} {accented}");
        assert!(peak(&slid) > -30.0, "{slid}");
        let now = instrument_params(&mut d, &t);
        let w = (now.iter())
            .find(|p| p["param"] == "instrument.wave")
            .unwrap();
        assert_eq!(w["text"], "Square");
        // The command adds one too, and names the synth.
        d.command("add-acid-three-track");
        let tracks = d.project()["tracks"].as_array().unwrap().len();
        assert_eq!(tracks, 2);
        assert_eq!(
            d.project()["tracks"][1]["instrument"]["builtin"],
            "acid-three"
        );
    }

    /// The Model Mono through the assistant interface (classic synths,
    /// S1): add the track, read its settings and the sounds made in
    /// code, voice a bass from them, play notes and render.
    #[test]
    fn the_model_mono_is_played_through_the_assistant() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "model-mono"})).unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let params = instrument_params(&mut d, &t);
        for key in [
            "instrument.osc1_wave",
            "instrument.osc3_keys",
            "instrument.noise_colour",
            "instrument.overdrive",
            "instrument.cutoff",
            "instrument.emphasis",
            "instrument.contour",
            "instrument.key_track",
            "instrument.decay_release",
            "instrument.glide",
            "instrument.priority",
            "instrument.trigger",
        ] {
            assert!(params.iter().any(|p| p["param"] == key), "{key}");
        }
        let choices = |params: &[Value], key: &str| -> Vec<Value> {
            let p = params.iter().find(|p| p["param"] == key).unwrap();
            p["choices"].as_array().unwrap().clone()
        };
        assert!(
            choices(&params, "instrument.osc1_wave")
                .iter()
                .any(|c| c == "Shark tooth")
        );
        let ranges = choices(&params, "instrument.osc1_range");
        let low = ranges.iter().position(|c| c == "32'").unwrap();
        assert_eq!(choices(&params, "instrument.key_track").len(), 4);
        // The sounds made in code come with the settings that make them.
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        let sounds = info["instrument"]["sounds"].as_array().unwrap();
        let fat = (sounds.iter()).find(|s| s["name"] == "Fat bass").unwrap();
        d.call(
            "set_instrument",
            json!({"track": t, "settings": fat["settings"]}),
        )
        .unwrap();
        // Then voiced further: hotter, lower, with more emphasis.
        d.call(
            "set_parameters",
            json!({"track": t, "changes": [
                {"param": "instrument.overdrive", "value": 0.7},
                {"param": "instrument.emphasis", "value": 0.5},
                {"param": "instrument.cutoff", "value": 300.0},
                {"param": "instrument.osc1_range", "value": low},
            ]}),
        )
        .unwrap();
        d.call(
            "add_note_clip",
            json!({"track": t, "bar": 1, "bars": 1, "notes": [
                {"beat": 0.0, "beats": 1.5, "pitch": 33, "velocity": 110},
                {"beat": 1.0, "beats": 1.0, "pitch": 40, "velocity": 100},
                {"beat": 2.0, "beats": 2.0, "pitch": 36, "velocity": 110}
            ]}),
        )
        .unwrap();
        let a = d
            .call("analyse", json!({"track": t, "from_bar": 1, "to_bar": 2}))
            .unwrap();
        assert!(a["sample_peak_dbfs"].as_f64().unwrap() > -30.0, "{a}");
        let now = instrument_params(&mut d, &t);
        let f = (now.iter())
            .find(|p| p["param"] == "instrument.osc1_range")
            .unwrap();
        assert_eq!(f["text"], "32'");
        let f = (now.iter())
            .find(|p| p["param"] == "instrument.osc3_level")
            .unwrap();
        assert_eq!(f["value"], 0.5);
        // The command is in the table too.
        d.call("run_command", json!({"id": "add-model-mono-track"}))
            .unwrap();
        let p = d.call("get_project", json!({})).unwrap();
        assert_eq!(p["tracks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn many_settings_are_set_in_one_call_as_one_undo_step() {
        let mut d = Driver::new();
        let a = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let a = a["track"].as_str().unwrap().to_string();
        let b = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let b = b["track"].as_str().unwrap().to_string();
        let params = instrument_params(&mut d, &a);
        let (p0, p1) = (&params[0], &params[1]);
        let mid = |p: &Value| (p["min"].as_f64().unwrap() + p["max"].as_f64().unwrap()) / 2.0;
        let r = d
            .call(
                "set_parameters",
                json!({"track": a, "changes": [
                    {"param": p0["param"], "value": mid(p0)},
                    {"param": p1["param"], "value": p1["max"].as_f64().unwrap() + 100.0},
                    {"param": "gain", "value": -6.0},
                    {"track": b, "param": "mute", "value": 1},
                ]}),
            )
            .unwrap();
        assert_eq!(r["set"], 4);
        let now = instrument_params(&mut d, &a);
        assert_eq!(now[0]["value"], mid(p0));
        assert_eq!(now[1]["value"], p1["max"], "kept in its range");
        let p = d.project();
        assert_eq!(p["tracks"][0]["gain_db"], -6.0);
        assert_eq!(p["tracks"][1]["mute"], true);
        let undo = d.call("get_history", json!({})).unwrap()["undo"].clone();
        assert_eq!(undo[0]["label"], "Assistant: Set 4 settings", "{undo}");

        // One undo takes back every change.
        d.command("undo");
        let back = instrument_params(&mut d, &a);
        assert_eq!(back[0]["value"], p0["value"]);
        assert_eq!(back[1]["value"], p1["value"]);
        let p = d.project();
        assert_eq!(p["tracks"][0]["gain_db"], 0.0);
        assert_eq!(p["tracks"][1]["mute"], false);
        assert_eq!(p["tracks"].as_array().unwrap().len(), 2, "the tracks stay");
    }

    #[test]
    fn set_parameters_changes_nothing_if_one_change_fails() {
        let mut d = Driver::new();
        let a = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let a = a["track"].as_str().unwrap().to_string();
        let params = instrument_params(&mut d, &a);
        let p0 = &params[0];
        let before = d.call("get_history", json!({})).unwrap()["undo"].clone();
        let e = d
            .call(
                "set_parameters",
                json!({"track": a, "changes": [
                    {"param": p0["param"], "value": p0["max"]},
                    {"param": "gain", "value": -12.0},
                    {"track": "master", "param": "gain", "value": -3.0},
                    {"param": "no-such", "value": 1},
                ]}),
            )
            .unwrap_err();
        assert!(
            e.contains("Change 3") && e.contains("Nothing was changed"),
            "{e}"
        );
        assert_eq!(instrument_params(&mut d, &a)[0]["value"], p0["value"]);
        let p = d.project();
        assert_eq!(p["tracks"][0]["gain_db"], 0.0);
        assert_eq!(p["master"]["gain_db"], 0.0);
        assert_eq!(
            d.call("get_history", json!({})).unwrap()["undo"],
            before,
            "no undo step"
        );
        let e = d
            .call(
                "set_parameters",
                json!({"changes": [{"param": "gain", "value": 1}]}),
            )
            .unwrap_err();
        assert!(e.contains("no track"), "{e}");
    }

    #[test]
    fn the_project_is_read_for_one_track() {
        let mut d = Driver::new();
        d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let t = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let key = t["track"].as_str().unwrap();
        let v = d.call("get_project", json!({"track": key})).unwrap();
        let tracks = v["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["id"], key);
        assert!(d.call("get_project", json!({"track": "nope"})).is_err());
    }

    #[test]
    fn a_setting_that_picks_from_a_list_names_its_choices() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let key = t["track"].as_str().unwrap().to_string();
        let list = d.call("list_parameters", json!({"track": key})).unwrap();
        let params = list[0]["params"].as_array().unwrap();
        let named: Vec<&Value> = params
            .iter()
            .filter(|p| p.get("choices").is_some())
            .collect();
        assert!(
            named
                .iter()
                .any(|p| p["choices"].as_array().unwrap().iter().any(|c| c == "Saw")),
            "an oscillator names its waves: {named:?}"
        );
        let gain = params.iter().find(|p| p["param"] == "gain").unwrap();
        assert!(gain.get("choices").is_none(), "a level is a number");
    }

    #[test]
    fn effect_settings_read_back_without_rounding_and_name_their_choices() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "audio"})).unwrap();
        let key = t["track"].as_str().unwrap().to_string();
        let fx = d
            .call("add_effect", json!({"track": key, "effect": "phaser"}))
            .unwrap();
        let fx = fx["effect"].as_str().unwrap().to_string();
        let list = d.call("list_parameters", json!({"track": key})).unwrap();
        let params = list[0]["params"].as_array().unwrap();
        let get = |k: &str| {
            let param = format!("fx:{fx}:{k}");
            params
                .iter()
                .find(|p| p["param"] == param.as_str())
                .cloned()
                .unwrap()
        };
        // A slow speed reads as itself, not rounded to 0 Hz.
        assert_eq!(get("rate")["text"], "0.40 Hz");
        // Stages are a choice by index: the list says what each means.
        let stages = get("stages");
        assert_eq!(stages["value"], 1.0);
        let choices = stages["choices"].as_array().unwrap();
        assert_eq!(choices[1], stages["text"], "{stages}");
        assert!(get("rate").get("choices").is_none());
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
    fn a_change_wakes_a_waiting_assistant() {
        let mut d = Driver::new();
        let r0 = d.call("wait_for_change", json!({})).unwrap()["revision"]
            .as_u64()
            .unwrap();
        let r = d
            .call(
                "wait_for_change",
                json!({"revision": r0, "timeout_seconds": 0.2}),
            )
            .unwrap();
        assert_eq!(r["changed"], false);
        // A change by the person, while the assistant waits.
        let (reply, answer) = mpsc::channel();
        d.app()
            .agent
            .inbox
            .send(Request {
                tool: "wait_for_change".into(),
                args: json!({"revision": r0, "timeout_seconds": 20}),
                access: Access::Read,
                reply,
            })
            .unwrap();
        d.h.step();
        let ctx = d.h.ctx.clone();
        d.app().run(&ctx, Cmd::AddAudioTrack);
        let mut got = None;
        for _ in 0..200 {
            d.h.step();
            if let Ok(v) = answer.try_recv() {
                got = Some(v.unwrap());
                break;
            }
        }
        let got = got.expect("answered");
        assert_eq!(got["changed"], true);
        assert!(got["revision"].as_u64().unwrap() > r0);
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
