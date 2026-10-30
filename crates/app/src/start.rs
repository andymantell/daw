// SPDX-License-Identifier: GPL-3.0-or-later
//! The start screen (roadmap 3.6): shown as the app starts with nothing
//! to open, it lists the projects opened or saved most recently and the
//! templates to start from, so the usual first click is one click. File
//! → Start screen… shows it again, and a box on it turns it off at start.
//!
//! The recent list and that choice are kept per computer in `start.txt`
//! beside the other settings: one `show yes|no` line, then a `recent`
//! line per project folder, newest first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use daw_project::{DOC_FILE, Template};
use eframe::egui::{self, RichText};

use crate::App;
use crate::project::{After, ago, find_projects_in, projects_dir};
use crate::templates::all_templates;

/// How many projects the list keeps, and shows.
const RECENT: usize = 8;

/// The settings kept in `start.txt`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// Show the start screen when the app starts with nothing to open.
    pub(crate) show: bool,
    /// Project folders, most recently opened or saved first.
    pub(crate) recent: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            show: true,
            recent: Vec::new(),
        }
    }
}

impl Settings {
    pub(crate) fn from_text(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            let value = value.trim();
            match key {
                "show" => s.show = value != "no",
                "recent" if !value.is_empty() && s.recent.len() < RECENT => {
                    s.recent.push(PathBuf::from(value));
                }
                _ => {}
            }
        }
        s
    }

    pub(crate) fn to_text(&self) -> String {
        let mut t = format!("show {}\n", if self.show { "yes" } else { "no" });
        for p in &self.recent {
            t.push_str(&format!("recent {}\n", p.display()));
        }
        t
    }

    /// Puts `dir` at the top of the list, once.
    pub(crate) fn opened(&mut self, dir: &Path) {
        self.recent.retain(|p| p != dir);
        self.recent.insert(0, dir.to_path_buf());
        self.recent.truncate(RECENT);
    }
}

/// A project on the list.
pub(crate) struct Recent {
    pub(crate) dir: PathBuf,
    pub(crate) name: String,
    /// How long ago it was last saved.
    pub(crate) saved: Option<Duration>,
}

/// What the start screen shows, read from disk as it opens.
pub(crate) struct Showing {
    pub(crate) recent: Vec<Recent>,
    templates: Vec<Template>,
    problems: Vec<String>,
}

pub(crate) struct Start {
    pub(crate) settings: Settings,
    /// Where the settings are kept; none in tests, which must not touch
    /// the user's own.
    file: Option<PathBuf>,
    /// Where projects are looked for when the recent list is short;
    /// none in tests until they set one.
    pub(crate) projects: Option<PathBuf>,
    /// The start screen, while it is open.
    pub(crate) showing: Option<Showing>,
}

impl Start {
    pub(crate) fn new(file: Option<PathBuf>) -> Start {
        let settings = (file.as_deref())
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map_or_else(Settings::default, |t| Settings::from_text(&t));
        Start {
            settings,
            projects: file.is_some().then(projects_dir),
            file,
            showing: None,
        }
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, self.settings.to_text()).map_err(|e| e.to_string())
    }

    /// The recent projects that are still there, then others in the
    /// projects folder, newest first, up to the list's length.
    pub(crate) fn recent(&self) -> Vec<Recent> {
        let mut dirs: Vec<PathBuf> = (self.settings.recent.iter())
            .filter(|d| d.join(DOC_FILE).is_file())
            .cloned()
            .collect();
        for d in self
            .projects
            .as_deref()
            .map(find_projects_in)
            .unwrap_or_default()
        {
            if dirs.len() >= RECENT {
                break;
            }
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        dirs.into_iter()
            .map(|dir| Recent {
                name: dir
                    .file_name()
                    .map_or("Project".into(), |n| n.to_string_lossy().into_owned()),
                saved: std::fs::metadata(dir.join(DOC_FILE))
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok()),
                dir,
            })
            .collect()
    }
}

impl App {
    /// Opens the start screen, reading what it lists afresh.
    pub(crate) fn show_start(&mut self) {
        let (templates, problems) = all_templates(self.template_ui.dir.as_deref());
        self.start.showing = Some(Showing {
            recent: self.start.recent(),
            templates: templates.into_iter().map(|(t, _)| t).collect(),
            problems,
        });
    }

    /// Puts a project just opened or saved at the top of the list.
    pub(crate) fn remember_recent(&mut self, dir: &Path) {
        self.start.settings.opened(dir);
        if let Err(e) = self.start.save() {
            self.message = Some(format!("Couldn't keep the recent projects: {e}"));
        }
    }

