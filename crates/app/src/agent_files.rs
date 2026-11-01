// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for the commands that open a window to ask for a
//! name or settings (roadmap 2.11): saving, opening, a new project from
//! a template, and exporting the mix. Each takes its settings instead.
//!
//! None of them throws away unsaved work: an unsaved project with tracks
//! has to be saved first.

use std::path::{Path, PathBuf};

use daw_engine::export::Format;
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::commands::Cmd;
use crate::notation::NotationFile;
use crate::project::After;

type Done = Result<(Value, Option<String>), String>;

/// The commands that ask for a file in a file chooser, and whether
/// they write one (needing the edit and export permission). Assistants
/// run them with `file_command`, giving the file.
pub(crate) const FILE_COMMANDS: &[(Cmd, bool)] = &[
    (Cmd::ImportPreset, false),
    (Cmd::ImportBundle, false),
    (Cmd::ExportBundle, true),
    (Cmd::ImportMidi, false),
    (Cmd::ExportMidi, true),
    (Cmd::ImportMusicXml, false),
    (Cmd::ExportMusicXml, true),
    (Cmd::ExportTextTab, true),
    (Cmd::PrintNotation, true),
    (Cmd::ImportKit, false),
    (Cmd::ExportKit, true),
    (Cmd::LoadSfz, false),
];

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "file_command",
        description: "Runs a command that would ask for a file in a file chooser, with the \
                      file given instead: import-preset, import-bundle, export-bundle, \
                      import-midi, export-midi, import-musicxml, export-musicxml, \
                      export-text-tab, print-notation (a PDF), import-kit, export-kit or \
                      load-sfz (an SFZ sample library, on the sampler track given, else the \
                      first sampler track, else a new one; a big library takes a few \
                      seconds). Those that write a file need the edit and export permission.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "id": {"type": "string", "description": "The command's id."},
                "path": {"type": "string", "description": "The file's full path."},
                "track": {"type": "string", "description": "For load-sfz: the sampler track to load it on."}
            }, "required": ["id", "path"]})
        },
    },
    Tool {
        name: "list_projects",
        description: "The projects in the projects folder, and the templates a new project \
                      can start from (those that come with the app, then the user's).",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "save_project",
        description: "Saves the project. One not saved before needs a name, and is saved \
                      as a new folder of that name in the projects folder.",
        access: Access::EditExport,
        schema: || {
            json!({"type": "object", "properties": {
                "name": {"type": "string"}
            }})
        },
    },
    Tool {
        name: "open_project",
        description: "Opens a project, by its folder's name in the projects folder or a \
                      full path. A saved project is saved first; an unsaved one with tracks \
                      is refused, so nothing is lost. It opens in the background.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "project": {"type": "string"}
            }, "required": ["project"]})
        },
    },
    Tool {
        name: "new_project",
        description: "Starts a new, empty project, or one from a template by name \
                      (list_projects gives them). As open_project, it won't lose unsaved \
                      work.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "template": {"type": "string"}
            }})
        },
    },
    Tool {
        name: "export_mix",
        description: "Exports the mix, and stems if asked, into a new numbered folder: the \
                      whole song, the loop, or bars given. Starts now, or waits in the \
                      export queue. Answers with the revision now: wait_for_change with \
                      it answers when the export (or anything else) finishes, then \
                      get_exports says where the files are.",
        access: Access::EditExport,
        schema: || {
            json!({"type": "object", "properties": {
                "format": {"type": "string", "enum": ["wav", "flac", "mp3", "opus"],
                           "description": "As the Export window is set if left out."},
                "from_bar": {"type": "integer"},
                "to_bar": {"type": "integer", "description": "The bar it stops at the start of."},
                "loop": {"type": "boolean", "description": "Only the loop."},
                "stems": {"type": "boolean", "description": "Each track on its own too."},
                "bus_stems": {"type": "boolean", "description": "Each bus as it sounds in the mix too."},
                "tracks": {"type": "array", "items": {"type": "string"},
                           "description": "Only these tracks' stems, by id."},
                "play_along": {"type": "boolean",
                               "description": "Leave out the tracks muted for practice."},
                "folder": {"type": "string",
                           "description": "A full path to make the numbered folder in; the project's Exports folder if left out."}
            }})
        },
    },
];

fn format_arg(args: &Value) -> Result<Option<Format>, String> {
    Ok(match args["format"].as_str() {
        None => None,
        Some("wav") => Some(Format::Wav),
        Some("flac") => Some(Format::Flac),
        Some("mp3") => Some(Format::Mp3(320)),
        Some("opus") => Some(Format::Opus(160)),
        Some(f) => return Err(format!("There is no file type {f}.")),
    })
}

fn project_path(name: &str) -> PathBuf {
    match Path::new(name) {
        p if p.is_absolute() => p.to_path_buf(),
        _ => crate::project::projects_dir().join(name),
    }
}

