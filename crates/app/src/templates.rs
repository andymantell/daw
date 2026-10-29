// SPDX-License-Identifier: GPL-3.0-or-later
//! Project templates (roadmap 2.8): File → New from template… starts a
//! project from one that comes with the app ("Guitar lesson", "Band
//! demo") or one of the user's own, and File → Save as template… keeps
//! the project open now as one: its tracks, effects, sections and notes,
//! but not its audio or comments (see `daw_project::Project::as_template`).
//!
//! The user's templates are files in their profile, e.g.
//! `C:\Users\<name>\DAW\Templates\Guitar lesson.dawtemplate`.

use std::path::{Path, PathBuf};

use daw_project::{TEMPLATE_EXTENSION, Template, built_in_templates};
use eframe::egui::{self, RichText};

use crate::project::After;
use crate::{APP_NAME, App, user_dir};

/// Where the user's own templates are kept.
pub fn templates_dir() -> PathBuf {
    user_dir().join(APP_NAME).join("Templates")
}

pub(crate) struct Templates {
    /// The New from template window, with the templates it lists and
    /// which is picked.
    pub(crate) choosing: Option<Choosing>,
    /// The Save as template window, with the name so far.
    pub(crate) saving: Option<String>,
    /// Where the user's templates are; none in tests until they set one,
    /// as they must not touch the user's own.
    pub(crate) dir: Option<PathBuf>,
}

impl Default for Templates {
    fn default() -> Templates {
        Templates {
            choosing: None,
            saving: None,
            dir: (!cfg!(test)).then(templates_dir),
        }
    }
}

pub(crate) struct Choosing {
    /// Built-in templates first, then the user's, each with its file.
    list: Vec<(Template, Option<PathBuf>)>,
    picked: usize,
    /// A user's template whose Delete… was pressed, waiting for yes.
    deleting: Option<usize>,
    /// Templates that couldn't be read, and why.
    problems: Vec<String>,
}

impl Choosing {
    fn new(dir: Option<&Path>) -> Choosing {
        let mut list: Vec<(Template, Option<PathBuf>)> = built_in_templates()
            .into_iter()
            .map(|t| (t, None))
            .collect();
        let (mine, problems) = dir.map(read_templates).unwrap_or_default();
        list.extend(mine.into_iter().map(|(t, p)| (t, Some(p))));
        Choosing {
            list,
            picked: 0,
            deleting: None,
            problems,
        }
    }
}

/// The user's templates in `dir`, by name, and the files that couldn't
/// be read.
fn read_templates(dir: &Path) -> (Vec<(Template, PathBuf)>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (found, problems);
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some(TEMPLATE_EXTENSION) {
            continue;
        }
        let name = path
            .file_stem()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let read = std::fs::read(&path).map_err(|e| e.to_string());
        match read.and_then(|b| daw_project::template_from_bytes(&b)) {
            Ok(project) => {
                let about = describe(&project);
                found.push((
                    Template {
                        name,
                        about,
                        project,
                    },
                    path,
                ));
            }
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    found.sort_by_key(|(t, _)| t.name.to_lowercase());
    (found, problems)
}

/// One line on what a saved template holds.
fn describe(p: &daw_project::Project) -> String {
    let tracks = match p.tracks.len() {
        0 => "No tracks".to_string(),
        1 => "1 track".to_string(),
        n => format!("{n} tracks"),
    };
    let markers = match p.markers.len() {
        0 => String::new(),
        1 => ", 1 section".to_string(),
        n => format!(", {n} sections"),
    };
    let tempo = format!("{:.0}", p.tempo_bpm);
    format!("{tracks}{markers}, at {tempo} BPM.")
}

/// A name that can be a file's: not empty, none of / \ : * ? " < > |.
fn file_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        && !name.starts_with('.')
}

impl App {
    /// Opens the New from template window.
    pub(crate) fn choose_template(&mut self) {
        self.template_ui.choosing = Some(Choosing::new(self.template_ui.dir.as_deref()));
    }

