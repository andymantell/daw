// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects in the app (roadmap 1.6 and 1.8): the project document kept
//! in step with what is on screen, undo and redo through it, saving into
//! a project folder (takes and imported audio move in), opening, and
//! autosave.
//!
//! Edits are made to the app's own model as before. Each frame, once no
//! button is held and no text is being typed, the model is compared with
//! what was last written to the document, and any difference is written
//! as one undoable edit. A drag or a burst of typing is one step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daw_collab::watch::{Change, FolderWatch};
use daw_engine::engine::Command;
use daw_engine::graph::Mix;
use daw_engine::import;
use daw_engine::stream::StreamSource;
use daw_engine::time::Frames;
use daw_project::{DOC_FILE, Practice, Project, ProjectDoc, ProjectFolder};
use eframe::egui::{self, RichText, ViewportCommand};

use crate::timeline::{Clip, Marker};
use crate::{APP_NAME, App, Track, cache_dir, takes_dir, user_dir};

/// How often a saved project is saved again while it changes.
const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);

/// Where new projects go: e.g. `C:\Users\<name>\DAW\Projects`.
pub fn projects_dir() -> PathBuf {
    user_dir().join(APP_NAME).join("Projects")
}

/// What to do once unsaved changes are dealt with.
#[derive(Clone)]
pub enum After {
    New,
    Open(PathBuf),
    Quit,
}

pub enum Dialog {
    SaveAs { name: String, then: Option<After> },
    Open { found: Vec<PathBuf> },
    Unsaved { then: After },
}

/// A project opened in the background.
pub struct Loaded {
    folder: ProjectFolder,
    doc: ProjectDoc,
    sources: HashMap<String, Arc<StreamSource>>,
    /// Assets that couldn't be opened, and why.
    failed: Vec<String>,
}

pub struct Proj {
    pub doc: ProjectDoc,
    pub folder: Option<ProjectFolder>,
    /// The project as last written to the document.
    committed: Project,
    /// This computer's practice settings as last written to it.
    pub(crate) practice: Practice,
    /// Changed since last saved.
    pub dirty: bool,
    last_save: Instant,
    /// Audio by asset, so clips that undo brings back can play.
    sources: HashMap<String, Arc<StreamSource>>,
    /// Files moved into the project folder: full path before, asset now.
    moved: HashMap<String, String>,
    /// Clips and effects that couldn't be loaded (audio missing, plugin
    /// not installed), by track: kept so saving doesn't lose them.
    pub(crate) missing_clips: HashMap<String, Vec<daw_project::Clip>>,
    missing_fx: HashMap<String, Vec<daw_project::Effect>>,
    loading: Option<mpsc::Receiver<Result<Loaded, String>>>,
    pub dialog: Option<Dialog>,
    /// Watches the saved project's folder for other computers' changes.
    pub(crate) watch: Option<(PathBuf, FolderWatch)>,
    /// Changes from the folder waiting for an edit under way to finish.
    pub(crate) incoming: Vec<Change>,
    /// The document's version as last written to this computer's copy.
    pub(crate) device_version: Vec<u8>,
    pub(crate) last_device_write: Instant,
    /// A bundle being written or read in the background.
    pub(crate) bundle_job: Option<mpsc::Receiver<crate::share::Job>>,
    /// The next change written is not an edit (audio reopened at a new
    /// rate), so it is kept out of undo.
    pub quiet_next: bool,
    /// The window may close: unsaved changes have been dealt with.
    may_close: bool,
    title: String,
}

impl Proj {
    pub fn new() -> Proj {
        let doc = ProjectDoc::new(daw_project::new_peer());
        let committed = doc.get();
        Proj {
            doc,
            folder: None,
            committed,
            practice: Practice::default(),
            dirty: false,
            last_save: Instant::now(),
            sources: HashMap::new(),
            moved: HashMap::new(),
            missing_clips: HashMap::new(),
            missing_fx: HashMap::new(),
            loading: None,
            dialog: None,
            watch: None,
            incoming: Vec::new(),
            device_version: Vec::new(),
            last_device_write: Instant::now(),
            bundle_job: None,
            quiet_next: false,
            may_close: false,
            title: String::new(),
        }
    }

    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }
}

