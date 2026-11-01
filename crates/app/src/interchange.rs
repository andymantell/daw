// SPDX-License-Identifier: GPL-3.0-or-later
//! Swapping projects with other DAWs (roadmap Phase 4): File → Export
//! for other DAWs writes the open exchange format, after showing what
//! won't go; File → Import from another DAW opens one of its files as a
//! new project and shows what didn't come in. The work is in the
//! `daw-interchange` crate; this is the app's side of it.

use std::path::{Path, PathBuf};

use daw_interchange::{ExportOptions, ImportOptions, Report};
use eframe::egui::{self, RichText};

use crate::project::After;
use crate::{APP_NAME, App, user_dir};

/// What the windows show.
#[derive(Default)]
pub(crate) struct Swap {
    /// Before an export: what won't go.
    pub(crate) asking: Option<Report>,
    /// After an import or export: what happened, and what didn't go.
    pub(crate) done: Option<(String, Report)>,
}

pub(crate) fn is_dawproject(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(daw_interchange::EXTENSION))
}

impl App {
    /// The project with each plugin's settings read in now.
    fn project_with_plugin_states(&mut self) -> daw_project::Project {
        self.write_plugin_states();
        let mut p = self.proj.doc.get();
        p.rate = self.rate;
        p
    }

    fn song_name(&self) -> String {
        self.proj
            .folder
            .as_ref()
            .map_or("Song".to_string(), |f| f.name())
    }

    /// What an export would leave out.
    pub(crate) fn interchange_report(&mut self) -> Report {
        let p = self.project_with_plugin_states();
        let title = self.song_name();
        let paths = self.asset_paths();
        let path = |a: &str| paths(a);
        daw_interchange::export_report(&p, &self.export_options(&title, &path))
    }

    fn export_options<'a>(
        &self,
        title: &'a str,
        asset_path: &'a dyn Fn(&str) -> PathBuf,
    ) -> ExportOptions<'a> {
        ExportOptions {
            title,
            app_version: env!("CARGO_PKG_VERSION"),
            asset_path,
            clap_id: &clap_id,
        }
    }

    /// Where each asset's file is, as a function the export can hold.
    fn asset_paths(&self) -> impl Fn(&str) -> PathBuf + use<> {
        let folder = self.proj.folder.clone();
        let moved = self.proj.moved.clone();
        move |a: &str| {
            let a = moved.get(a).map_or(a, String::as_str);
            match &folder {
                Some(f) if f.has_asset(a) => f.asset(a),
                _ => PathBuf::from(a),
            }
        }
    }

    /// File → Export for other DAWs: shows what won't go first.
    pub(crate) fn ask_export_dawproject(&mut self) {
        self.swap.asking = Some(self.interchange_report());
    }

    /// Asks where, then writes the file.
    fn pick_export_dawproject(&mut self) {
        let Some(dest) = rfd::FileDialog::new()
            .set_title("Export for other DAWs")
            .set_directory(user_dir())
            .set_file_name(format!(
                "{}.{}",
                self.song_name(),
                daw_interchange::EXTENSION
            ))
            .add_filter("DAW project", &[daw_interchange::EXTENSION])
            .save_file()
        else {
            return;
        };
        let dest = if is_dawproject(&dest) {
            dest
        } else {
            PathBuf::from(format!("{}.{}", dest.display(), daw_interchange::EXTENSION))
        };
        match self.write_dawproject(&dest) {
            Ok(r) => self.swap.done = Some((format!("Exported to {}.", dest.display()), r)),
            Err(e) => self.message = Some(e),
        }
    }

    /// Writes the project to `dest`; what didn't go.
    pub(crate) fn write_dawproject(&mut self, dest: &Path) -> Result<Report, String> {
        let p = self.project_with_plugin_states();
        let title = self.song_name();
        let paths = self.asset_paths();
        let path = |a: &str| paths(a);
        let (bytes, report) = daw_interchange::export(&p, &self.export_options(&title, &path))?;
        std::fs::write(dest, bytes)
            .map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
        self.message = Some(format!("Exported to {}.", dest.display()));
        Ok(report)
    }

    /// File → Import from another DAW.
    pub(crate) fn pick_import_dawproject(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import from another DAW")
            .set_directory(user_dir())
            .add_filter("DAW project", &[daw_interchange::EXTENSION])
            .pick_file()
        else {
            return;
        };
        if let Err(e) = self.import_dawproject(ctx, &path) {
            self.message = Some(e);
        }
    }

    /// Opens `path` as a new, unsaved project (asking about unsaved
    /// work first); what didn't come in.
    pub(crate) fn import_dawproject(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
    ) -> Result<Report, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let stem = path
            .file_stem()
            .map_or("Imported".into(), |s| s.to_string_lossy().into_owned());
        // Its audio is kept here until the project is saved, which
        // copies it into the project's folder.
        let audio_dir = user_dir()
            .join(APP_NAME)
            .join("Imported")
            .join(format!("{stem} {}", daw_project::new_id()));
        let list = self.effects.list.plugins();
        let has_plugin = |e: &daw_project::Effect| {
            list.iter().any(|a| {
                a.class
                    .cid
                    .iter()
                    .map(|&b| b as u8)
                    .eq(e.class.iter().copied())
            })
        };
        let imported = daw_interchange::import(
            &bytes,
            &ImportOptions {
                rate: self.rate,
                audio_dir,
                base_dir: path.parent().map(Path::to_path_buf),
                has_plugin: &has_plugin,
            },
        )
        .map_err(|e| format!("Could not import {}: {e}", path.display()))?;
        let from = if imported.application.is_empty() {
            String::new()
        } else {
            format!(" (made in {})", imported.application)
        };
        let report = imported.report.clone();
        self.swap.done = Some((
            format!("Imported {}{from}. Save it to keep it.", path.display()),
            report.clone(),
        ));
        self.then(ctx, After::NewFrom(Box::new(imported.project)));
        Ok(report)
    }

    /// The windows: what won't go before an export, and what happened
    /// after.
    pub(crate) fn interchange_windows(&mut self, ctx: &egui::Context) {
        if let Some(r) = &self.swap.asking {
            let mut go = false;
            let mut cancel = false;
            egui::Window::new("Export for other DAWs")
                .collapsible(false)
                .resizable(true)
                .default_width(460.0)
                .show(ctx, |ui| {
                    ui.label(
                        "Writes a .dawproject file, which several other DAWs can open: \
                         tracks, routing, clips, notes, automation, tempo, markers and \
                         plugins with their settings.",
                    );
                    ui.add_space(6.0);
                    if r.is_empty() {
                        ui.label("Everything in this project goes.");
                    } else {
                        ui.label(RichText::new("What won't go").strong());
                        report_list(ui, r);
                        ui.label(
                            RichText::new(
                                "To keep how a track sounds in another DAW, render it to \
                                 audio first, from the track's menu.",
                            )
                            .weak(),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        go = ui.button("Export…").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if cancel {
                self.swap.asking = None;
            }
            if go {
                self.swap.asking = None;
                self.pick_export_dawproject();
            }
        }
        if let Some((what, r)) = &self.swap.done {
            let mut close = false;
            egui::Window::new("Other DAWs")
                .collapsible(false)
                .resizable(true)
                .default_width(460.0)
                .show(ctx, |ui| {
                    ui.label(what);
                    if !r.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("What didn't go").strong());
                        report_list(ui, r);
                    }
                    close = ui.button("Close").clicked();
                });
            if close {
                self.swap.done = None;
            }
        }
    }
}

fn report_list(ui: &mut egui::Ui, r: &Report) {
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            for i in &r.items {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&i.what).strong());
                    let mut d = i.detail.clone();
                    if i.count > 1 {
                        d.push_str(&format!(" (×{})", i.count));
                    }
                    ui.label(d);
                });
            }
        });
}