    pub(crate) fn template_windows(&mut self, ctx: &egui::Context) {
        self.choose_template_window(ctx);
        self.save_template_window(ctx);
    }

    fn choose_template_window(&mut self, ctx: &egui::Context) {
        let Some(c) = &mut self.template_ui.choosing else {
            return;
        };
        let mut keep = true;
        let mut create = None;
        let mut delete = None;
        egui::Window::new("New from template")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        let mut heading = None;
                        for (i, (t, file)) in c.list.iter().enumerate() {
                            let group = if file.is_some() { "Yours" } else { "Included" };
                            if heading != Some(group) {
                                ui.label(RichText::new(group).strong());
                                heading = Some(group);
                            }
                            let r = ui.selectable_label(c.picked == i, &t.name);
                            if r.clicked() {
                                c.picked = i;
                            }
                            if r.double_clicked() {
                                create = Some(i);
                            }
                            ui.label(RichText::new(&t.about).weak());
                        }
                    });
                for p in &c.problems {
                    ui.label(RichText::new(format!("Couldn't read {p}")).weak());
                }
                ui.separator();
                if let Some(i) = c.deleting {
                    ui.label(format!(
                        "Delete the template “{}”? Projects made from it stay as they are.",
                        c.list[i].0.name
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            delete = Some(i);
                        }
                        if ui.button("Cancel").clicked() {
                            c.deleting = None;
                        }
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        create = Some(c.picked);
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    let mine = c.list.get(c.picked).is_some_and(|(_, f)| f.is_some());
                    if mine && ui.button("Delete…").clicked() {
                        c.deleting = Some(c.picked);
                    }
                });
            });
        if let Some(i) = delete {
            if let Some(path) = &c.list[i].1
                && let Err(e) = std::fs::remove_file(path)
            {
                self.message = Some(format!("Couldn't delete the template: {e}"));
            }
            *c = Choosing::new(self.template_ui.dir.as_deref());
        }
        if let Some(i) = create {
            let p = c.list[i].0.project.from_template(self.rate);
            self.template_ui.choosing = None;
            self.then(ctx, After::NewFrom(Box::new(p)));
            return;
        }
        if !keep {
            self.template_ui.choosing = None;
        }
    }

    fn save_template_window(&mut self, ctx: &egui::Context) {
        let Some(mut name) = self.template_ui.saving.take() else {
            return;
        };
        let mut keep = true;
        let mut go = false;
        let path = (self.template_ui.dir.as_ref())
            .map(|d| d.join(format!("{}.{TEMPLATE_EXTENSION}", name.trim())));
        let exists = file_name_ok(name.trim()) && path.as_ref().is_some_and(|p| p.exists());
        egui::Window::new("Save as template")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    "Keeps this project's tracks, effects, sections, tempo and notes to \
                     start new projects from. Its audio and comments aren't kept.",
                );
                ui.horizontal(|ui| {
                    let l = ui.label("Name");
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut name).hint_text("e.g. Tuesday lesson"))
                        .labelled_by(l.id);
                    if !r.has_focus() && name.is_empty() {
                        r.request_focus();
                    }
                    go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
                if exists {
                    ui.label(
                        RichText::new(
                            "There is already a template with this name: saving replaces it.",
                        )
                        .weak(),
                    );
                }
                ui.horizontal(|ui| {
                    go |= ui.button(if exists { "Replace" } else { "Save" }).clicked();
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
        if go {
            match self.save_template(ctx, name.trim()) {
                Ok(()) => {
                    keep = false;
                    self.message = Some(format!("Saved the template “{}”.", name.trim()));
                }
                Err(e) => self.message = Some(e),
            }
        }
        if keep {
            self.template_ui.saving = Some(name);
        }
    }

    /// Keeps the project open now as the template `name`.
    pub(crate) fn save_template(&mut self, ctx: &egui::Context, name: &str) -> Result<(), String> {
        if !file_name_ok(name) {
            return Err("Choose a name without / \\ : * ? \" < > |".into());
        }
        let Some(dir) = self.template_ui.dir.clone() else {
            return Err("There is nowhere to keep templates.".into());
        };
        self.sync_doc(ctx, true);
        self.write_plugin_states();
        let bytes = daw_project::template_to_bytes(&self.proj.doc.get());
        std::fs::create_dir_all(&dir)
            .and_then(|()| {
                // Written aside first, so a failed write leaves any old
                // one as it was.
                let path = dir.join(format!("{name}.{TEMPLATE_EXTENSION}"));
                let part = path.with_extension("part");
                std::fs::write(&part, bytes)?;
                std::fs::rename(&part, &path)
            })
            .map_err(|e| format!("Couldn't save the template: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::App;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-templates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn app(dir: &std::path::Path) -> Harness<'static, App> {
        let dir = dir.to_path_buf();
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(move |_cc| {
                let mut app = App::new(Vec::new());
                app.template_ui.dir = Some(dir.clone());
                app
            });
        h.run();
        h
    }

    fn open_chooser(h: &mut Harness<'_, App>) {
        h.state_mut().choose_template();
        h.run();
    }

    #[test]
    fn a_built_in_template_starts_a_project() {
        let dir = scratch("built-in");
        let mut h = app(&dir);
        open_chooser(&mut h);
        h.get_by_label("Guitar lesson").click();
        h.run();
        h.get_by_label("Create").click();
        h.run();
        let app = h.state();
        let names: Vec<_> = app.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Teacher", "Student", "Backing track"]);
        assert_eq!(app.markers.len(), 3);
        assert_eq!(app.tempo_bpm, 90.0);
        assert!(app.notes.contains("To practise"));
        assert!(app.template_ui.choosing.is_none());
        assert!(app.proj.folder.is_none());
        assert!(!app.can_undo(), "the template is where undo starts");
    }

    #[test]
    fn a_saved_template_is_listed_and_starts_a_project_without_audio() {
        let dir = scratch("saved");
        let mut h = app(&dir);
        {
            let app = h.state_mut();
            let mut t = crate::Track::new("Lead".into());
            t.mix.gain_db = -4.0;
            app.tracks.push(t);
            app.tempo_bpm = 72.0;
            app.markers.push(crate::timeline::Marker {
                id: daw_project::new_id(),
                bar: 3,
                name: "Riff".into(),
            });
        }
        h.run();
        h.state_mut().template_ui.saving = Some(String::new());
        h.run();
        h.get_by_label("Name").type_text("Tuesday lesson");
        h.run();
        h.get_by_label("Save").click();
        h.run();
        assert!(dir.join("Tuesday lesson.dawtemplate").exists());
        assert!(h.state().template_ui.saving.is_none());

        // Saving again under that name asks to replace it.
        h.state_mut().template_ui.saving = Some("Tuesday lesson".into());
        h.run();
        h.get_by_label("Replace").click();
        h.run();

        open_chooser(&mut h);
        h.get_by_label("Tuesday lesson").click();
        h.run();
        h.get_by_label("Create").click();
        h.run();
        let app = h.state();
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.tracks[0].name, "Lead");
        assert_eq!(app.tracks[0].mix.gain_db, -4.0);
        assert_eq!(app.tempo_bpm, 72.0);
        assert_eq!(app.markers[0].name, "Riff");

        // And it can be deleted, after asking.
        open_chooser(&mut h);
        h.get_by_label("Tuesday lesson").click();
        h.run();
        h.get_by_label("Delete…").click();
        h.run();
        assert!(dir.join("Tuesday lesson.dawtemplate").exists());
        h.get_by_label("Delete").click();
        h.run();
        assert!(!dir.join("Tuesday lesson.dawtemplate").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_that_cant_be_files_are_refused() {
        assert!(super::file_name_ok("Lesson 3"));
        for bad in ["", "a/b", "a:b", ".hidden", "what?"] {
            assert!(!super::file_name_ok(bad), "{bad}");
        }
    }
}