impl App {
    /// The project as the app has it now. Plugin settings are left out
    /// (read only when saving).
    pub(crate) fn project(&self) -> Project {
        Project {
            rate: self.rate,
            tempo_bpm: self.tempo_bpm,
            beats_per_bar: self.beats_per_bar,
            loop_bars: self.loop_bars,
            reference_hz: self.tuner.a4 as f64,
            notes: self.notes.clone(),
            markers: (self.markers.iter())
                .map(|m| daw_project::Marker {
                    id: m.id.clone(),
                    bar: m.bar,
                    name: m.name.clone(),
                })
                .collect(),
            tracks: self.tracks.iter().map(|t| self.track_value(t)).collect(),
            comments: self.comments.clone(),
        }
    }

    fn track_value(&self, t: &Track) -> daw_project::Track {
        let mut clips: Vec<daw_project::Clip> = (t.clips.iter())
            .map(|c| daw_project::Clip {
                id: c.key.clone(),
                asset: c.asset.clone(),
                name: c.name(),
                start: c.start,
                offset: c.offset as i64,
                len: c.len as i64,
                take: c.take.map(|t| t as u32),
                muted: c.muted,
                seq: 0,
            })
            .collect();
        clips.extend(
            self.proj
                .missing_clips
                .get(&t.key)
                .into_iter()
                .flatten()
                .cloned(),
        );
        for (i, c) in clips.iter_mut().enumerate() {
            c.seq = i as i64;
        }
        let mut effects: Vec<daw_project::Effect> = (t.fx.iter())
            .map(|s| daw_project::Effect {
                id: s.key.clone(),
                name: s.name.clone(),
                class: s.class(),
                bundle: s.bundle.to_string_lossy().into_owned(),
                bypass: s.bypass,
                state: None,
            })
            .collect();
        effects.extend(
            self.proj
                .missing_fx
                .get(&t.key)
                .into_iter()
                .flatten()
                .cloned(),
        );
        daw_project::Track {
            id: t.key.clone(),
            name: t.name.clone(),
            gain_db: t.mix.gain_db as f64,
            pan: t.mix.pan as f64,
            mute: t.mix.mute,
            solo: t.mix.solo,
            clips,
            effects,
        }
    }

    /// Writes any change to the document, unless one is under way (a
    /// button held, text being typed) and `force` isn't set.
    pub(crate) fn sync_doc(&mut self, ctx: &egui::Context, force: bool) {
        self.sync_practice(ctx, force);
        if !force && (ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input()) {
            return;
        }
        if self.proj.loading() {
            return;
        }
        let p = self.project();
        if p == self.proj.committed {
            return;
        }
        for c in self.tracks.iter().flat_map(|t| &t.clips) {
            self.proj
                .sources
                .entry(c.asset.clone())
                .or_insert_with(|| c.source.clone());
        }
        if std::mem::take(&mut self.proj.quiet_next) {
            self.proj.doc.set_quietly(&p);
        } else {
            self.proj.doc.set(&p);
        }
        self.proj.committed = p;
        self.proj.dirty = true;
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.proj.doc.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.proj.doc.can_redo()
    }

    pub(crate) fn undo(&mut self, ctx: &egui::Context) {
        self.sync_doc(ctx, true);
        if self.proj.doc.undo() {
            self.apply(self.proj.doc.get());
            self.proj.dirty = true;
        }
    }

    pub(crate) fn redo(&mut self, ctx: &egui::Context) {
        self.sync_doc(ctx, true);
        if self.proj.doc.redo() {
            self.apply(self.proj.doc.get());
            self.proj.dirty = true;
        }
    }

    /// The audio for an asset: one already open, or the file, opened
    /// (and converted to the device's rate if needed) now.
    fn source_for(&mut self, asset: &str) -> Result<Arc<StreamSource>, String> {
        let asset = self
            .proj
            .moved
            .get(asset)
            .cloned()
            .unwrap_or(asset.to_string());
        if let Some(s) = self.proj.sources.get(&asset)
            && s.rate == self.rate
        {
            return Ok(s.clone());
        }
        let s = import::open_at_rate(&self.asset_path(&asset), self.rate, &cache_dir())?;
        self.proj.sources.insert(asset, s.clone());
        Ok(s)
    }

    /// Where an asset's file is: in the project folder, or (before the
    /// project is saved) the full path it is named by.
    fn asset_path(&self, asset: &str) -> PathBuf {
        match &self.proj.folder {
            Some(f) if f.has_asset(asset) => f.asset(asset),
            _ => PathBuf::from(asset),
        }
    }