    pub(crate) fn start_window(&mut self, ctx: &egui::Context) {
        // Out of the way while the user answers something else first.
        if self.proj.found.is_some() || self.proj.dialog.is_some() {
            return;
        }
        // Something was opened another way (a file dropped on the window).
        if self.start.showing.is_some() && (!self.tracks.is_empty() || self.proj.loading()) {
            self.start.showing = None;
        }
        let Some(s) = &self.start.showing else {
            return;
        };
        let mut then = None;
        let mut close = false;
        let mut show = self.start.settings.show;
        egui::Window::new("Start")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(300.0);
                        ui.label(RichText::new("Recent projects").strong());
                        if s.recent.is_empty() {
                            ui.label(RichText::new("Projects you open or save show here.").weak());
                        }
                        for r in &s.recent {
                            let b = ui.selectable_label(false, RichText::new(&r.name).strong());
                            let when = r
                                .saved
                                .map_or(String::new(), |d| format!("Saved {}", ago(d)));
                            let b = b.on_hover_text(r.dir.display().to_string());
                            ui.label(RichText::new(when).weak().small());
                            if b.clicked() {
                                then = Some(After::Open(r.dir.clone()));
                            }
                        }
                        ui.add_space(4.0);
                        if ui.button("Open another…").clicked()
                            && let Some(dir) = rfd::FileDialog::new()
                                .set_title("Open project folder")
                                .set_directory(projects_dir())
                                .pick_folder()
                        {
                            then = Some(After::Open(dir));
                        }
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(260.0);
                        ui.label(RichText::new("Start something new").strong());
                        for t in &s.templates {
                            if ui.selectable_label(false, &t.name).clicked() {
                                let p = t.project.from_template(self.rate);
                                then = Some(After::NewFrom(Box::new(p)));
                            }
                            ui.label(RichText::new(&t.about).weak().small());
                        }
                        for p in &s.problems {
                            ui.label(RichText::new(format!("Couldn't read {p}")).weak());
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut show, "Show this when the app starts");
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if show != self.start.settings.show {
            self.start.settings.show = show;
            if let Err(e) = self.start.save() {
                self.message = Some(format!("Couldn't keep that choice: {e}"));
            }
        }
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.start.showing = None;
        }
        if let Some(t) = then {
            self.start.showing = None;
            self.then(ctx, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use super::Settings;
    use crate::App;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-start-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app(dir: &Path) -> Harness<'static, App> {
        let dir = dir.to_path_buf();
        Harness::builder()
            .with_size([1100.0, 700.0])
            .allow_missing_glyphs()
            .build_eframe(move |_cc| {
                let mut app = App::new(Vec::new());
                app.start.projects = Some(dir.join("Projects"));
                app.template_ui.dir = Some(dir.join("Templates"));
                app
            })
    }

    /// A saved project called `name` in the test's projects folder.
    fn saved(h: &mut Harness<'_, App>, dir: &Path, name: &str) -> PathBuf {
        let at = dir.join("Projects").join(name);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &at).unwrap();
        at
    }

    #[test]
    fn settings_are_written_and_read_back() {
        let mut s = Settings::default();
        assert!(s.show);
        s.show = false;
        s.opened(Path::new("/a"));
        s.opened(Path::new("/b"));
        s.opened(Path::new("/a"));
        assert_eq!(s.recent, [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(Settings::from_text(&s.to_text()), s);
        for i in 0..20 {
            s.opened(&PathBuf::from(format!("/p{i}")));
        }
        assert_eq!(s.recent.len(), super::RECENT);
        assert_eq!(s.recent[0], PathBuf::from("/p19"));
        // Anything else in the file is left alone.
        assert_eq!(Settings::from_text("odd line\n"), Settings::default());
    }

    #[test]
    fn it_lists_projects_and_opens_one() {
        let dir = scratch("open");
        let mut h = app(&dir);
        h.run();
        let song = saved(&mut h, &dir, "Song for Tuesday");
        h.state_mut().new_project();
        h.state_mut().show_start();
        h.run();
        h.get_by_label("Song for Tuesday").click();
        h.run();
        assert!(h.state().start.showing.is_none());
        for _ in 0..200 {
            h.run();
            if !h.state().proj.loading() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let folder = h.state().proj.folder.as_ref().map(|f| f.dir.clone());
        assert_eq!(folder, Some(song));
    }

    #[test]
    fn it_starts_from_a_template() {
        let dir = scratch("template");
        let mut h = app(&dir);
        h.state_mut().show_start();
        h.run();
        let n = h.get_by_label("Guitar lesson");
        n.click();
        h.run();
        h.run();
        assert!(h.state().start.showing.is_none());
        let names: Vec<_> = h.state().tracks.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, ["Teacher", "Student", "Backing track"]);
    }

    #[test]
    fn it_closes_and_stays_off_when_asked() {
        let dir = scratch("off");
        let file = dir.join("start.txt");
        let mut h = Harness::builder()
            .with_size([1100.0, 700.0])
            .allow_missing_glyphs()
            .build_eframe({
                let file = file.clone();
                move |_cc| {
                    let mut app = App::new(Vec::new());
                    app.start = super::Start::new(Some(file));
                    app.start.projects = None;
                    app
                }
            });
        h.state_mut().show_start();
        h.run();
        h.get_by_label("Show this when the app starts").click();
        h.run();
        h.get_by_label("Close").click();
        h.run();
        assert!(h.state().start.showing.is_none());
        let kept = Settings::from_text(&std::fs::read_to_string(&file).unwrap());
        assert!(!kept.show);
    }
}
