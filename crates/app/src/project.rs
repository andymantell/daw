// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects in the app (roadmap 1.6 and 1.8): the project document kept
//! in step with what is on screen, undo and redo through it, saving into
//! a project folder (takes and imported audio move in), opening,
//! autosave, and crash recovery for a project never saved.
//!
//! Edits are made to the app's own model as before. Each frame, once no
//! button is held and no text is being typed, the model is compared with
//! what was last written to the document, and any difference is written
//! as one undoable edit. A drag or a burst of typing is one step.
//!
//! A project with no folder yet is written now and then to a recovery
//! file in the user's profile, and removed again once it is saved or
//! deliberately left. If the app stops without doing either (a crash, a
//! power cut), the next start offers to bring it back. Its audio is
//! named by full path (takes stay in `Unsaved takes` until a save), so
//! it comes back with it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daw_collab::watch::{Change, FolderWatch};
use daw_engine::engine::Command;
use daw_engine::fade::{Curve, Fade, GainPoint};
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

/// How often a project never saved is written to the recovery file
/// while it changes: more often than autosave, as nothing else holds it.
const RECOVER_EVERY: Duration = Duration::from_secs(10);

/// Where new projects go: e.g. `C:\Users\<name>\DAW\Projects`.
pub fn projects_dir() -> PathBuf {
    user_dir().join(APP_NAME).join("Projects")
}

/// Where a project never saved is kept in case the app stops: e.g.
/// `C:\Users\<name>\DAW\Recovery\Unsaved project.daw`.
pub fn recovery_file() -> PathBuf {
    user_dir()
        .join(APP_NAME)
        .join("Recovery")
        .join("Unsaved project.daw")
}

/// The most files a project opens at once. Opening is mostly reading
/// the disk, with conversion between rates on the processor, so a few at
/// a time is quicker than one; more than this only queue on the disk.
const OPEN_AT_ONCE: usize = 8;

/// How far a project opening in the background has got.
#[derive(Default)]
pub struct Progress {
    /// Audio files it uses, once its document has been read.
    total: AtomicUsize,
    done: AtomicUsize,
}

