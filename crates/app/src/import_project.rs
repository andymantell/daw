// SPDX-License-Identifier: GPL-3.0-or-later
//! Parts in from another project, and parts out (Phase 4 plan A5).
//!
//! File → Import from project… lists the projects in the projects
//! folder (or one chosen anywhere), and brings in the tracks ticked,
//! with their audio, and if asked the sections, the tempo and time
//! signature, the key and loop, and the lesson notes. It is one edit:
//! Undo takes it all back. A teacher can start a student's project with
//! last week's backing track and sections.
//!
//! File → Export parts… sends chosen tracks, or a range of bars, out:
//! as a new project of their own (with their audio), or as a stem
//! package: each track as audio, its notes as a MIDI file, and a page
//! saying the tempo, in a folder or packed into one `.tar` file, for
//! someone working in another program.
//!
//! The work is in [`daw_project::Project::part`] and
//! [`daw_project::Project::bring_in`]; the `App` calls here
//! ([`App::import_parts`], [`App::export_as_project`],
//! [`App::export_stem_package`]) are what the windows, and commands from
//! outside the UI, call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use daw_engine::export::Format;
use daw_project::{Brought, Pick, Project, ProjectDoc, ProjectFolder, Range, midifile, new_peer};
use eframe::egui::{self, RichText};

use crate::App;
use crate::export::{self, Package, Request};
use crate::project::{find_projects, projects_dir};

/// The windows' state.
#[derive(Default)]
pub(crate) struct Parts {
    pub(crate) importing: Option<Importing>,
    pub(crate) exporting: Option<Exporting>,
}

/// The Import from project window.
pub(crate) struct Importing {
    /// Projects in the projects folder, most recent first.
    found: Vec<PathBuf>,
    /// The project chosen, once read.
    source: Option<Source>,
    problem: Option<String>,
    pick: Pick,
}

struct Source {
    path: PathBuf,
    name: String,
    project: Project,
}

/// How a part goes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Out {
    /// As a new project.
    Project,
    /// As a stem package.
    Stems,
}

/// What stretch of the song.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Span {
    Song,
    Loop,
    Bars(i64, i64),
}

/// What goes into a stem package besides the tracks chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StemOptions {
    /// Each track (and bus) as audio.
    pub(crate) audio: bool,
    /// Each instrument track's notes as a MIDI file.
    pub(crate) notes: bool,
    /// The mix of the tracks chosen, too.
    pub(crate) mix: bool,
    /// Packed into one `.tar` file.
    pub(crate) archive: bool,
    /// FLAC in place of WAV.
    pub(crate) flac: bool,
}

impl Default for StemOptions {
    fn default() -> StemOptions {
        StemOptions {
            audio: true,
            notes: true,
            mix: false,
            archive: false,
            flac: false,
        }
    }
}

/// The Export parts window.
pub(crate) struct Exporting {
    pub(crate) out: Out,
    /// Track keys.
    tracks: Vec<String>,
    span: Span,
    name: String,
    /// For a new project: what besides tracks and tempo goes.
    pick: Pick,
    stems: StemOptions,
}

/// A name that can be a folder's.
fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        && !name.starts_with('.')
}

/// Reads the project in `path` (its folder or document), with every
/// computer's copy merged in, as opening it would.
fn read(path: &Path) -> Result<(ProjectFolder, Project), String> {
    let (folder, doc, _) = daw_collab::open_doc(path, new_peer())?;
    Ok((folder, doc.get()))
}

impl App {
    /// Opens the Import from project window.
    pub(crate) fn open_import(&mut self) {
        let here = self.proj.folder.as_ref().map(|f| f.dir.clone());
        let found = (find_projects().into_iter())
            .filter(|p| Some(p) != here.as_ref())
            .collect();
        self.parts.importing = Some(Importing {
            found,
            source: None,
            problem: None,
            pick: Pick::default(),
        });
    }