impl App {
    /// Runs one of the tools here, or `None` if `name` isn't one.
    pub(crate) fn files_tool(
        &mut self,
        ctx: &eframe::egui::Context,
        name: &str,
        args: &Value,
        access: Access,
    ) -> Option<Done> {
        Some(match name {
            "file_command" => self.file_command_tool(ctx, args, access),
            "save_project" => self.save_project_tool(ctx, args),
            "open_project" => self.leave_for(ctx, args, true),
            "new_project" => self.leave_for(ctx, args, false),
            "export_mix" => self.export_mix_tool(args),
            _ => return None,
        })
    }

    fn file_command_tool(
        &mut self,
        ctx: &eframe::egui::Context,
        args: &Value,
        access: Access,
    ) -> Done {
        let id = args["id"].as_str().ok_or("Give the command's id.")?;
        let path = PathBuf::from(args["path"].as_str().ok_or("Give the file's full path.")?);
        let &(cmd, writes) = (FILE_COMMANDS.iter())
            .find(|(c, _)| Some(*c) == Cmd::from_id(id))
            .ok_or_else(|| format!("{id} isn't a command that asks for a file."))?;
        if writes && access < Access::EditExport {
            return Err(format!(
                "{id} writes a file, which needs the edit and export permission."
            ));
        }
        if !writes && !path.is_file() {
            return Err(format!("There is no file {}.", path.display()));
        }
        self.message = None;
        match cmd {
            Cmd::ImportPreset => self.import_dropped_preset(&path),
            Cmd::ImportBundle => self.import_bundle(path.clone()),
            Cmd::ExportBundle => {
                if self.proj.folder.is_none() {
                    return Err("Save the project first, then export it as a bundle.".into());
                }
                self.save(ctx)?;
                self.write_bundle(path.clone());
            }
            Cmd::ImportMidi => self.import_midi(&path),
            Cmd::ExportMidi => self.write_midi(&path),
            Cmd::ImportMusicXml => self.import_musicxml_file(&path),
            Cmd::ExportMusicXml | Cmd::ExportTextTab | Cmd::PrintNotation => {
                let score = self.notation_score();
                if score.parts.is_empty() {
                    return Err("Show a notation lane under a track first.".into());
                }
                let what = match cmd {
                    Cmd::ExportMusicXml => NotationFile::MusicXml,
                    Cmd::ExportTextTab => NotationFile::TextTab,
                    _ => NotationFile::Pdf,
                };
                self.write_notation(what, &score, &path);
            }
            Cmd::ImportKit => self.import_kit(&path),
            Cmd::LoadSfz => {
                if let Some(key) = args["track"].as_str() {
                    let i = self
                        .track_index(key)
                        .ok_or_else(|| format!("There is no track {key}."))?;
                    self.instrument_open = Some(i);
                }
                let m = self.load_sfz(&path)?;
                self.message = Some(m);
            }
            Cmd::ExportKit => self.export_kit(&path),
            _ => unreachable!("only the commands listed"),
        }
        let name = self.commands.name(cmd).to_string();
        Ok((
            json!({"ran": id, "path": path.display().to_string(), "message": self.message.take()}),
            (!writes).then_some(name),
        ))
    }

    pub(crate) fn projects_value(&self) -> Value {
        let projects: Vec<Value> = (crate::project::find_projects().into_iter())
            .map(|p| {
                json!({"name": p.file_name().map(|n| n.to_string_lossy().into_owned()),
                       "path": p.display().to_string()})
            })
            .collect();
        let (templates, _) = crate::templates::all_templates(self.template_ui.dir.as_deref());
        let templates: Vec<Value> = (templates.iter())
            .map(|(t, file)| json!({"name": t.name, "about": t.about, "mine": file.is_some()}))
            .collect();
        json!({
            "open": self.proj.folder.as_ref().map(|f| f.name()),
            "unsaved_changes": self.proj.dirty,
            "projects": projects,
            "templates": templates,
        })
    }

    fn save_project_tool(&mut self, ctx: &eframe::egui::Context, args: &Value) -> Done {
        self.sync_doc(ctx, true);
        if self.proj.folder.is_some() {
            self.save(ctx)?;
        } else {
            let name = args["name"]
                .as_str()
                .ok_or("The project hasn't been saved yet: give it a name.")?;
            self.save_as(ctx, name)?;
        }
        let f = self.proj.folder.as_ref().expect("just saved");
        Ok((
            json!({"saved": f.name(), "folder": f.dir.display().to_string()}),
            None,
        ))
    }