    /// Makes the app match `p`, keeping what it can (open audio, live
    /// plugins, meters, inputs) for tracks, clips and effects it already
    /// has.
    pub(crate) fn apply(&mut self, p: Project) {
        self.tempo_bpm = p.tempo_bpm;
        self.beats_per_bar = p.beats_per_bar;
        self.loop_bars = p.loop_bars;
        self.tuner.a4 = p.reference_hz as f32;
        self.notes = p.notes.clone();
        self.comments = p.comments.clone();
        self.markers = (p.markers.iter())
            .map(|m| Marker {
                id: m.id.clone(),
                bar: m.bar,
                name: m.name.clone(),
            })
            .collect();
        let k = self.rate as f64 / p.rate.max(1) as f64;
        let scale = |v: i64| (v as f64 * k).round() as i64;
        let mut old: HashMap<String, Track> = (self.tracks.drain(..))
            .map(|t| (t.key.clone(), t))
            .collect();
        let open_key = (self.effects.open_track)
            .and_then(|i| self.proj.committed.tracks.get(i))
            .map(|t| t.id.clone());
        self.proj.missing_clips.clear();
        self.proj.missing_fx.clear();
        let mut problems = Vec::new();
        let mut tracks = Vec::new();
        for pt in &p.tracks {
            let mut t = old.remove(&pt.id).unwrap_or_else(|| {
                let mut t = Track::new(pt.name.clone());
                t.key = pt.id.clone();
                t
            });
            t.name = pt.name.clone();
            t.mix = Mix {
                gain_db: pt.gain_db as f32,
                pan: pt.pan as f32,
                mute: pt.mute,
                solo: pt.solo,
            };
            let mut had: HashMap<String, Clip> =
                t.clips.drain(..).map(|c| (c.key.clone(), c)).collect();
            for pc in &pt.clips {
                let source = match had.get(&pc.id) {
                    Some(c) if c.asset == pc.asset => Ok(c.source.clone()),
                    _ => self.source_for(&pc.asset),
                };
                let source = match source {
                    Ok(s) => s,
                    Err(e) => {
                        problems.push(format!("{}: {e}", pc.name));
                        self.proj
                            .missing_clips
                            .entry(pt.id.clone())
                            .or_default()
                            .push(pc.clone());
                        continue;
                    }
                };
                let mut c = match had.remove(&pc.id) {
                    Some(mut c) => {
                        c.source = source;
                        c
                    }
                    None => Clip::new(source, self.asset_path(&pc.asset), 0, None),
                };
                c.key = pc.id.clone();
                c.asset = pc.asset.clone();
                c.path = self.asset_path(&pc.asset);
                if !pc.name.is_empty() {
                    c.label = pc.name.clone();
                }
                c.start = scale(pc.start).max(0);
                c.offset = (scale(pc.offset).max(0) as usize).min(c.source.frames);
                c.len = (scale(pc.len).max(0) as usize).min(c.source.frames - c.offset);
                c.take = pc.take.map(|t| t as usize);
                c.muted = pc.muted;
                t.clips.push(c);
            }
            let mut had: HashMap<String, crate::fx::FxSlot> =
                t.fx.drain(..).map(|s| (s.key.clone(), s)).collect();
            for pe in &pt.effects {
                match had.remove(&pe.id) {
                    Some(mut s) => {
                        s.bypass = pe.bypass;
                        t.fx.push(s);
                    }
                    None => match self.effects.restore(pe, self.rate) {
                        Ok(s) => t.fx.push(s),
                        Err(e) => {
                            problems.push(format!("{}: {e}", pe.name));
                            self.proj
                                .missing_fx
                                .entry(pt.id.clone())
                                .or_default()
                                .push(pe.clone());
                        }
                    },
                }
            }
            tracks.push(t);
        }
        self.tracks = tracks;
        // Clips and effects no longer there go with the old graph.
        let ids: std::collections::HashSet<u64> = (self.tracks.iter())
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        self.timeline.selected.retain(|id| ids.contains(id));
        self.effects.open_track =
            open_key.and_then(|k| self.tracks.iter().position(|t| t.key == k));
        self.proj.committed = self.project();
        if !problems.is_empty() {
            self.message = Some(format!("Couldn't load {}", problems.join("; ")));
        }
        self.rebuild();
        self.send_loop();
    }

