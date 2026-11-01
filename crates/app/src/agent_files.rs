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
use crate::project::After;

type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
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
                      export queue. Follow it with get_exports.",
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
    ) -> Option<Done> {
        Some(match name {
            "save_project" => self.save_project_tool(ctx, args),
            "open_project" => self.leave_for(ctx, args, true),
            "new_project" => self.leave_for(ctx, args, false),
            "export_mix" => self.export_mix_tool(args),
            _ => return None,
        })
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
        Ok((json!({"started": !queued, "queued": queued}), None))
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
        let r = d
            .call(
                "export_mix",
                json!({"format": "wav", "from_bar": 1, "to_bar": 2, "folder": dir}),
            )
            .unwrap();
        assert_eq!(r["started"], true);
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