impl Progress {
    /// Files opened so far, and how many there are (0 until known).
    pub fn get(&self) -> (usize, usize) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

/// What to do once unsaved changes are dealt with.
#[derive(Clone)]
pub enum After {
    New,
    /// A new project made from a template.
    NewFrom(Box<Project>),
    Open(PathBuf),
    Quit,
    /// Bring back the project found in the recovery file.
    Recover,
}

/// A project found in the recovery file at start, waiting for the user
/// to restore or discard it.
pub struct Found {
    doc: ProjectDoc,
    /// How long ago it was last written, and what it holds.
    pub age: Duration,
    pub summary: String,
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
    pub(crate) sources: HashMap<String, Arc<StreamSource>>,
    /// Assets that couldn't be opened, and why.
    failed: Vec<String>,
}

pub struct Proj {
    pub doc: ProjectDoc,
    pub folder: Option<ProjectFolder>,
    /// The project as last written to the document.
    pub(crate) committed: Project,
    /// This computer's practice settings as last written to it.
    pub(crate) practice: Practice,
    /// Changed since last saved.
    pub dirty: bool,
    last_save: Instant,
    /// Audio by asset, so clips that undo brings back can play.
    pub(crate) sources: HashMap<String, Arc<StreamSource>>,
    /// Files moved into the project folder: full path before, asset now.
    moved: HashMap<String, String>,
    /// Clips and effects that couldn't be loaded (audio missing, plugin
    /// not installed), by track: kept so saving doesn't lose them. Each
    /// effect is kept with its place in the chain.
    pub(crate) missing_clips: HashMap<String, Vec<daw_project::Clip>>,
    pub(crate) missing_fx: HashMap<String, Vec<(usize, daw_project::Effect)>>,
    loading: Option<mpsc::Receiver<Result<Loaded, String>>>,
    /// How far the project opening has got.
    pub(crate) progress: Arc<Progress>,
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
    /// Where a project never saved is kept in case the app stops; none
    /// in tests, which must not touch the user's own.
    pub(crate) recovery: Option<PathBuf>,
    /// The document's version as last written there.
    recovery_version: Vec<u8>,
    last_recovery: Instant,
    /// What was found there at start, until the user decides. Nothing
    /// is written there meanwhile, so it isn't lost.
    pub(crate) found: Option<Found>,
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
            progress: Arc::default(),
            dialog: None,
            watch: None,
            incoming: Vec::new(),
            device_version: Vec::new(),
            last_device_write: Instant::now(),
            bundle_job: None,
            quiet_next: false,
            may_close: false,
            title: String::new(),
            recovery: (!cfg!(test)).then(recovery_file),
            recovery_version: Vec::new(),
            last_recovery: Instant::now(),
            found: None,
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
            beat_unit: self.time.beat_unit,
            tempo_changes: self.time.tempos.clone(),
            meter_changes: self.time.meters.clone(),
            loop_bars: self.loop_bars,
            reference_hz: self.tuner.a4 as f64,
            notes: self.notes.clone(),
            tabs: self.tabs.clone(),
            assignments: self.assignments.clone(),
            markers: (self.markers.iter())
                .map(|m| daw_project::Marker {
                    id: m.id.clone(),
                    bar: m.bar,
                    name: m.name.clone(),
                })
                .collect(),
            tracks: self.tracks.iter().map(|t| self.track_value(t)).collect(),
            comments: self.comments.clone(),
            edit_groups: self.edit_groups_value(),
            control_groups: self.groups.clone(),
            cue_mixes: self.cues.mixes.clone(),
            timecode: self.timecode.clone(),
            hit_points: self.hit_points.clone(),
            scale: self.scale,
            grooves: self.grooves.clone(),
            midi_maps: self.midi_maps.clone(),
            patterns: self.patterns.clone(),
            videos: self.video.clips.iter().map(|c| c.value()).collect(),
        }
    }

    pub(crate) fn track_value(&self, t: &Track) -> daw_project::Track {
        // The project is written at the app's rate, so this is 1 (kept
        // so lanes scale like clips should the rates ever differ).
        let p_k = 1.0;
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
                comp: c.comp,
                seq: 0,
                gain_db: c.gain_db as f64,
                fade_in: fade_value(c.fade_in),
                fade_out: fade_value(c.fade_out),
                envelope: (c.envelope.iter())
                    .map(|p| daw_project::GainPoint {
                        at: p.at as i64,
                        db: p.db as f64,
                    })
                    .collect(),
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
        let mut effects: Vec<daw_project::Effect> =
            t.fx.iter().map(crate::fx::FxSlot::value).collect();
        // Missing effects keep their places, so the chain's order in the
        // document stays as the computers with them see it.
        for (i, e) in self.proj.missing_fx.get(&t.key).into_iter().flatten() {
            effects.insert((*i).min(effects.len()), e.clone());
        }
        daw_project::Track {
            id: t.key.clone(),
            name: t.name.clone(),
            gain_db: t.mix.gain_db as f64,
            pan: t.mix.pan as f64,
            mute: t.mix.mute,
            solo: t.mix.solo,
            clips,
            note_clips: t.note_clips.clone(),
            instrument: t.instrument.as_ref().map(crate::instrument::Slot::value),
            instrument_outputs: t.instrument_outputs.clone(),
            layers: t.layers.iter().map(crate::layers::Layer::value).collect(),
            effects,
            frozen: t.frozen.clone(),
            freeze: t.freeze,
            comps: self.comps_value(t),
            comp: t.comp.clone(),
            kind: t.kind,
            parent: t.parent.clone(),
            output: t.output.clone(),
            sends: t.sends.clone(),
            group: t.group.clone(),
            notation: t.notation.clone(),
            automation: (t.automation.iter())
                .map(|l| daw_project::Lane {
                    param: l.param.clone(),
                    points: (l.points.iter())
                        .map(|p| daw_project::AutoPoint {
                            at: (p.at as f64 * p_k).round() as i64,
                            value: p.value,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// The track's comps, the one playing as its pieces now have it.
    fn comps_value(&self, t: &Track) -> Vec<daw_project::Comp> {
        let mut comps = t.comps.clone();
        if let Some(c) = comps.iter_mut().find(|c| Some(&c.id) == t.comp.as_ref()) {
            c.parts = crate::comping::parts_of(&t.clips);
        }
        comps
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
        let before = self.proj.doc.get();
        if self.proj.doc.undo() {
            let after = self.proj.doc.get();
            // Plugin settings a restore changed come back too.
            self.reload_changed_fx(&before, &after);
            self.apply(after);
            self.proj.dirty = true;
        }
    }

    pub(crate) fn redo(&mut self, ctx: &egui::Context) {
        self.sync_doc(ctx, true);
        let before = self.proj.doc.get();
        if self.proj.doc.redo() {
            let after = self.proj.doc.get();
            // Plugin settings a restore changed come back too.
            self.reload_changed_fx(&before, &after);
            self.apply(after);
            self.proj.dirty = true;
        }
    }

    /// The audio for an asset: one already open, or the file, opened
    /// (and converted to the device's rate if needed) now.
    pub(crate) fn source_for(&mut self, asset: &str) -> Result<Arc<StreamSource>, String> {
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
        self.effects.folder = self.proj.folder.as_ref().map(|f| f.dir.clone());
        self.tempo_bpm = p.tempo_bpm;
        self.beats_per_bar = p.beats_per_bar;
        self.time = crate::tempo::Changes {
            beat_unit: p.beat_unit,
            tempos: p.tempo_changes.clone(),
            meters: p.meter_changes.clone(),
        };
        self.loop_bars = p.loop_bars;
        self.tuner.a4 = p.reference_hz as f32;
        self.notes = p.notes.clone();
        self.tabs = p.tabs.clone();
        self.assignments = p.assignments.clone();
        self.comments = p.comments.clone();
        self.edit_groups = p.edit_groups.clone();
        self.groups = p.control_groups.clone();
        self.cues.mixes = p.cue_mixes.clone();
        self.markers = (p.markers.iter())
            .map(|m| Marker {
                id: m.id.clone(),
                bar: m.bar,
                name: m.name.clone(),
            })
            .collect();
        let k = self.rate as f64 / p.rate.max(1) as f64;
        let scale = |v: i64| (v as f64 * k).round() as i64;
        self.timecode = daw_project::TimecodeSetup {
            offset: scale(p.timecode.offset),
            ..p.timecode.clone()
        };
        // At another rate, hit points go back on the start of their
        // frame.
        let tc = crate::hitpoints::clock(&self.timecode);
        let rated = self.rate != p.rate;
        self.hit_points = (p.hit_points.iter())
            .map(|h| daw_project::HitPoint {
                at: if rated {
                    tc.snap(scale(h.at) + 1, self.rate)
                } else {
                    h.at
                },
                ..h.clone()
            })
            .collect();
        self.scale = p.scale;
        self.grooves = p.grooves.clone();
        self.midi_maps = p.midi_maps.clone();
        self.patterns = p.patterns.clone();
        let folder = self.proj.folder.clone();
        let video_path = |a: &str| match &folder {
            Some(f) if f.has_asset(a) => f.asset(a),
            _ => PathBuf::from(a),
        };
        self.video.apply(&p.videos, &video_path, &scale);
        let mut old: HashMap<String, Track> = (self.tracks.drain(..))
            .map(|t| (t.key.clone(), t))
            .collect();
        let open_key = (self.effects.open_track)
            .and_then(|i| self.proj.committed.tracks.get(i))
            .map(|t| t.id.clone());
        self.proj.missing_clips.clear();
        self.proj.missing_fx.clear();
        let mut problems = Vec::new();
        let read = self.sound_reader();
        // Tracks playing their frozen audio for missing plugins.
        let mut frozen = Vec::new();
        let mut tracks = Vec::new();
        for pt in &p.tracks {
            let mut t = old.remove(&pt.id).unwrap_or_else(|| {
                let mut t = Track::new(pt.name.clone());
                t.key = pt.id.clone();
                t
            });
            t.name = pt.name.clone();
            t.kind = pt.kind;
            t.parent = pt.parent.clone();
            t.output = pt.output.clone();
            t.sends = pt.sends.clone();
            t.group = pt.group.clone();
            t.freeze = pt.freeze;
            t.note_clips = pt.note_clips.clone();
            t.instrument_outputs = pt.instrument_outputs.clone();
            t.notation = pt.notation.clone();
            match (&mut t.instrument, &pt.instrument) {
                (Some(slot), Some(e)) if slot.same_shape(e) => slot.load(e),
                (slot, Some(e)) => {
                    let (new, missing) = crate::instrument::Slot::restore_any(
                        e,
                        &read,
                        &mut self.effects,
                        self.rate,
                    );
                    problems.extend(missing);
                    new.make_into(self.rate, &mut self.pending_instruments);
                    *slot = Some(new);
                }
                (slot, None) => *slot = None,
            }
            let mut had: HashMap<String, crate::layers::Layer> =
                t.layers.drain(..).map(|l| (l.key.clone(), l)).collect();
            for pl in &pt.layers {
                let layer = match had.remove(&pl.id) {
                    Some(mut l) if l.slot.same_shape(&pl.instrument) => {
                        l.slot.load(&pl.instrument);
                        l.set(pl);
                        l
                    }
                    _ => {
                        let (l, missing) =
                            crate::layers::Layer::restore(pl, &read, &mut self.effects, self.rate);
                        problems.extend(missing);
                        l.slot.make_into(self.rate, &mut self.pending_instruments);
                        l
                    }
                };
                t.layers.push(layer);
            }
            t.automation = (pt.automation.iter())
                .map(|l| daw_project::Lane {
                    param: l.param.clone(),
                    points: (l.points.iter())
                        .map(|p| daw_project::AutoPoint {
                            at: scale(p.at).max(0),
                            value: p.value,
                        })
                        .collect(),
                })
                .collect();
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
                c.comp = pc.comp;
                c.gain_db = pc.gain_db as f32;
                c.fade_in = fade_from(pc.fade_in, &scale);
                c.fade_out = fade_from(pc.fade_out, &scale);
                c.envelope = (pc.envelope.iter())
                    .map(|p| GainPoint {
                        at: scale(p.at).max(0) as usize,
                        db: p.db as f32,
                    })
                    .collect();
                c.set_fade(false, c.fade_in.len);
                c.set_fade(true, c.fade_out.len);
                t.clips.push(c);
            }
            let mut had: HashMap<String, crate::fx::FxSlot> =
                t.fx.drain(..).map(|s| (s.key.clone(), s)).collect();
            for (i, pe) in pt.effects.iter().enumerate() {
                match had.remove(&pe.id) {
                    Some(mut s) => {
                        s.bypass = pe.bypass;
                        let before = s.layout();
                        s.sidechain = pe.sidechain.clone();
                        s.outputs = pe.outputs.clone();
                        s.place = pe.container.clone().map(|c| (c, pe.branch));
                        // A plugin's side buses are switched on as it
                        // starts, so a change makes it again.
                        if s.is_plugin()
                            && s.layout() != before
                            && let Err(e) = self.effects.reload(&mut s, self.rate)
                        {
                            problems.push(format!("{}: {e}", pe.name));
                        }
                        s.set_params(&pe.params);
                        if s.response_ref() != pe.response
                            && let Err(e) =
                                self.effects.set_response(&mut s, &pe.response, self.rate)
                        {
                            problems.push(format!("{}: {e}", pe.name));
                        }
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
                                .push((i, pe.clone()));
                        }
                    },
                }
            }
            t.comps = (pt.comps.iter())
                .map(|c| daw_project::Comp {
                    parts: (c.parts.iter())
                        .map(|p| daw_project::CompPart {
                            start: scale(p.start),
                            end: scale(p.end),
                            ..*p
                        })
                        .collect(),
                    ..c.clone()
                })
                .collect();
            t.comp = pt.comp.clone();
            t.frozen = pt.frozen.clone();
            t.frozen_audio = None;
            let missing = self.proj.missing_fx.contains_key(&pt.id);
            if let Some(f) = &pt.frozen
                && (missing || pt.freeze)
            {
                match self.source_for(&f.asset) {
                    Ok(s) => {
                        t.frozen_audio = Some(s);
                        if missing && f.fits(pt, p.rate) {
                            frozen.push(pt.name.clone());
                        }
                    }
                    Err(e) => problems.push(format!("frozen audio for {}: {e}", pt.name)),
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
            let mut m = format!("Couldn't load {}", problems.join("; "));
            if !frozen.is_empty() {
                m += &format!(". Playing frozen audio instead on {}", frozen.join(", "));
            }
            self.message = Some(m);
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
        self.flush_practice();
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
        // So do instruments' sound files.
        for slot in self.tracks.iter_mut().flat_map(|t| t.instruments_mut()) {
            for r in slot.outside_files() {
                let name = folder.add_asset(Path::new(&r), false)?;
                slot.rename_sample(&r, &name);
            }
        }
        // So do reverb response files.
        for s in self.tracks.iter().flat_map(|t| &t.fx) {
            let r = s.response_ref();
            if r.is_empty()
                || r.starts_with(daw_effects::library::PREFIX)
                || folder.has_asset(&r)
                || !Path::new(&r).is_file()
            {
                continue;
            }
            let name = folder.add_asset(Path::new(&r), false)?;
            self.effects.remember_as(&r, &name);
            s.rename_response(&name);
        }
        // And videos.
        let videos: Vec<String> = (self.video.clips.iter())
            .map(|c| c.asset.clone())
            .filter(|a| !folder.has_asset(a) && Path::new(a).is_file())
            .collect();
        for old in videos {
            let name = folder.add_asset(Path::new(&old), false)?;
            self.video.saved_as(&old, &name, &folder.asset(&name));
        }
        self.effects.folder = Some(folder.dir.clone());
        self.rebuild();
        self.write_plugin_states();
        let bytes = self.proj.doc.to_bytes();
        if self.proj.doc.take_dropped() {
            folder.save_dropping_backup(&bytes)?;
        } else {
            folder.save(&bytes)?;
        }
        self.save_device_copy(&folder, &bytes)?;
        // Saved now, so nothing to recover.
        self.discard_recovery();
        self.proj.dirty = false;
        self.proj.last_save = Instant::now();
        self.watch_folder();
        self.freeze_tracks();
        Ok(())
    }

    /// Reads each plugin's settings into the document. Not an edit:
    /// kept out of undo.
    pub(crate) fn write_plugin_states(&mut self) {
        let mut p = self.project();
        for (pt, t) in p.tracks.iter_mut().zip(&mut self.tracks) {
            // By id: effects missing here sit among them, and keep the
            // settings they came with.
            for pe in &mut pt.effects {
                if let Some(s) = t.fx.iter_mut().find(|s| s.key == pe.id) {
                    pe.state = s.state();
                }
            }
            if let (Some(pi), Some(i)) = (&mut pt.instrument, &mut t.instrument)
                && i.is_plugin()
            {
                pi.state = i.state();
            }
            for (pl, l) in pt.layers.iter_mut().zip(&mut t.layers) {
                if l.slot.is_plugin() {
                    pl.instrument.state = l.slot.state();
                }
            }
        }
        self.proj.doc.set_quietly(&p);
        self.proj.committed = self.project();
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
        self.save(ctx)?;
        self.remember_recent(dir);
        Ok(())
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
        if self.proj.folder.is_none()
            && self.proj.dirty
            && !self.recording
            && self.proj.last_recovery.elapsed() >= RECOVER_EVERY
            && !ctx.input(|i| i.pointer.any_down())
        {
            if let Err(e) = self.write_recovery() {
                self.message = Some(format!("Couldn't keep a recovery copy: {e}"));
            }
            self.proj.last_recovery = Instant::now();
        }
        // The window only redraws when something happens, so ask to be
        // woken when the next save or recovery copy is due.
        if self.proj.dirty {
            let (every, last) = match self.proj.folder {
                Some(_) => (AUTOSAVE_EVERY, self.proj.last_save),
                None => (RECOVER_EVERY, self.proj.last_recovery),
            };
            let due = every.saturating_sub(last.elapsed());
            ctx.request_repaint_after(due.max(Duration::from_millis(500)));
        }
    }

    /// Writes a project never saved to the recovery file, if it changed
    /// since last time, whole or not at all.
    pub(crate) fn write_recovery(&mut self) -> Result<(), String> {
        if self.proj.found.is_some() || self.proj.loading() {
            return Ok(());
        }
        let Some(path) = self.proj.recovery.clone() else {
            return Ok(());
        };
        self.write_plugin_states();
        let version = self.proj.doc.version();
        if version == self.proj.recovery_version {
            return Ok(());
        }
        let err = |e: std::io::Error| format!("{}: {e}", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let tmp = path.with_extension("daw.tmp");
        std::fs::write(&tmp, self.proj.doc.to_bytes()).map_err(err)?;
        std::fs::rename(&tmp, &path).map_err(err)?;
        self.proj.recovery_version = version;
        Ok(())
    }

    /// Removes the recovery file: the project in it was saved or
    /// deliberately left. Kept while the user hasn't yet decided about
    /// one found at start.
    fn discard_recovery(&mut self) {
        if self.proj.found.is_some() {
            return;
        }
        if let Some(path) = &self.proj.recovery {
            let _ = std::fs::remove_file(path);
        }
        self.proj.recovery_version.clear();
    }

    /// Looks for a project left in the recovery file by a run of the app
    /// that stopped without saving or leaving it, to offer it back.
    pub(crate) fn look_for_recovery(&mut self) {
        let Some(path) = &self.proj.recovery else {
            return;
        };
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let age = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .unwrap_or_default();
        let Ok(doc) = ProjectDoc::from_bytes(&bytes, daw_project::new_peer()) else {
            // Damaged, perhaps cut off mid-write by the crash itself:
            // kept for the curious, but not offered.
            let _ = std::fs::rename(path, path.with_extension("daw.damaged"));
            return;
        };
        let p = doc.get();
        if p.tracks.is_empty()
            && p.markers.is_empty()
            && p.notes.trim().is_empty()
            && p.tabs.is_empty()
        {
            let _ = std::fs::remove_file(path);
            return;
        }
        let clips = p.tracks.iter().map(|t| t.clips.len()).sum::<usize>();
        let summary = format!(
            "{} {}, {} {}",
            p.tracks.len(),
            if p.tracks.len() == 1 {
                "track"
            } else {
                "tracks"
            },
            clips,
            if clips == 1 { "clip" } else { "clips" },
        );
        self.proj.found = Some(Found { doc, age, summary });
    }

    /// Brings back the project found in the recovery file, unsaved.
    fn recover(&mut self) {
        let Some(found) = self.proj.found.take() else {
            return;
        };
        self.send(Command::Stop);
        self.send(Command::Seek(Frames(0)));
        self.tracks.clear();
        self.timeline.selected.clear();
        let mut proj = Proj::new();
        proj.recovery = self.proj.recovery.take();
        proj.doc = found.doc;
        proj.recovery_version = proj.doc.version();
        self.proj = proj;
        self.sign_in();
        self.apply(self.proj.doc.get());
        // Unsaved, so it asks before being left and stays recoverable.
        self.proj.dirty = true;
        if self.message.is_none() {
            self.message = Some("Brought back the unsaved project. Save it to keep it.".into());
        }
    }

    /// Starts afresh with an empty, unsaved project.
    pub(crate) fn new_project(&mut self) {
        self.new_project_from(Project {
            rate: self.rate,
            ..Project::default()
        });
    }

    /// Starts afresh with `p`, unsaved, as where undo begins.
    pub(crate) fn new_project_from(&mut self, p: Project) {
        self.send(Command::Stop);
        self.send(Command::Seek(Frames(0)));
        self.tracks.clear();
        let mut proj = Proj::new();
        proj.found = self.proj.found.take();
        proj.recovery = self.proj.recovery.take();
        self.proj = proj;
        self.sign_in();
        self.apply(p);
        self.proj.committed = self.project();
        self.proj.doc.set_quietly(&self.proj.committed);
        self.practice_log = Default::default();
        self.apply_practice();
        self.proj.dirty = false;
    }

    /// Opens the project at `path` in the background.
    pub(crate) fn open_project(&mut self, path: PathBuf) {
        // Given on the command line, it may be relative.
        let path = std::path::absolute(&path).unwrap_or(path);
        let (tx, rx) = mpsc::channel();
        let rate = self.rate;
        let progress = Arc::new(Progress::default());
        self.proj.progress = progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load(&path, rate, &progress));
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
                proj.found = self.proj.found.take();
                proj.recovery = self.proj.recovery.take();
                proj.doc = l.doc;
                let dir = l.folder.dir.clone();
                proj.folder = Some(l.folder);
                proj.sources = l.sources;
                proj.device_version = proj.doc.version();
                self.proj = proj;
                self.remember_recent(&dir);
                self.sign_in();
                self.apply(self.proj.doc.get());
                self.practice_log = Default::default();
                self.apply_practice();
                self.watch_folder();
                self.merge_bundle_after_open(ctx);
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
        // Somewhere to go: the start screen has done its job.
        self.start.showing = None;
        // The project being left was saved or let go on purpose.
        if self.proj.folder.is_none() && !matches!(then, After::Recover) {
            self.discard_recovery();
        }
        match then {
            After::New => self.new_project(),
            After::NewFrom(p) => self.new_project_from(*p),
            After::Open(p) => self.open_project(p),
            After::Recover => self.recover(),
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

    /// The offer to bring back a project found in the recovery file.
    fn recovery_window(&mut self, ctx: &egui::Context) {
        let Some(found) = &self.proj.found else {
            return;
        };
        let mut restore = false;
        let mut discard = false;
        egui::Window::new("Recover unsaved project")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{APP_NAME} closed without saving a project you were working on."
                ));
                ui.label(format!("Last kept {}: {}.", ago(found.age), found.summary));
                ui.label(RichText::new("Restored, it is still unsaved until you save it.").weak());
                ui.horizontal(|ui| {
                    restore = ui.button("Restore").clicked();
                    discard = ui.button("Discard").clicked();
                });
            });
        if restore {
            self.then(ctx, After::Recover);
        } else if discard {
            self.proj.found = None;
            if let Some(path) = &self.proj.recovery {
                let _ = std::fs::remove_file(path);
            }
            // The project open now is written there afresh.
            self.proj.recovery_version.clear();
        }
    }

    /// The save, open, unsaved-changes and recovery windows.
    pub(crate) fn project_windows(&mut self, ctx: &egui::Context) {
        if self.proj.dialog.is_none() {
            self.recovery_window(ctx);
        }
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

/// How long ago, in words: "just now", "5 minutes ago", "2 days ago".
pub(crate) fn ago(d: Duration) -> String {
    let s = d.as_secs();
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86400 => (s / 3600, "hour"),
        _ => (s / 86400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Reads a project and opens its audio, off the UI thread: several files
/// at once, each asset once, counting them off in `progress`.
fn load(path: &Path, rate: u32, progress: &Progress) -> Result<Loaded, String> {
    // Other computers' copies, if it is shared, are merged in.
    let (folder, doc, mut failed) = daw_collab::open_doc(path, daw_project::new_peer())?;
    let p = doc.get();
    // Each asset with the name of the first clip that plays it.
    let mut wanted: Vec<(&str, &str, PathBuf)> = Vec::new();
    for c in p.tracks.iter().flat_map(|t| &t.clips) {
        if wanted.iter().any(|(a, ..)| *a == c.asset) {
            continue;
        }
        let file = if folder.has_asset(&c.asset) {
            folder.asset(&c.asset)
        } else {
            PathBuf::from(&c.asset)
        };
        wanted.push((&c.asset, &c.name, file));
    }
    progress.total.store(wanted.len(), Ordering::Relaxed);
    let cache = cache_dir();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, OPEN_AT_ONCE)
        .min(wanted.len());
    let mut opened: Vec<(usize, Result<Arc<StreamSource>, String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((.., file)) = wanted.get(i) else {
                            break;
                        };
                        out.push((i, import::open_at_rate(file, rate, &cache)));
                        progress.done.fetch_add(1, Ordering::Relaxed);
                    }
                    out
                })
            })
            .collect();
        // A file whose opening panicked is left out here, and tried
        // again (and reported) as the project is shown.
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    // In the project's order, so problems read the same each time.
    opened.sort_by_key(|(i, _)| *i);
    let mut sources = HashMap::new();
    for (i, r) in opened {
        let (asset, name, _) = &wanted[i];
        match r {
            Ok(s) => {
                sources.insert(asset.to_string(), s);
            }
            Err(e) => failed.push(format!("{name}: {e}")),
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
    find_projects_in(&projects_dir())
}

/// Project folders in `dir`, most recently saved first.
pub(crate) fn find_projects_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
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

/// A clip's fade as the document has it.
fn fade_value(f: Fade) -> daw_project::Fade {
    daw_project::Fade {
        len: f.len as i64,
        curve: match f.curve {
            Curve::Linear => daw_project::Curve::Linear,
            Curve::EqualPower => daw_project::Curve::EqualPower,
            Curve::SCurve => daw_project::Curve::SCurve,
            Curve::Fast => daw_project::Curve::Fast,
            Curve::Slow => daw_project::Curve::Slow,
        },
    }
}

/// A fade from the document, its length counted at the device's rate.
fn fade_from(f: daw_project::Fade, scale: &dyn Fn(i64) -> i64) -> Fade {
    Fade {
        len: scale(f.len).max(0) as usize,
        curve: match f.curve {
            daw_project::Curve::Linear => Curve::Linear,
            daw_project::Curve::EqualPower => Curve::EqualPower,
            daw_project::Curve::SCurve => Curve::SCurve,
            daw_project::Curve::Fast => Curve::Fast,
            daw_project::Curve::Slow => Curve::Slow,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use eframe::egui;
    use egui_kittest::Harness;

    use super::After;
    use crate::App;

    /// An empty folder of its own for each test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-recovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The app, keeping its recovery file at `file`, and having looked
    /// there as it does at start.
    fn app(file: &Path) -> Harness<'static, App> {
        let file = file.to_path_buf();
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(move |_cc| {
                let mut app = App::new(Vec::new());
                app.proj.recovery = Some(file.clone());
                app.look_for_recovery();
                app
            });
        h.run();
        h
    }

    /// A second of quiet audio at the app's rate.
    fn wav(dir: &Path, rate: u32) -> PathBuf {
        let path = dir.join("guitar.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate {
            w.write_sample(((i % 100) as i16 - 50) * 10).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    /// The app with an imported file on a track and a marker, unsaved.
    fn with_work(h: &mut Harness<'_, App>, dir: &Path) {
        let rate = h.state().rate;
        h.state_mut().add_file(&wav(dir, rate));
        for _ in 0..200 {
            h.step();
            if !h.state().tracks.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(h.state().tracks.len(), 1, "the file was imported");
        h.state_mut().markers.push(crate::timeline::Marker {
            id: daw_project::new_id(),
            bar: 2,
            name: "Chorus".into(),
        });
        h.run();
        assert!(h.state().proj.dirty);
        assert!(h.state().proj.folder.is_none());
    }

    fn ctx(h: &Harness<'_, App>) -> egui::Context {
        h.ctx.clone()
    }

    #[test]
    fn unsaved_work_comes_back_after_a_crash() {
        let dir = scratch("crash");
        let file = dir.join("Recovery").join("Unsaved project.daw");
        let mut h = app(&file);
        assert!(h.state().proj.found.is_none(), "nothing to recover yet");
        with_work(&mut h, &dir);
        // Kept by itself once a while has passed.
        h.run();
        assert!(!file.exists(), "not yet");
        h.state_mut().proj.last_recovery = (std::time::Instant::now())
            .checked_sub(super::RECOVER_EVERY)
            .unwrap();
        h.run();
        assert!(file.exists());
        // The app stops here without saving or quitting.
        drop(h);

        let mut h = app(&file);
        let found = h.state().proj.found.as_ref().expect("offered back");
        assert_eq!(found.summary, "1 track, 1 clip");
        assert!(h.state().tracks.is_empty());
        // Nothing is written over it while the offer is open.
        h.state_mut().write_recovery().unwrap();
        assert!(file.exists());
        let ctx = ctx(&h);
        h.state_mut().then(&ctx, After::Recover);
        h.run();
        let app = h.state();
        assert!(app.proj.found.is_none());
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.tracks[0].clips.len(), 1);
        assert_eq!(app.tracks[0].clips[0].len, app.rate as usize);
        assert_eq!(app.markers.len(), 1);
        assert_eq!(app.markers[0].name, "Chorus");
        assert!(app.proj.dirty, "still unsaved");
        assert!(app.proj.folder.is_none());
        assert!(file.exists(), "kept until saved or let go");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_or_letting_go_removes_the_recovery_file() {
        let dir = scratch("save");
        let file = dir.join("Recovery").join("Unsaved project.daw");
        let mut h = app(&file);
        with_work(&mut h, &dir);
        h.state_mut().write_recovery().unwrap();
        assert!(file.exists());
        let ctx = ctx(&h);
        h.state_mut().save_in(&ctx, &dir.join("Song")).unwrap();
        assert!(!file.exists(), "saved, so nothing to recover");

        // A new project, worked on and then deliberately not saved.
        h.state_mut().then(&ctx, After::New);
        with_work(&mut h, &dir);
        h.state_mut().write_recovery().unwrap();
        assert!(file.exists());
        h.state_mut().then(&ctx, After::New);
        assert!(matches!(
            h.state().proj.dialog,
            Some(super::Dialog::Unsaved { .. })
        ));
        h.state_mut().proj.dialog = None;
        h.state_mut().go(&ctx, After::New);
        assert!(!file.exists(), "\"Don't save\" lets it go");
        drop(h);
        let h = app(&file);
        assert!(h.state().proj.found.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_recovery_file_is_set_aside() {
        let dir = scratch("damaged");
        let file = dir.join("Unsaved project.daw");
        std::fs::write(&file, b"not a project").unwrap();
        let h = app(&file);
        assert!(h.state().proj.found.is_none());
        assert!(!file.exists());
        assert!(file.with_extension("daw.damaged").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_opens_its_files_side_by_side() {
        let dir = scratch("parallel");
        let mut h = app(&dir.join("recovery.daw"));
        let rate = h.state().rate;
        for i in 0..6 {
            // Each different, as the same audio twice is kept once.
            let copy = dir.join(format!("part {i}.wav"));
            std::fs::rename(wav(&dir, rate + i), &copy).unwrap();
            h.state_mut().add_file(&copy);
        }
        for _ in 0..400 {
            h.step();
            if h.state().tracks.len() == 6 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(h.state().tracks.len(), 6);
        let song = dir.join("Six parts");
        let ctx = ctx(&h);
        h.state_mut().save_in(&ctx, &song).unwrap();
        // One file gone missing is reported, the rest still open.
        let clip = &h.state().tracks[2].clips[0];
        let (gone, name) = (clip.path.clone(), clip.name());
        std::fs::remove_file(&gone).unwrap();
        let progress = super::Progress::default();
        let l = super::load(&song, rate, &progress).unwrap();
        assert_eq!(progress.get(), (6, 6));
        assert_eq!(l.sources.len(), 5);
        assert_eq!(l.failed.len(), 1, "{:?}", l.failed);
        assert!(l.failed[0].starts_with(&name), "{:?}", l.failed);
    }

    #[test]
    fn ages_read_as_words() {
        use std::time::Duration;
        assert_eq!(super::ago(Duration::from_secs(5)), "just now");
        assert_eq!(super::ago(Duration::from_secs(60)), "1 minute ago");
        assert_eq!(super::ago(Duration::from_secs(7300)), "2 hours ago");
        assert_eq!(super::ago(Duration::from_secs(3 * 86400)), "3 days ago");
    }
}