    /// Opens the Export parts window, every track ticked.
    pub(crate) fn open_export_parts(&mut self, out: Out) {
        let name =
            (self.proj.folder.as_ref()).map_or("Part".into(), |f| format!("{} part", f.name()));
        self.parts.exporting = Some(Exporting {
            out,
            tracks: self.tracks.iter().map(|t| t.key.clone()).collect(),
            span: Span::Song,
            name,
            pick: Pick {
                markers: true,
                tempo: true,
                settings: true,
                notes: true,
                ..Pick::default()
            },
            stems: StemOptions::default(),
        });
    }

    /// Brings the parts `pick` names in from the project at `from`, as
    /// one undoable edit. Their audio is copied into this project's
    /// folder, or (before this one is saved) played from the other's
    /// until it is.
    pub(crate) fn import_parts(
        &mut self,
        ctx: &egui::Context,
        from: &Path,
        pick: &Pick,
    ) -> Result<Brought, String> {
        if !self.may_edit_song() {
            return Err("Only the owner and editors can bring tracks in.".into());
        }
        let (src_folder, src) = read(from)?;
        let mut part = src.part(pick);
        let assets: Vec<String> = part.tracks.iter().flat_map(|t| t.assets()).collect();
        let missing = match &self.proj.folder {
            Some(here) => here.bring_assets(&src_folder, &assets)?,
            None => {
                // Not saved yet: the other project's files are used where
                // they are, and come in when this one is saved. Frozen
                // audio is made again then.
                for t in &mut part.tracks {
                    t.frozen = None;
                    t.rename_assets(&|a| {
                        if src_folder.has_asset(a) {
                            src_folder.asset(a).display().to_string()
                        } else {
                            a.to_string()
                        }
                    });
                }
                Vec::new()
            }
        };
        self.sync_doc(ctx, true);
        // Plugins as they are now, so Undo brings their settings back.
        self.write_plugin_states();
        let mut p = self.shown_project();
        let brought = p.bring_in(part, pick);
        let label = self.proj.label.clone();
        self.set_song(&p, label.as_deref());
        self.apply(p);
        self.proj.dirty = true;
        let n = brought.tracks.len();
        let mut m = format!(
            "Brought in {n} track{} from {}.",
            if n == 1 { "" } else { "s" },
            src_folder.name()
        );
        if !missing.is_empty() {
            m += &format!(
                " {} sound file{} weren't in that project's folder.",
                missing.len(),
                if missing.len() == 1 { "" } else { "s" }
            );
        }
        // A problem loading what came in says so itself.
        if self.message.is_none() {
            self.message = Some(m);
        }
        Ok(brought)
    }

    /// Bars `a` to `b` as a range for cutting a part.
    fn range_of(&self, (a, b): (i64, i64)) -> Range {
        let map = self.tempo_map();
        let (ta, tb) = (map.bar_start(a), map.bar_start(b));
        Range {
            bars: (a, b),
            frames: (map.frames_at(ta).0, map.frames_at(tb).0),
            ticks: (ta.0, tb.0),
        }
    }

    /// The bars a span covers, or `None` for the whole song.
    pub(crate) fn span_bars(&self, span: Span) -> Option<(i64, i64)> {
        match span {
            Span::Song => None,
            Span::Loop => Some(self.loop_bars),
            Span::Bars(a, b) => Some((a.max(1), b.max(a.max(1) + 1))),
        }
    }