    /// Saves into the project folder, moving takes and copying imported
    /// audio in first. Plugin settings are read now.
    pub(crate) fn save(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let Some(folder) = self.proj.folder.clone() else {
            return Err("the project has no folder yet".into());
        };
        self.sync_doc(ctx, true);
        // Audio from outside the folder comes in.
        let takes = takes_dir();
        let outside: Vec<String> = {
            let mut v: Vec<String> = (self.tracks.iter().flat_map(|t| &t.clips))
                .map(|c| c.asset.clone())
                .filter(|a| !folder.has_asset(a))
                .collect();
            v.sort();
            v.dedup();
            v
        };
        for old in outside {
            let path = PathBuf::from(&old);
            let name = folder.add_asset(&path, path.starts_with(&takes))?;
            let new_path = folder.asset(&name);
            let mut reopened: Option<Arc<StreamSource>> = None;
            for c in self.tracks.iter_mut().flat_map(|t| &mut t.clips) {
                if c.asset != old {
                    continue;
                }
                // Streamed straight from the file that moved: open it
                // where it is now. A converted copy stays as it is.
                if c.source.path == path {
                    if reopened.is_none() {
                        reopened = Some(StreamSource::open(&new_path)?);
                    }
                    c.source = reopened.clone().unwrap_or_else(|| c.source.clone());
                }
                c.asset = name.clone();
                c.path = new_path.clone();
            }
            if let Some(s) = self
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .find(|c| c.asset == name)
            {
                self.proj.sources.insert(name.clone(), s.source.clone());
            }
            self.proj.moved.insert(old, name);
        }
        self.rebuild();
        // Not an edit: kept out of undo.
        let mut p = self.project();
        for (pt, t) in p.tracks.iter_mut().zip(&self.tracks) {
            for (pe, s) in pt.effects.iter_mut().zip(&t.fx) {
                pe.state = s.state();
            }
        }
        self.proj.doc.set_quietly(&p);
        self.proj.committed = self.project();
        let bytes = self.proj.doc.to_bytes();
        if self.proj.doc.take_dropped() {
            folder.save_dropping_backup(&bytes)?;
        } else {
            folder.save(&bytes)?;
        }
        self.save_device_copy(&folder, &bytes)?;
        self.proj.dirty = false;
        self.proj.last_save = Instant::now();
        self.watch_folder();
        Ok(())
    }