/// A CLAP plugin's own id, read from its bundle (as scanning does).
fn clap_id(e: &daw_project::Effect) -> Option<String> {
    let mut cid: vst3::Steinberg::TUID = Default::default();
    if e.class.len() != cid.len() {
        return None;
    }
    for (c, &b) in cid.iter_mut().zip(&e.class) {
        *c = b as _;
    }
    let m = daw_plugins::clap::ClapModule::load(Path::new(&e.bundle)).ok()?;
    m.id_of(&cid)
}

/// The report as JSON, for the agent interface.
pub(crate) fn report_value(r: &Report) -> serde_json::Value {
    serde_json::Value::Array(
        (r.items.iter())
            .map(|i| serde_json::json!({"what": i.what, "detail": i.detail, "count": i.count}))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    #[test]
    fn a_project_goes_out_and_back_through_the_agent_interface() {
        let dir = std::env::temp_dir().join(format!("daw-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("song.dawproject");
        let mut d = Driver::new();
        d.command("add-instrument-track");
        let key = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        d.call(
            "set_parameter",
            json!({"track": key, "param": "gain", "value": -6.0}),
        )
        .unwrap();
        // What won't go is there to read before anything is written.
        let early = d.call("export_report", json!({})).unwrap();
        assert!(early.is_array());
        let out = d
            .call(
                "export_project_file",
                json!({"path": file.to_str().unwrap()}),
            )
            .unwrap();
        assert_eq!(out["left_out"], early);
        assert!(file.is_file());
        // Unsaved work isn't thrown away for an import.
        let refused = d.call(
            "import_project_file",
            json!({"path": file.to_str().unwrap()}),
        );
        assert!(refused.unwrap_err().contains("unsaved work"));
        d.app().proj.dirty = false;
        d.call(
            "import_project_file",
            json!({"path": file.to_str().unwrap()}),
        )
        .unwrap();
        let p = d.project();
        let tracks = p["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["kind"], "instrument");
        assert_eq!(tracks[0]["name"], "Instrument 1");
        assert!((tracks[0]["gain_db"].as_f64().unwrap() + 6.0).abs() < 1e-9);
        assert_ne!(tracks[0]["id"], key.as_str(), "a new project, with new ids");
        assert!(tracks[0]["instrument"]["builtin"].is_string());
        // Both commands are in the command table, for people.
        let list = d.call("list_commands", json!({})).unwrap();
        let ids: Vec<&str> = (list.as_array().unwrap().iter())
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"export-for-other-daws"));
        assert!(ids.contains(&"import-from-other-daw"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