    /// Saves the parts `pick` names (the tempo always comes) as a new
    /// project at `dir`, cut to `bars` if given, with their audio.
    pub(crate) fn export_as_project(
        &mut self,
        ctx: &egui::Context,
        dir: &Path,
        pick: &Pick,
        bars: Option<(i64, i64)>,
    ) -> Result<(), String> {
        if dir.exists() {
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            return Err(format!("There is already a project called {name}."));
        }
        self.sync_doc(ctx, true);
        self.write_plugin_states();
        let pick = Pick {
            tempo: true,
            ..pick.clone()
        };
        let mut part = self.shown_project().part(&pick);
        if let Some(bars) = bars {
            part.cut_to(self.range_of(bars));
        }
        let folder = ProjectFolder::create(dir)?;
        let made = (|| {
            // Audio from this project's folder, or from where it is
            // until this one is saved.
            let mut renamed = HashMap::new();
            let assets: Vec<String> = part.tracks.iter().flat_map(|t| t.assets()).collect();
            for a in assets {
                match &self.proj.folder {
                    Some(here) if here.has_asset(&a) => {
                        folder.bring_assets(here, std::slice::from_ref(&a))?;
                    }
                    _ if Path::new(&a).is_file() => {
                        let name = folder.add_asset(Path::new(&a), false)?;
                        renamed.insert(a, name);
                    }
                    _ => {}
                }
            }
            for t in &mut part.tracks {
                t.rename_assets(&|a| renamed.get(a).cloned().unwrap_or_else(|| a.to_string()));
            }
            let mut doc = ProjectDoc::new(new_peer());
            doc.set_quietly(&part);
            folder.save(&doc.to_bytes())
        })();
        if let Err(e) = made {
            let _ = std::fs::remove_dir_all(dir);
            return Err(e);
        }
        self.message = Some(format!(
            "Saved {} track{} as the project {}.",
            part.tracks.len(),
            if part.tracks.len() == 1 { "" } else { "s" },
            folder.name()
        ));
        Ok(())
    }

    /// Starts writing a stem package of the tracks `keys`, over `bars`
    /// (or the whole song), into a new numbered folder in `into`, and
    /// shows the Export window for its progress.
    pub(crate) fn export_stem_package(
        &mut self,
        into: &Path,
        keys: &[String],
        bars: Option<(i64, i64)>,
        opts: StemOptions,
    ) -> Result<(), String> {
        if self.export.busy() {
            return Err("An export is already running.".into());
        }
        if keys.is_empty() {
            return Err("Choose at least one track.".into());
        }
        let p = self.project();
        // The tempo where the package starts.
        let mut at = p.part(&Pick {
            tempo: true,
            ..Pick::default()
        });
        let range = bars.map(|b| self.range_of(b));
        if let Some(r) = range {
            at.cut_to(r);
        }
        let mut files = Vec::new();
        if opts.notes {
            for (i, t) in self.tracks.iter().enumerate() {
                if !keys.contains(&t.key) || !t.kind.plays_notes() {
                    continue;
                }
                let mut one = daw_project::Track::new(String::new(), String::new());
                one.note_clips = t.note_clips.clone();
                if let Some(r) = range {
                    let mut cut = Project {
                        tracks: vec![one],
                        ..Project::default()
                    };
                    cut.cut_to(r);
                    one = cut.tracks.remove(0);
                }
                let notes = one.placed_notes();
                if notes.is_empty() {
                    continue;
                }
                let bytes = midifile::write(
                    at.tempo_bpm,
                    (at.beats_per_bar.min(255) as u8, at.beat_unit.min(255) as u8),
                    &[midifile::MidiTrack {
                        name: t.name.clone(),
                        notes,
                    }],
                );
                files.push((
                    format!("{:02} {}.mid", i + 1, export::file_name(&t.name)),
                    bytes,
                ));
            }
        }
        let name = self
            .proj
            .folder
            .as_ref()
            .map_or("Untitled".into(), |f| f.name());
        files.push((
            "About these stems.txt".into(),
            about(
                &name,
                &at,
                bars,
                self.rate,
                !p.tempo_changes.is_empty() || !p.meter_changes.is_empty(),
            )
            .into_bytes(),
        ));
        let req = Request {
            loop_only: false,
            stems: opts.audio,
            bus_stems: opts.audio,
            format: if opts.flac { Format::Flac } else { Format::Wav },
            folder: into.to_path_buf(),
            mix: opts.mix,
            video: false,
            only: Some(keys.to_vec()),
            bars,
            package: Some(Package {
                files,
                archive: opts.archive,
            }),
            sections: Vec::new(),
            play_along: false,
        };
        // The mix is of the tracks chosen only.
        let muted: Vec<(usize, bool)> = (self.tracks.iter().enumerate())
            .filter(|(_, t)| opts.mix && !keys.contains(&t.key) && !t.kind.is_bus())
            .map(|(i, t)| (i, t.mix.mute))
            .collect();
        for &(i, _) in &muted {
            self.tracks[i].mix.mute = true;
        }
        let started = self.start_export(req);
        for (i, was) in muted {
            self.tracks[i].mix.mute = was;
        }
        started?;
        self.export.open = true;
        Ok(())
    }