    /// Makes a project folder called `name` and saves into it.
    fn save_as(&mut self, ctx: &egui::Context, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            return Err("Choose a name without / \\ : * ? \" < > |".into());
        }
        self.save_in(ctx, &projects_dir().join(name))
    }

    /// Saves the project as a new folder at `dir`.
    pub(crate) fn save_in(&mut self, ctx: &egui::Context, dir: &Path) -> Result<(), String> {
        if dir.exists() {
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            return Err(format!("There is already a project called {name}."));
        }
        self.proj.folder = Some(ProjectFolder::create(dir)?);
        self.save(ctx)
    }

    /// Saves a saved project now and then while it changes, and keeps
    /// this computer's copy for others more up to date than that.
    pub(crate) fn autosave(&mut self, ctx: &egui::Context) {
        self.share_soon(ctx);
        if self.proj.folder.is_some()
            && self.proj.dirty
            && !self.recording
            && self.proj.last_save.elapsed() >= AUTOSAVE_EVERY
            && !ctx.input(|i| i.pointer.any_down())
        {
            if let Err(e) = self.save(ctx) {
                self.message = Some(format!("Autosave failed: {e}"));
            }
            // Try again later either way.
            self.proj.last_save = Instant::now();
        }
    }

    /// Starts afresh with an empty, unsaved project.
    pub(crate) fn new_project(&mut self) {
        self.send(Command::Stop);
        self.send(Command::Seek(Frames(0)));
        self.tracks.clear();
        self.proj = Proj::new();
        self.apply(Project {
            rate: self.rate,
            ..Project::default()
        });
        self.proj.committed = self.project();
        self.proj.doc.set_quietly(&self.proj.committed);
        self.apply_practice();
        self.proj.dirty = false;
    }

    /// Opens the project at `path` in the background.
    pub(crate) fn open_project(&mut self, path: PathBuf) {
        // Given on the command line, it may be relative.
        let path = std::path::absolute(&path).unwrap_or(path);
        let (tx, rx) = mpsc::channel();
        let rate = self.rate;
        std::thread::spawn(move || {
            let _ = tx.send(load(&path, rate));
        });
        self.proj.loading = Some(rx);
    }

    /// Takes in a project that has finished opening.
    pub(crate) fn poll_loading(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.proj.loading else { return };
        ctx.request_repaint_after(Duration::from_millis(100));
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("opening stopped".into()),
        };
        self.proj.loading = None;
        match r {
            Ok(l) => {
                self.send(Command::Stop);
                self.send(Command::Seek(Frames(0)));
                self.tracks.clear();
                self.timeline.selected.clear();
                let mut proj = Proj::new();
                proj.doc = l.doc;
                proj.folder = Some(l.folder);
                proj.sources = l.sources;
                proj.device_version = proj.doc.version();
                self.proj = proj;
                self.apply(self.proj.doc.get());
                self.apply_practice();
                self.watch_folder();
                if !l.failed.is_empty() {
                    self.message = Some(format!("Couldn't open {}", l.failed.join("; ")));
                }
            }
            Err(e) => self.message = Some(format!("Couldn't open the project: {e}")),
        }
    }

    /// Does `then`, first asking about unsaved changes if there are any.
    pub(crate) fn then(&mut self, ctx: &egui::Context, then: After) {
        self.sync_doc(ctx, true);
        if self.proj.folder.is_some() {
            // Saved projects are saved on the way out.
            if self.proj.dirty
                && let Err(e) = self.save(ctx)
            {
                self.message = Some(e);
                return;
            }
        } else if self.proj.dirty && !self.tracks.is_empty() {
            self.proj.dialog = Some(Dialog::Unsaved { then });
            return;
        }
        self.go(ctx, then);
    }

    fn go(&mut self, ctx: &egui::Context, then: After) {
        match then {
            After::New => self.new_project(),
            After::Open(p) => self.open_project(p),
            After::Quit => {
                self.proj.may_close = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    /// Asks before the window closes with unsaved work.
    pub(crate) fn on_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.proj.may_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.then(ctx, After::Quit);
        }
    }

    /// File menu items for projects.
    pub(crate) fn project_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if ui.button("New project").clicked() {
            self.then(&ctx, After::New);
            ui.close();
        }
        if ui.button("Open project…").clicked() {
            self.proj.dialog = Some(Dialog::Open {
                found: find_projects(),
            });
            ui.close();
        }
        if ui.button("Save").on_hover_text("Ctrl+S").clicked() {
            self.save_or_ask(&ctx);
            ui.close();
        }
        if ui.button("Save as…").clicked() {
            self.proj.dialog = Some(Dialog::SaveAs {
                name: String::new(),
                then: None,
            });
            ui.close();
        }
    }

    pub(crate) fn save_or_ask(&mut self, ctx: &egui::Context) {
        if self.proj.folder.is_some() {
            if let Err(e) = self.save(ctx) {
                self.message = Some(e);
            }
        } else {
            self.proj.dialog = Some(Dialog::SaveAs {
                name: String::new(),
                then: None,
            });
        }
    }

    /// The window title: the project's name, marked while unsaved.
    pub(crate) fn title(&mut self, ctx: &egui::Context) {
        let name = self
            .proj
            .folder
            .as_ref()
            .map_or("Untitled".into(), |f| f.name());
        let mark = if self.proj.dirty { " •" } else { "" };
        let title = format!("{name}{mark} — {APP_NAME}");
        if title != self.proj.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.proj.title = title;
        }
    }

    /// The save, open and unsaved-changes windows.
    pub(crate) fn project_windows(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.proj.dialog.take() else {
            return;
        };
        let mut keep = true;
        match dialog {
            Dialog::SaveAs { mut name, then } => {
                let mut go = false;
                let mut browse = false;
                egui::Window::new("Save project")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label("Name");
                        let r = ui.text_edit_singleline(&mut name);
                        if !r.has_focus() && name.is_empty() {
                            r.request_focus();
                        }
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.label(
                            RichText::new(format!("Saved in {}", projects_dir().display())).weak(),
                        );
                        ui.horizontal(|ui| {
                            go = ui.button("Save").clicked() || enter;
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                            if ui.button("Somewhere else…").clicked() {
                                browse = true;
                            }
                        });
                    });
                // The system's own Save window. The project is a folder,
                // so the name chosen there becomes the folder's.
                let chosen = browse
                    .then(|| {
                        rfd::FileDialog::new()
                            .set_title("Save project")
                            .set_directory(projects_dir())
                            .set_file_name(name.trim())
                            .save_file()
                    })
                    .flatten();
                if go || chosen.is_some() {
                    let old = self.proj.folder.take();
                    let saved = match &chosen {
                        Some(dir) => self.save_in(ctx, dir),
                        None => self.save_as(ctx, &name),
                    };
                    match saved {
                        Ok(()) => {
                            keep = false;
                            if let Some(t) = then.clone() {
                                self.go(ctx, t);
                            }
                        }
                        Err(e) => {
                            self.proj.folder = old;
                            self.message = Some(e);
                        }
                    }
                }
                if keep {
                    self.proj.dialog = Some(Dialog::SaveAs { name, then });
                }
            }
            Dialog::Open { found } => {
                let mut pick = None;
                egui::Window::new("Open project")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        if found.is_empty() {
                            ui.label(
                                RichText::new(format!(
                                    "No projects in {}",
                                    projects_dir().display()
                                ))
                                .weak(),
                            );
                        }
                        egui::ScrollArea::vertical()
                            .max_height(240.0)
                            .show(ui, |ui| {
                                for f in &found {
                                    let name = f.file_name().map_or(String::new(), |n| {
                                        n.to_string_lossy().into_owned()
                                    });
                                    if ui.selectable_label(false, name).clicked() {
                                        pick = Some(f.clone());
                                    }
                                }
                            });
                        ui.separator();
                        ui.horizontal(|ui| {
                            // The system's own window, for a project
                            // folder anywhere else.
                            if ui.button("Browse…").clicked() {
                                pick = rfd::FileDialog::new()
                                    .set_title("Open project folder")
                                    .set_directory(projects_dir())
                                    .pick_folder();
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
                if let Some(p) = pick {
                    keep = false;
                    self.then(ctx, After::Open(p));
                }
                if keep {
                    self.proj.dialog = Some(Dialog::Open { found });
                }
            }
            Dialog::Unsaved { then } => {
                egui::Window::new("Unsaved project")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label("This project hasn't been saved. Save it first?");
                        ui.horizontal(|ui| {
                            if ui.button("Save…").clicked() {
                                keep = false;
                                self.proj.dialog = Some(Dialog::SaveAs {
                                    name: String::new(),
                                    then: Some(then.clone()),
                                });
                            }
                            if ui.button("Don't save").clicked() {
                                keep = false;
                                self.go(ctx, then.clone());
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
                if keep {
                    self.proj.dialog = Some(Dialog::Unsaved { then });
                }
            }
        }
    }
}

/// Reads a project and opens its audio, off the UI thread.
fn load(path: &Path, rate: u32) -> Result<Loaded, String> {
    // Other computers' copies, if it is shared, are merged in.
    let (folder, doc, mut failed) = daw_collab::open_doc(path, daw_project::new_peer())?;
    let p = doc.get();
    let mut sources = HashMap::new();
    for c in p.tracks.iter().flat_map(|t| &t.clips) {
        if sources.contains_key(&c.asset) {
            continue;
        }
        let file = if folder.has_asset(&c.asset) {
            folder.asset(&c.asset)
        } else {
            PathBuf::from(&c.asset)
        };
        match import::open_at_rate(&file, rate, &cache_dir()) {
            Ok(s) => {
                sources.insert(c.asset.clone(), s);
            }
            Err(e) => failed.push(format!("{}: {e}", c.name)),
        }
    }
    Ok(Loaded {
        folder,
        doc,
        sources,
        failed,
    })
}

/// Project folders in the projects folder, most recently saved first.
pub(crate) fn find_projects() -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(projects_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let t = std::fs::metadata(p.join(DOC_FILE)).ok()?.modified().ok()?;
            Some((t, p))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Whether a path is a project (its folder or document).
pub fn is_project(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == DOC_FILE) || path.join(DOC_FILE).exists()
}

/// Opens a project given on the command line or dropped on the window.
impl App {
    pub(crate) fn open_dropped(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.then(ctx, After::Open(path));
    }
}