    /// Opens a project (`open`) or starts a new one, keeping any work.
    fn leave_for(&mut self, ctx: &eframe::egui::Context, args: &Value, open: bool) -> Done {
        let then = if open {
            let name = args["project"]
                .as_str()
                .ok_or("Give the project's name or folder.")?;
            let path = project_path(name);
            if !crate::project::is_project(&path) {
                return Err(format!("There is no project at {}.", path.display()));
            }
            After::Open(path)
        } else {
            match args["template"].as_str() {
                None => After::New,
                Some(name) => {
                    let (list, _) =
                        crate::templates::all_templates(self.template_ui.dir.as_deref());
                    let t = (list.into_iter())
                        .find(|(t, _)| t.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| format!("There is no template {name}."))?;
                    After::NewFrom(Box::new(t.0.project.from_template(self.rate)))
                }
            }
        };
        self.sync_doc(ctx, true);
        if self.proj.folder.is_none() && self.proj.dirty && !self.tracks.is_empty() {
            return Err(
                "This project has unsaved work: save it with save_project first, so it isn't \
                 lost."
                    .into(),
            );
        }
        self.message = None;
        self.then(ctx, then);
        if let Some(e) = self.message.take() {
            return Err(e);
        }
        Ok((json!({"started": true}), None))
    }

    fn export_mix_tool(&mut self, args: &Value) -> Done {
        let format = format_arg(args)?;
        let mut req = self.export.mix_request();
        if let Some(f) = format {
            req.format = f;
        }
        if let Some(dir) = args["folder"].as_str() {
            req.folder = dir.into();
        }
        req.loop_only = args["loop"].as_bool() == Some(true);
        req.bars = match (args["from_bar"].as_i64(), args["to_bar"].as_i64()) {
            (Some(a), Some(b)) if b > a && a >= 1 => Some((a, b)),
            (None, None) => None,
            _ => return Err("Give both bars, the second after the first.".into()),
        };
        req.stems = args["stems"].as_bool() == Some(true) || args["tracks"].is_array();
        req.bus_stems = args["bus_stems"].as_bool() == Some(true);
        req.play_along = args["play_along"].as_bool() == Some(true);
        if let Some(list) = args["tracks"].as_array() {
            let mut only = Vec::new();
            for k in list {
                let k = k.as_str().ok_or("Track ids are text.")?;
                if self.track_index(k).is_none() {
                    return Err(format!("There is no track {k}."));
                }
                only.push(k.to_string());
            }
            req.only = Some(only);
        }
        let queued = self.export.busy() || self.export.queue().next().is_some();
        self.export_or_queue(req)?;
        // wait_for_change with this revision answers when it is done.
        let revision = self.agent.revision;
        Ok((
            json!({"started": !queued, "queued": queued, "revision": revision}),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    #[test]
    fn templates_are_listed_and_a_new_project_comes_from_one() {
        let mut d = Driver::new();
        let list = d.call("list_projects", json!({})).unwrap();
        let first = list["templates"][0]["name"].as_str().unwrap().to_string();
        d.call("new_project", json!({"template": first})).unwrap();
        assert!(
            d.call("new_project", json!({"template": "no such"}))
                .is_err()
        );
        assert!(
            d.call("open_project", json!({"project": "/no/such/project"}))
                .is_err()
        );
    }

    #[test]
    fn unsaved_work_is_never_thrown_away() {
        let mut d = Driver::new();
        d.access = crate::agent::Access::Edit;
        d.call("add_track", json!({"kind": "audio"})).unwrap();
        let e = d.call("new_project", json!({})).unwrap_err();
        assert!(e.contains("unsaved"), "{e}");
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), 1);
        // Saving needs a name the first time, and the permission.
        let e = d.call("save_project", json!({})).unwrap_err();
        assert!(e.contains("permission"), "{e}");
    }

    #[test]
    fn an_export_is_asked_for_with_its_settings() {
        let mut d = Driver::new();
        d.access = crate::agent::Access::EditExport;
        d.call("add_track", json!({"kind": "instrument"})).unwrap();
        let dir = std::env::temp_dir().join(format!("daw-export-mix-{}", std::process::id()));
        assert!(
            d.call("export_mix", json!({"from_bar": 3, "to_bar": 2}))
                .is_err()
        );
        assert!(d.call("export_mix", json!({"format": "aiff"})).is_err());
        // Waiting for a change, from the revision export_mix gives,
        // hears the export finish.
        let r = d
            .call(
                "export_mix",
                json!({"format": "wav", "from_bar": 1, "to_bar": 2, "folder": dir}),
            )
            .unwrap();
        assert_eq!(r["started"], true);
        let woke = d
            .call(
                "wait_for_change",
                json!({"revision": r["revision"], "timeout_seconds": 30}),
            )
            .unwrap();
        assert_eq!(woke["changed"], true, "{woke}");
        let mut done = serde_json::Value::Null;
        for _ in 0..500 {
            done = d.call("get_exports", json!({})).unwrap();
            if !done["finished"].as_array().is_none_or(|f| f.is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let files = &done["finished"][0]["files"];
        assert!(files.as_array().is_some_and(|f| !f.is_empty()), "{done}");
        assert!(files[0].get("error").is_none(), "{done}");
        assert!(dir.exists(), "{done}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