    pub(crate) fn parts_windows(&mut self, ctx: &egui::Context) {
        self.import_window(ctx);
        self.export_parts_window(ctx);
    }

    fn import_window(&mut self, ctx: &egui::Context) {
        let Some(w) = &mut self.parts.importing else {
            return;
        };
        let mut open = true;
        let mut chosen: Option<PathBuf> = None;
        let mut go = false;
        egui::Window::new("Import from project")
            .open(&mut open)
            .default_pos([280.0, 120.0])
            .default_width(400.0)
            .show(ctx, |ui| {
                ui.label("Bring tracks, sections and settings in from another project.");
                ui.horizontal(|ui| {
                    let l = ui.label("Project");
                    let shown = w
                        .source
                        .as_ref()
                        .map_or("Choose…".into(), |s| s.name.clone());
                    egui::ComboBox::from_id_salt("import-from")
                        .selected_text(shown)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for f in &w.found {
                                let name = f.file_name().unwrap_or_default().to_string_lossy();
                                let on = w.source.as_ref().is_some_and(|s| s.path == *f);
                                // ai: import_from_project.project
                                if ui.selectable_label(on, name).clicked() {
                                    chosen = Some(f.clone());
                                }
                            }
                            if w.found.is_empty() {
                                ui.label(RichText::new("No other projects yet.").weak());
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    // ai: import_from_project.project
                    if ui.button("Other folder…").clicked()
                        && let Some(d) = rfd::FileDialog::new()
                            .set_title("Import from project")
                            .set_directory(projects_dir())
                            .pick_folder()
                    {
                        chosen = Some(d);
                    }
                });
                if let Some(e) = &w.problem {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                let Some(s) = &w.source else {
                    return;
                };
                ui.separator();
                pick_tracks(ui, &s.project, &mut w.pick.tracks);
                ui.separator();
                let p = &s.project;
                // ai: import_from_project.routing
                ui.checkbox(
                    &mut w.pick.routing,
                    "The buses and folders they play through",
                )
                .on_hover_text("Without them, the tracks play straight to the master");
                ui.add_enabled_ui(!p.markers.is_empty(), |ui| {
                    // ai: import_from_project.markers
                    ui.checkbox(
                        &mut w.pick.markers,
                        format!(
                            "Sections ({} marker{})",
                            p.markers.len(),
                            plural(p.markers.len())
                        ),
                    )
                    .on_hover_text("Those already here at the same bar are kept once");
                });
                // ai: import_from_project.tempo
                ui.checkbox(
                    &mut w.pick.tempo,
                    format!(
                        "Tempo and time signature ({} BPM in {}/{}{})",
                        p.tempo_bpm,
                        p.beats_per_bar,
                        p.beat_unit,
                        if p.tempo_changes.is_empty() && p.meter_changes.is_empty() {
                            ""
                        } else {
                            ", with changes"
                        }
                    ),
                )
                .on_hover_text("In place of this project's");
                // ai: import_from_project.settings
                ui.checkbox(&mut w.pick.settings, "Key, tuning and loop")
                    .on_hover_text("In place of this project's");
                // ai: import_from_project.notes
                ui.checkbox(&mut w.pick.notes, "Lesson notes and tab blocks")
                    .on_hover_text("Added after this project's notes");
                ui.add_space(4.0);
                let any = !w.pick.tracks.is_empty()
                    || w.pick.markers
                    || w.pick.tempo
                    || w.pick.settings
                    || w.pick.notes;
                // ai: import_from_project
                if ui.add_enabled(any, egui::Button::new("Bring in")).clicked() {
                    go = true;
                }
            });
        if let Some(path) = chosen {
            match read(&path) {
                Ok((folder, project)) => {
                    w.pick = Pick {
                        tracks: project.tracks.iter().map(|t| t.id.clone()).collect(),
                        routing: true,
                        markers: !project.markers.is_empty(),
                        ..Pick::default()
                    };
                    w.source = Some(Source {
                        path,
                        name: folder.name(),
                        project,
                    });
                    w.problem = None;
                }
                Err(e) => w.problem = Some(format!("Couldn't read that project: {e}")),
            }
        }
        if go && let Some(s) = &w.source {
            let (path, pick) = (s.path.clone(), w.pick.clone());
            match self.import_parts(ctx, &path, &pick) {
                Ok(_) => self.parts.importing = None,
                Err(e) => {
                    if let Some(w) = &mut self.parts.importing {
                        w.problem = Some(e);
                    }
                }
            }
            return;
        }
        if !open {
            self.parts.importing = None;
        }
    }

    fn export_parts_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.parts.exporting.take() else {
            return;
        };
        let mut open = true;
        let mut go = false;
        let busy = self.export.busy();
        let here = self.project();
        let loop_bars = self.loop_bars;
        egui::Window::new("Export parts")
            .open(&mut open)
            .default_pos([280.0, 120.0])
            .default_width(400.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // ai: export_as_project
                    ui.radio_value(&mut w.out, Out::Project, "As a new project");
                    // ai: export_stem_package
                    ui.radio_value(&mut w.out, Out::Stems, "As a stem package");
                });
                ui.label(
                    RichText::new(match w.out {
                        Out::Project => {
                            "The tracks with their audio, effects and routing, in a project of \
                             their own."
                        }
                        Out::Stems => {
                            "Each track as a sound file and its notes as a MIDI file, all \
                             starting at the same moment, for another program."
                        }
                    })
                    .weak(),
                );
                ui.separator();
                pick_tracks(ui, &here, &mut w.tracks);
                ui.separator();
                // ai: export_as_project.from_bar
                ui.radio_value(&mut w.span, Span::Song, "The whole song");
                // ai: export_as_project.from_bar
                ui.radio_value(
                    &mut w.span,
                    Span::Loop,
                    format!("The loop (bars {} to {})", loop_bars.0, loop_bars.1 - 1),
                );
                ui.horizontal(|ui| {
                    let mut bars = match w.span {
                        Span::Bars(a, b) => (a, b - 1),
                        _ => (loop_bars.0, loop_bars.1 - 1),
                    };
                    let on = matches!(w.span, Span::Bars(..));
                    // ai: export_as_project.from_bar
                    if ui.radio(on, "Bars").clicked() {
                        w.span = Span::Bars(bars.0, bars.1 + 1);
                    }
                    // ai: export_as_project.from_bar
                    let a = ui.add(egui::DragValue::new(&mut bars.0).range(1..=9999));
                    ui.label("to");
                    // ai: export_as_project.to_bar
                    let b = ui.add(egui::DragValue::new(&mut bars.1).range(bars.0..=9999));
                    if a.changed() || b.changed() {
                        w.span = Span::Bars(bars.0, bars.1.max(bars.0) + 1);
                    }
                });
                ui.separator();
                match w.out {
                    Out::Project => {
                        ui.horizontal(|ui| {
                            let l = ui.label("Name");
                            // ai: export_as_project.name
                            ui.add(egui::TextEdit::singleline(&mut w.name).desired_width(240.0))
                                .labelled_by(l.id);
                        });
                        // ai: export_as_project.routing
                        ui.checkbox(
                            &mut w.pick.routing,
                            "The buses and folders they play through",
                        );
                        // ai: export_as_project.markers
                        ui.checkbox(&mut w.pick.markers, "Sections");
                        // ai: export_as_project.settings
                        ui.checkbox(&mut w.pick.settings, "Key, tuning and loop");
                        // ai: export_as_project.notes
                        ui.checkbox(&mut w.pick.notes, "Lesson notes and tab blocks");
                        ui.label(RichText::new("The tempo and time signature always go.").weak());
                    }
                    Out::Stems => {
                        let s = &mut w.stems;
                        // ai: export_stem_package.audio
                        ui.checkbox(&mut s.audio, "Each track as audio");
                        // ai: export_stem_package.notes
                        ui.checkbox(&mut s.notes, "Each instrument track's notes as MIDI");
                        // ai: export_stem_package.mix
                        ui.checkbox(&mut s.mix, "The mix of these tracks too");
                        // ai: export_stem_package.flac
                        ui.checkbox(&mut s.flac, "FLAC in place of WAV")
                            .on_hover_text("The same sound in about half the space");
                        // ai: export_stem_package.archive
                        ui.checkbox(&mut s.archive, "Packed into one file (.tar)")
                            .on_hover_text("Easier to send; any archive tool opens it");
                    }
                }
                ui.add_space(4.0);
                let ok = !w.tracks.is_empty()
                    && match w.out {
                        Out::Project => name_ok(w.name.trim()),
                        Out::Stems => !busy && (w.stems.audio || w.stems.notes || w.stems.mix),
                    };
                let label = match w.out {
                    Out::Project => "Export project",
                    Out::Stems => "Export stems",
                };
                // ai: export_as_project
                if ui.add_enabled(ok, egui::Button::new(label)).clicked() {
                    go = true;
                }
            });
        if go {
            let bars = self.span_bars(w.span);
            let r = match w.out {
                Out::Project => {
                    let pick = Pick {
                        tracks: w.tracks.clone(),
                        ..w.pick.clone()
                    };
                    let dir = projects_dir().join(w.name.trim());
                    self.export_as_project(ctx, &dir, &pick, bars)
                }
                Out::Stems => {
                    let into = self.export.dir().to_path_buf();
                    self.export_stem_package(&into, &w.tracks, bars, w.stems)
                }
            };
            match r {
                Ok(()) => return,
                Err(e) => self.message = Some(e),
            }
        }
        if open {
            self.parts.exporting = Some(w);
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Ticks for each of `p`'s tracks, by id, with All and None.
fn pick_tracks(ui: &mut egui::Ui, p: &Project, picked: &mut Vec<String>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Tracks").strong());
        // ai: import_from_project.tracks
        if ui.small_button("All").clicked() {
            *picked = p.tracks.iter().map(|t| t.id.clone()).collect();
        }
        // ai: import_from_project.tracks
        if ui.small_button("None").clicked() {
            picked.clear();
        }
    });
    if p.tracks.is_empty() {
        ui.label(RichText::new("No tracks.").weak());
    }
    egui::ScrollArea::vertical()
        .id_salt("pick-tracks")
        .max_height(200.0)
        .show(ui, |ui| {
            for t in &p.tracks {
                let mut on = picked.contains(&t.id);
                let what = match (t.clips.len(), t.note_clips.len()) {
                    _ if t.kind.is_bus() => t.kind.name().to_string(),
                    (0, 0) => "empty".into(),
                    (a, 0) => format!("{a} clip{}", plural(a)),
                    (0, n) => format!("{n} note clip{}", plural(n)),
                    (a, n) => format!("{a} clip{}, {n} note clip{}", plural(a), plural(n)),
                };
                // ai: import_from_project.tracks
                let r = ui.checkbox(&mut on, format!("{}  ·  {what}", t.name));
                if r.changed() {
                    if on {
                        picked.push(t.id.clone());
                    } else {
                        picked.retain(|k| *k != t.id);
                    }
                }
            }
        });
}

/// The page that goes with a stem package.
fn about(name: &str, at: &Project, bars: Option<(i64, i64)>, rate: u32, changes: bool) -> String {
    let mut s = format!("Stems from {name}\n\n");
    match bars {
        Some((a, b)) => s += &format!("Bars {a} to {}.\n", b - 1),
        None => s += "The whole song.\n",
    }
    s += &format!(
        "Tempo {} BPM in {}/{}{}.\n",
        at.tempo_bpm,
        at.beats_per_bar,
        at.beat_unit,
        if changes {
            " at the start (the song changes tempo or time signature later; \
             the MIDI files keep the starting tempo)"
        } else {
            ""
        }
    );
    s += &format!(
        "Sound files: {rate} Hz, 24-bit. Every file starts at the same moment, so line \
         them all up at the start.\n"
    );
    s += "MIDI files, if any: each instrument track's notes.\n";
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("daw-parts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A short WAV file of a steady tone.
    fn wav(path: &Path, rate: u32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..rate / 2 {
            w.write_sample(((i as f32 * 0.05).sin() * 8000.0) as i16)
                .unwrap();
        }
        w.finalize().unwrap();
    }

    fn app() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_| App::new(Vec::new()));
        h.run();
        h
    }

    /// Steps the app until `done`, or a couple of seconds pass.
    fn wait(h: &mut Harness<'_, App>, done: impl Fn(&App) -> bool) {
        for _ in 0..300 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn ctx(h: &Harness<'_, App>) -> egui::Context {
        h.ctx.clone()
    }

    /// A saved lesson project with a guitar track, a section and a tempo.
    fn lesson(dir: &Path) -> PathBuf {
        let mut h = app();
        let file = dir.join("riff.wav");
        wav(&file, 48_000);
        let c = ctx(&h);
        h.state_mut().add_file(&file);
        wait(&mut h, |a| !a.tracks.is_empty());
        {
            let a = h.state_mut();
            a.tracks[0].name = "Backing track".into();
            a.tempo_bpm = 90.0;
            a.markers.push(crate::timeline::Marker {
                id: "m".into(),
                bar: 5,
                name: "Riff".into(),
            });
            a.notes = "# Riff".into();
        }
        let lesson = dir.join("Lesson 1");
        h.state_mut().save_in(&c, &lesson).unwrap();
        lesson
    }

    #[test]
    fn tracks_come_in_from_another_project_with_their_audio_and_undo_together() {
        let dir = scratch("import");
        let lesson = lesson(&dir);
        let mut h = app();
        let c = ctx(&h);
        let student = dir.join("Student");
        h.state_mut().save_in(&c, &student).unwrap();
        let (_, src) = read(&lesson).unwrap();
        let pick = Pick {
            markers: true,
            tempo: true,
            notes: true,
            ..Pick::all(&src)
        };
        let got = h.state_mut().import_parts(&c, &lesson, &pick).unwrap();
        let a = h.state_mut();
        assert_eq!(got.tracks.len(), 1);
        assert_eq!(a.tracks.len(), 1);
        assert_eq!(a.tracks[0].name, "Backing track");
        assert_eq!(a.tracks[0].clips.len(), 1, "{:?}", a.message);
        let asset = a.tracks[0].clips[0].asset.clone();
        let here = a.proj.folder.clone().unwrap();
        assert!(
            here.has_asset(&asset),
            "the audio came into this project's folder"
        );
        assert_eq!(a.tempo_bpm, 90.0);
        assert_eq!(a.markers.len(), 1);
        assert_eq!(a.notes, "# Riff");
        // One undo takes it all back.
        a.undo(&c);
        assert!(a.tracks.is_empty());
        assert!(a.markers.is_empty());
        assert_eq!(a.tempo_bpm, 120.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unsaved_project_plays_the_other_ones_audio_until_it_is_saved() {
        let dir = scratch("unsaved");
        let lesson = lesson(&dir);
        let mut h = app();
        let c = ctx(&h);
        let (_, src) = read(&lesson).unwrap();
        h.state_mut()
            .import_parts(&c, &lesson, &Pick::all(&src))
            .unwrap();
        let a = h.state_mut();
        let asset = a.tracks[0].clips[0].asset.clone();
        assert!(
            Path::new(&asset).is_file(),
            "{asset} is the other project's file"
        );
        let mine = dir.join("Mine");
        a.save_in(&c, &mine).unwrap();
        let asset = a.tracks[0].clips[0].asset.clone();
        assert!(a.proj.folder.as_ref().unwrap().has_asset(&asset));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chosen_tracks_and_bars_go_out_as_a_new_project() {
        let dir = scratch("export");
        let lesson = lesson(&dir);
        let mut h = app();
        let c = ctx(&h);
        h.state_mut().open_project(lesson.clone());
        wait(&mut h, |a| !a.proj.loading() && !a.tracks.is_empty());
        let a = h.state_mut();
        assert_eq!(a.tracks.len(), 1);
        let pick = Pick {
            tracks: vec![a.tracks[0].key.clone()],
            markers: true,
            ..Pick::default()
        };
        let out = dir.join("Riff only");
        a.export_as_project(&c, &out, &pick, Some((5, 9))).unwrap();
        assert!(
            a.export_as_project(&c, &out, &pick, None).is_err(),
            "never over a project"
        );
        let (folder, p) = read(&out).unwrap();
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.tempo_bpm, 90.0);
        assert_eq!(p.markers.len(), 1);
        assert_eq!(p.markers[0].bar, 1, "bar 5 is now the start");
        // Half a second of audio at bar 1 is before bar 5: nothing left.
        assert!(p.tracks[0].clips.is_empty());
        let whole = dir.join("Whole");
        a.export_as_project(&c, &whole, &pick, None).unwrap();
        let (folder2, p) = read(&whole).unwrap();
        assert!(folder2.has_asset(&p.tracks[0].clips[0].asset));
        assert_ne!(folder.dir, folder2.dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stem_package_has_audio_notes_and_a_page_and_can_be_one_file() {
        let dir = scratch("stems");
        let mut h = app();
        let file = dir.join("riff.wav");
        wav(&file, 48_000);
        h.state_mut().add_file(&file);
        wait(&mut h, |a| !a.tracks.is_empty());
        let mid = dir.join("keys.mid");
        let notes = vec![daw_project::Note::new(0, 960, 60, 100)];
        let bytes = midifile::write(
            120.0,
            (4, 4),
            &[midifile::MidiTrack {
                name: "Keys".into(),
                notes,
            }],
        );
        std::fs::write(&mid, bytes).unwrap();
        h.state_mut().import_midi(&mid);
        assert_eq!(h.state().tracks.len(), 2, "{:?}", h.state().message);
        let keys: Vec<String> = h.state().tracks.iter().map(|t| t.key.clone()).collect();
        let opts = StemOptions {
            archive: true,
            ..StemOptions::default()
        };
        h.state_mut()
            .export_stem_package(&dir, &keys, None, opts)
            .unwrap();
        for _ in 0..3000 {
            h.state_mut().export.poll();
            if !h.state().export.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let tar = dir.join("Stems 1.tar");
        assert!(
            tar.is_file(),
            "{:?}",
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .collect::<Vec<_>>()
        );
        assert!(
            !dir.join("Stems 1").exists(),
            "packed in place of the folder"
        );
        let bytes = std::fs::read(&tar).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Stems 1/01 riff.wav"));
        assert!(text.contains("Stems 1/About these stems.txt"));
        assert!(text.contains("Stems 1/02 Keys.mid"), "the notes as MIDI");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_assistant_brings_tracks_in_and_sends_them_out() {
        use serde_json::json;
        let dir = scratch("agent");
        let lesson = lesson(&dir);
        let mut d = crate::agent::driver::Driver::new();
        let got = d
            .call(
                "import_from_project",
                json!({"project": lesson.display().to_string(),
                       "tracks": ["Backing track"], "markers": true}),
            )
            .unwrap();
        assert_eq!(got["tracks"].as_array().unwrap().len(), 1, "{got}");
        let p = d.project();
        assert_eq!(p["tracks"].as_array().unwrap().len(), 1, "{p}");
        assert!(
            d.call(
                "import_from_project",
                json!({"project": lesson.display().to_string(), "tracks": ["Nope"]})
            )
            .is_err()
        );
        d.app().export.set_default_dir(dir.clone());
        let started = d
            .call("export_stem_package", json!({"from_bar": 1, "to_bar": 1}))
            .unwrap();
        assert_eq!(started["started"], true);
        assert!(
            d.call("export_stem_package", json!({"from_bar": 3, "to_bar": 2}))
                .is_err()
        );
        d.command("undo");
        assert!(d.app().tracks.is_empty(), "one undo step");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_page_says_the_tempo_and_bars() {
        let p = Project {
            tempo_bpm: 90.0,
            beats_per_bar: 3,
            ..Project::default()
        };
        let s = about("Lesson", &p, Some((5, 9)), 48_000, false);
        assert!(s.contains("Bars 5 to 8."));
        assert!(s.contains("Tempo 90 BPM in 3/4."));
    }
}
