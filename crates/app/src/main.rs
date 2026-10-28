// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop application. Phase 1 so far: open WAV files as tracks,
//! play them with a mixer, loop, metronome and count-in, choose the audio
//! device, and record: per track input, arm and monitor, with takes kept
//! and switchable; VST 3 effects as track inserts; a tuner and practice
//! mute; export of the mix and stems to WAV; a timeline to edit clips
//! and mark sections.
//!
//! Usage: `daw [file.wav ...]`, or drop WAV files on the window. Files at
//! another sample rate from the device are converted in the background
//! into a cache folder first.
//!
//! `daw --scan-vst3 <bundle>` is the plugin scanner: the app runs itself
//! that way to open each plugin safely in a separate process.

mod export;
mod fx;
mod perf;
mod project;
mod share;
mod timeline;
mod trainer;
mod tuner;

#[cfg(test)]
mod a11y;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daw_audio_io::{self as audio, DeviceProfile, HostInfo, Running, SavedProfiles};
use daw_engine::engine::{self, Command, Controller};
use daw_engine::graph::{ClipSpec, Input, Mix, Session, TrackSpec};
use daw_engine::import;
use daw_engine::metronome::Metronome;
use daw_engine::record::{RecorderThread, Take};
use daw_engine::stream::{StreamSource, StreamerThread};
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};
use eframe::egui::{self, Color32, Key, RichText, Sense, Stroke, vec2};
use fx::{Effects, FxSlot};
use timeline::{Clip, Lane, Marker, Song};

struct Track {
    /// The track's id in the project document.
    key: String,
    name: String,
    /// Imported files and recorded takes, in the order added (later ones
    /// draw on top).
    clips: Vec<Clip>,
    mix: Mix,
    input: Option<Input>,
    arm: bool,
    monitor: bool,
    /// Silenced to play the part yourself. Kept apart from `mix.mute`,
    /// which belongs to the mix.
    practice_mute: bool,
    /// Insert effects, in order.
    fx: Vec<FxSlot>,
    /// Peak levels shown on the meter, decaying.
    meter: (f32, f32),
}

impl Track {
    /// The mix the engine plays: the track's own, with practice mute.
    fn heard(&self) -> Mix {
        Mix {
            mute: self.mix.mute || self.practice_mute,
            ..self.mix
        }
    }

    fn new(name: String) -> Track {
        Track {
            key: daw_project::new_id(),
            name,
            clips: Vec::new(),
            mix: Mix::default(),
            input: None,
            arm: false,
            monitor: false,
            practice_mute: false,
            fx: Vec::new(),
            meter: (0.0, 0.0),
        }
    }
}

/// A file being opened (and converted, if needed) on a thread of its own.
struct Import {
    path: PathBuf,
    /// Opened again at a new rate for the clips already using it, rather
    /// than added as a track.
    reopen: bool,
    rate: u32,
    done: mpsc::Receiver<Result<Arc<StreamSource>, String>>,
}

/// Where converted copies of files go until projects have folders of
/// their own.
pub(crate) fn cache_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("daw").join("converted")
}

/// The product's name, until it has a real one: the window title and the
/// folder for unsaved takes follow it.
const APP_NAME: &str = "DAW";

/// Where takes go until the project is saved, when they move into the
/// project's folder: a visible folder in the user's profile (the owner's
/// choice, 2026-10-27), e.g. `C:\Users\<name>\DAW\Unsaved takes`.
fn takes_dir() -> PathBuf {
    user_dir().join(APP_NAME).join("Unsaved takes")
}

/// The user's own folder: `C:\Users\<name>` on Windows.
fn user_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(std::env::temp_dir, PathBuf::from)
}

/// What the audio side is doing.
enum Audio {
    Running(Running),
    /// No device could be opened, so the engine runs without sound; the
    /// message says why. The `Running` is held to keep it going.
    Silent(Running, String),
    /// Between devices.
    Stopped,
}

struct App {
    tracks: Vec<Track>,
    imports: Vec<Import>,
    tempo_bpm: f64,
    beats_per_bar: u32,
    rate: u32,
    ctl: Controller,
    _disk: StreamerThread,
    _recorder: RecorderThread,
    recording: bool,
    audio: Audio,
    profile: Option<DeviceProfile>,
    /// The settings being edited in the device window.
    draft: Option<DeviceProfile>,
    hosts: Vec<HostInfo>,
    /// Devices on the draft's driver that can record.
    input_devices: Vec<String>,
    show_devices: bool,
    metronome: Metronome,
    count_in: bool,
    looping: bool,
    /// Bars, counted from 1: loop from the start of `loop_bars.0` to the
    /// start of `loop_bars.1`.
    loop_bars: (i64, i64),
    master_meter: (f32, f32),
    message: Option<String>,
    /// The device stopped (unplugged, say): when to look for it again.
    reconnect: Option<Instant>,
    /// Device settings kept between runs.
    saved: SavedProfiles,
    effects: Effects,
    tuner: tuner::Tuner,
    export: export::Export,
    /// Practice speed, percent, and transpose.
    speed_pct: f64,
    semitones: i32,
    cents: i32,
    trainer: trainer::Trainer,
    /// A track's tuning being measured, and the last result.
    detecting: Option<(String, mpsc::Receiver<Option<f32>>)>,
    detected: Option<(String, Option<f32>)>,
    markers: Vec<Marker>,
    timeline: timeline::Timeline,
    perf: perf::Perf,
    /// Lesson notes, and whether their window is open.
    notes: String,
    notes_open: bool,
    shortcuts_open: bool,
    proj: project::Proj,
}

/// Every keyboard shortcut, for Help → Keyboard shortcuts. Letters work
/// when no text field has the keyboard.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Space", "Play or stop"),
    ("Home", "Back to the start"),
    ("R", "Record on or off: armed tracks record while playing"),
    ("L", "Loop on or off"),
    ("K", "Metronome on or off"),
    ("C", "Count-in on or off"),
    ("Tab, Shift+Tab", "Move between controls"),
    (
        "Enter",
        "Press the focused button, or open the focused list",
    ),
    ("Ctrl+S", "Save the project"),
    ("Ctrl+Z", "Undo"),
    ("Ctrl+Shift+Z, Ctrl+Y", "Redo"),
    ("S", "Split clips at the playhead"),
    ("Delete", "Delete the selected clips"),
    ("M", "Mark a section at the playhead's bar"),
    ("Esc", "Clear the clip selection"),
];

/// How often to look for a lost device.
const RECONNECT_EVERY: Duration = Duration::from_secs(2);

impl App {
    fn new(files: Vec<String>) -> App {
        // The device used last time, if it is still there. Tests run
        // without settings or a device.
        let saved = if cfg!(test) {
            SavedProfiles::default()
        } else {
            load_settings()
        };
        let profile = saved
            .current()
            .filter(|p| audio::is_present(p))
            .cloned()
            .or_else(|| (!cfg!(test)).then(audio::default_profile).flatten());
        let rate = profile.as_ref().map_or(48_000, |p| p.rate);
        let (ctl, eng, disk, recorder) = new_engine();
        let mut app = App {
            tracks: Vec::new(),
            imports: Vec::new(),
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            rate,
            ctl,
            _disk: disk,
            _recorder: recorder,
            recording: false,
            audio: Audio::Stopped,
            draft: profile.clone(),
            profile: profile.clone(),
            hosts: audio::list(),
            input_devices: input_names(&draft_host(&profile)),
            show_devices: false,
            metronome: Metronome::default(),
            count_in: false,
            looping: false,
            loop_bars: (1, 5),
            master_meter: (0.0, 0.0),
            message: None,
            reconnect: None,
            saved,
            effects: Effects::new(config_dir().join("plugins.txt")),
            tuner: tuner::Tuner::new(rate),
            export: export::Export::default(),
            speed_pct: 100.0,
            semitones: 0,
            cents: 0,
            trainer: trainer::Trainer::default(),
            detecting: None,
            detected: None,
            markers: Vec::new(),
            timeline: timeline::Timeline::default(),
            perf: perf::Perf::default(),
            notes: String::new(),
            notes_open: false,
            shortcuts_open: false,
            proj: project::Proj::new(),
        };
        if !cfg!(test) {
            app.effects.start_scan();
        }
        app.audio = app.open_device(eng);
        // The empty project as it starts is not an edit.
        let start = app.project();
        app.proj.doc.set_quietly(&start);
        app.apply(start);
        for f in files {
            let f = PathBuf::from(f);
            if project::is_project(&f) {
                app.open_project(f);
            } else if daw_collab::bundle::is_bundle(&f) {
                app.import_bundle(f);
            } else {
                app.add_file(&f);
            }
        }
        app.rebuild();
        app
    }

    fn open_device(&mut self, eng: engine::Engine) -> Audio {
        let Some(p) = &self.profile else {
            return Audio::Silent(
                audio::start_silent(self.rate, 256, eng),
                "No audio device found".into(),
            );
        };
        // `start` consumes the engine even when it fails, so try with a
        // stand-in first only if needed: open directly, and fall back to a
        // fresh engine pair running silently.
        match audio::start(p, eng) {
            Ok(r) => Audio::Running(r),
            Err(e) => {
                let eng = self.replace_engine();
                Audio::Silent(audio::start_silent(self.rate, 256, eng), e)
            }
        }
    }

    /// Starts a fresh engine on the chosen device, keeping the session.
    fn restart_audio(&mut self) {
        // Stop the old audio before starting the new.
        self.audio = Audio::Stopped;
        self.rate = self.profile.as_ref().map_or(self.rate, |p| p.rate);
        // Files and takes at the old rate are opened again at the new one.
        let mut stale: Vec<PathBuf> = (self.tracks.iter().flat_map(|t| &t.clips))
            .filter(|c| c.source.rate != self.rate)
            .map(|c| c.path.clone())
            .collect();
        stale.sort();
        stale.dedup();
        for path in stale {
            self.start_import(path, true);
        }
        let eng = self.replace_engine();
        self.audio = self.open_device(eng);
    }

    /// A fresh engine with this session, metronome, loop and position;
    /// the returned half goes to the audio device. Recording stops, which
    /// finishes any take in progress.
    fn replace_engine(&mut self) -> engine::Engine {
        let pos = self.ctl.status().position();
        let (ctl, eng, disk, recorder) = new_engine();
        let mut old = std::mem::replace(&mut self.ctl, ctl);
        self._disk = disk;
        // Dropping the old recorder finishes any take in progress; keep it.
        drop(std::mem::replace(&mut self._recorder, recorder));
        let takes = old.finished_takes();
        self.add_takes(takes);
        self.recording = false;
        // The old engine took its effects with it: make them again, at
        // the new rate, with the same settings.
        let rate = self.profile.as_ref().map_or(self.rate, |p| p.rate);
        self.effects.pending.clear();
        for t in &mut self.tracks {
            let mut failed = Vec::new();
            for (i, slot) in t.fx.iter_mut().enumerate() {
                if let Err(e) = self.effects.reload(slot, rate) {
                    self.message = Some(format!("{}: {e}", slot.name));
                    failed.push(i);
                }
            }
            for i in failed.into_iter().rev() {
                t.fx.remove(i);
            }
        }
        self.rebuild();
        self.send(Command::SetMetronome(self.metronome));
        self.send(self.tuner.command());
        self.send(self.practice());
        self.send_loop();
        self.send(Command::Seek(pos));
        eng
    }

    /// Notices a device that has stopped, plays silently meanwhile, and
    /// starts again on it once it is back.
    fn watch_device(&mut self, ctx: &egui::Context) {
        if let Audio::Running(r) = &self.audio
            && r.status.failed.load(Ordering::Relaxed)
        {
            self.restart_audio();
            if !matches!(self.audio, Audio::Running(_)) {
                self.message = Some(
                    "The audio device stopped (unplugged?). Playing silently until it is back."
                        .into(),
                );
                self.reconnect = Some(Instant::now() + RECONNECT_EVERY);
            }
        }
        let Some(at) = self.reconnect else { return };
        if Instant::now() >= at {
            if self.profile.as_ref().is_some_and(audio::is_present) {
                self.restart_audio();
            }
            if matches!(self.audio, Audio::Running(_)) {
                self.reconnect = None;
                self.message = None;
            } else {
                self.reconnect = Some(Instant::now() + RECONNECT_EVERY);
            }
        }
        ctx.request_repaint_after(RECONNECT_EVERY);
    }

    fn tempo_map(&self) -> TempoMap {
        TempoMap::new(
            self.rate,
            Tempo::from_f64(self.tempo_bpm),
            TimeSignature::new(self.beats_per_bar, 4),
        )
    }

    fn add_file(&mut self, path: &Path) {
        // In full, so the project can find it from anywhere.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        self.start_import(path, false);
    }

    fn start_import(&mut self, path: PathBuf, reopen: bool) {
        let (tx, rx) = mpsc::channel();
        let (p, rate) = (path.clone(), self.rate);
        std::thread::spawn(move || {
            let _ = tx.send(import::open_at_rate(&p, rate, &cache_dir()));
        });
        self.imports.push(Import {
            path,
            reopen,
            rate,
            done: rx,
        });
    }

    /// Takes in files that have finished opening. Returns true if any did.
    fn finish_imports(&mut self) -> bool {
        let mut changed = false;
        // New tracks are added in the order the files were given, so one
        // waits for any earlier one still converting.
        let mut waiting = false;
        let mut i = 0;
        while i < self.imports.len() {
            let new_track = !self.imports[i].reopen;
            if new_track && waiting {
                i += 1;
                continue;
            }
            let result = match self.imports[i].done.try_recv() {
                Ok(r) => r,
                Err(mpsc::TryRecvError::Empty) => {
                    waiting |= new_track;
                    i += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("the import stopped".into()),
            };
            let im = self.imports.remove(i);
            match result {
                // The device changed rate while this was converting: start
                // again at the new one.
                Ok(_) if im.rate != self.rate => self.start_import(im.path, im.reopen),
                Ok(source) => {
                    changed = true;
                    if im.reopen {
                        // A new device rate, not an edit.
                        self.proj.quiet_next = true;
                        for c in self.tracks.iter_mut().flat_map(|t| &mut t.clips) {
                            if c.path == im.path && c.source.rate != source.rate {
                                c.reopened(source.clone());
                            }
                        }
                    } else {
                        let mut t = Track::new(
                            im.path
                                .file_stem()
                                .map_or("Track".into(), |s| s.to_string_lossy().into_owned()),
                        );
                        t.clips.push(Clip::new(source, im.path, 0, None));
                        self.tracks.push(t);
                    }
                }
                Err(e) => self.message = Some(e),
            }
        }
        changed
    }

    /// Compiles the session and sends it to the engine.
    fn rebuild(&mut self) {
        let s = self.session();
        match s.compile_with(audio::MAX_BLOCK, &mut self.effects.pending) {
            Ok(g) => {
                self.send_graph(g);
            }
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// The session as the engine should play it now.
    fn session(&self) -> Session {
        let mut s = Session::new(self.tempo_map());
        for t in &self.tracks {
            // A file still being converted to a new rate plays nothing
            // until it is ready, but keeps its place.
            let clips = (t.clips.iter())
                .filter(|c| !c.muted && c.source.rate == self.rate)
                .map(|c| ClipSpec {
                    offset: c.offset,
                    len: Some(c.len),
                    ..ClipSpec::new(Frames(c.start), c.source.clone())
                })
                .collect();
            let mut spec = TrackSpec::new(clips);
            spec.mix = t.heard();
            spec.input = t.input;
            spec.arm = t.arm;
            // With direct monitoring the interface plays the input.
            spec.monitor = t.monitor && !self.profile.as_ref().is_some_and(|p| p.direct_monitoring);
            spec.inserts = t.fx.iter().map(FxSlot::spec).collect();
            s.tracks.push(spec);
        }
        s
    }

    /// Renders the mix (and stems, if asked) to WAV in the background.
    fn start_export(&mut self, req: export::Request) -> Result<(), String> {
        let map = self.tempo_map();
        let mut s = self.session();
        // The mix as it stands: no practice mute, no live input.
        for (spec, t) in s.tracks.iter_mut().zip(&self.tracks) {
            spec.mix = t.mix;
            spec.input = None;
            spec.arm = false;
            spec.monitor = false;
        }
        let mut plugins = Vec::new();
        let rate = self.rate;
        let mut graph = |s: &Session, tracks: &[&Track], plugins: &mut Vec<_>| {
            let mut fx = Vec::new();
            for slot in tracks.iter().flat_map(|t| &t.fx) {
                let (p, proc) = self.effects.fresh(slot, rate)?;
                plugins.push(p);
                fx.push((slot.id, proc));
            }
            s.compile_with(audio::MAX_BLOCK, &mut fx)
                .map_err(|e| e.to_string())
        };
        let all: Vec<&Track> = self.tracks.iter().collect();
        let mix = graph(&s, &all, &mut plugins)?;
        let (from, to) = if req.loop_only {
            (
                map.frames_at(map.bar_start(self.loop_bars.0)),
                map.frames_at(map.bar_start(self.loop_bars.1)),
            )
        } else {
            // Room for effects to ring out after the last clip.
            let tail = if self.tracks.iter().any(|t| !t.fx.is_empty()) {
                2 * self.rate as i64
            } else {
                0
            };
            (Frames(0), Frames(mix.end().0 + tail))
        };
        let frames = (to.0 - from.0).max(0) as usize;
        if frames == 0 {
            return Err("There is nothing to export yet.".into());
        }
        let mut parts = vec![export::Part {
            name: "Mix".into(),
            graph: mix,
        }];
        if req.stems {
            for (i, t) in self.tracks.iter().enumerate() {
                let mut one = Session::new(map.clone());
                let mut spec = s.tracks[i].clone();
                spec.mix.mute = false;
                spec.mix.solo = false;
                one.tracks.push(spec);
                parts.push(export::Part {
                    name: format!("{:02} {}", i + 1, export::file_name(&t.name)),
                    graph: graph(&one, &[t], &mut plugins)?,
                });
            }
        }
        self.export.start(&req.folder, parts, plugins, from, frames)
    }

    fn send_graph(&mut self, g: daw_engine::graph::Graph) {
        if self.ctl.set_graph(g).is_err() {
            self.message = Some("The audio engine is busy; try again.".into());
        }
    }

    fn send(&mut self, cmd: Command) {
        if self.ctl.send(cmd).is_err() {
            self.message = Some("The audio engine is busy; try again.".into());
        }
    }

    fn send_loop(&mut self) {
        let map = self.tempo_map();
        let l = self.looping.then(|| {
            (
                map.frames_at(map.bar_start(self.loop_bars.0)),
                map.frames_at(map.bar_start(self.loop_bars.1)),
            )
        });
        self.send(Command::SetLoop(l));
    }

    /// Adds finished takes to their tracks. Returns true if any arrived.
    fn finish_takes(&mut self) -> bool {
        let takes = self.ctl.finished_takes();
        self.add_takes(takes)
    }

    fn add_takes(&mut self, takes: Vec<Take>) -> bool {
        let offset = match (&self.audio, &self.profile) {
            (Audio::Running(r), Some(p)) => r.record_offset(p),
            _ => 0,
        };
        let mut any = false;
        for take in takes {
            if let Some(e) = &take.error {
                self.message = Some(format!("Recording problem: {e}"));
            }
            if take.frames == 0 {
                continue;
            }
            match self.place_take(&take, offset) {
                Ok(()) => any = true,
                Err(e) => self.message = Some(e),
            }
        }
        any
    }

    fn place_take(&mut self, take: &Take, offset: u32) -> Result<(), String> {
        let source = StreamSource::open(&take.path)?;
        let t = self
            .tracks
            .get_mut(take.track)
            .ok_or("a take arrived for a track that is gone")?;
        let start = take.clip_start(Frames(offset as i64)).0;
        timeline::add_take(
            &mut t.clips,
            Clip::new(source, take.path.clone(), start, None),
        );
        Ok(())
    }

    /// The performance check, and its actions.
    fn perf_window(&mut self, ctx: &egui::Context) {
        let status = match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => r.status.clone(),
            Audio::Stopped => return,
        };
        self.perf.update(&status);
        if self.perf.open {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let overloads = status.overloads.load(Ordering::Relaxed);
        let underruns = self.ctl.status().underruns();
        let frames = status.last_frames.load(Ordering::Relaxed);
        let buffer = (frames > 0).then_some((frames, self.rate));
        let effects: Vec<_> = (self.effects.list.plugins().into_iter())
            .filter(|p| p.enabled && !p.class.is_instrument())
            .collect();
        let now = perf::Now {
            overloads,
            underruns,
            buffer,
            tracks: self.tracks.len(),
            rate: self.rate,
        };
        let action = self.perf.window(ctx, now, &effects);
        match action {
            Some(perf::Action::AddTracks(files)) => {
                for f in files {
                    self.add_file(&f);
                }
            }
            Some(perf::Action::AddPlugin(a)) => {
                for i in 0..perf::PLUGINS.min(self.tracks.len()) {
                    match self.effects.create(&a, self.rate) {
                        Ok(slot) => self.tracks[i].fx.push(slot),
                        Err(e) => {
                            self.message = Some(format!("{}: {e}", a.class.name));
                            break;
                        }
                    }
                }
                self.rebuild();
            }
            Some(perf::Action::Reset) => self.perf.reset(overloads, underruns),
            None => {}
        }
    }

    /// Lesson notes: plain text kept with the project (roadmap 1.7).
    fn notes_window(&mut self, ctx: &egui::Context) {
        let mut open = self.notes_open;
        egui::Window::new("Lesson notes")
            .open(&mut open)
            .default_size([380.0, 300.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.notes)
                            .desired_width(f32::INFINITY)
                            .desired_rows(14)
                            .hint_text("What to practise, chord shapes, tab…")
                            .font(egui::TextStyle::Monospace),
                    );
                });
            });
        self.notes_open = open;
    }

    /// Runs `f` on the timeline and the song it edits.
    fn with_song<R>(&mut self, f: impl FnOnce(&mut timeline::Timeline, &mut Song) -> R) -> R {
        let mut song = Song {
            lanes: (self.tracks.iter_mut())
                .map(|t| Lane {
                    name: &t.name,
                    clips: &mut t.clips,
                })
                .collect(),
            markers: &mut self.markers,
        };
        f(&mut self.timeline, &mut song)
    }

    /// The timeline, with its outcome acted on.
    fn timeline(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let view = timeline::View {
            map: &map,
            rate: self.rate,
            playhead: self.ctl.status().position().0,
            playing: self.ctl.status().playing(),
            loop_bars: self.looping.then_some(self.loop_bars),
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
        };
        let out = self.with_song(|tl, song| tl.show(ui, song, &view));
        if out.undo {
            self.undo(ui.ctx());
        }
        if out.redo {
            self.redo(ui.ctx());
        }
        if out.changed {
            self.rebuild();
        }
        if let Some(f) = out.seek {
            self.send(Command::Seek(Frames(f)));
        }
        if let Some(bars) = out.loop_bars {
            self.loop_bars = bars;
            self.looping = true;
            self.send_loop();
        }
    }

    fn toggle_play(&mut self) {
        if self.ctl.status().playing() {
            self.send(Command::Stop);
        } else if self.count_in {
            self.send(Command::CountIn { bars: 1 });
        } else {
            self.send(Command::Play);
        }
    }

    /// The transport's keys, which work anywhere except while typing.
    /// Space always plays or stops; Enter presses the focused control.
    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::S)) {
            self.save_or_ask(ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let key = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
        if key(Key::Space) {
            self.toggle_play();
        }
        if key(Key::Home) {
            self.send(Command::Seek(Frames(0)));
        }
        if key(Key::R) {
            self.recording = !self.recording;
            self.send(Command::SetRecording(self.recording));
        }
        if key(Key::L) {
            self.looping = !self.looping;
            self.send_loop();
        }
        if key(Key::K) {
            self.metronome.enabled = !self.metronome.enabled;
            self.send(Command::SetMetronome(self.metronome));
        }
        if key(Key::C) {
            self.count_in = !self.count_in;
        }
    }

    /// Help → Keyboard shortcuts.
    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Keyboard shortcuts")
            .open(&mut self.shortcuts_open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
                    for (keys, what) in SHORTCUTS {
                        ui.strong(*keys);
                        ui.label(*what);
                        ui.end_row();
                    }
                });
            });
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.ctl.status().playing();
            if ui
                .button(if playing { "⏹ Stop" } else { "▶ Play" })
                .on_hover_text("Space")
                .clicked()
            {
                self.toggle_play();
            }
            if ui
                .button("⏮")
                .accessible_name("Back to the start")
                .on_hover_text("Back to the start (Home)")
                .clicked()
            {
                self.send(Command::Seek(Frames(0)));
            }
            let rec = RichText::new("⏺ Record").color(if self.recording {
                Color32::from_rgb(240, 70, 60)
            } else {
                ui.visuals().text_color()
            });
            if ui
                .selectable_label(self.recording, rec)
                .on_hover_text("Armed tracks record while playing (R)")
                .clicked()
            {
                self.recording = !self.recording;
                self.send(Command::SetRecording(self.recording));
            }
            let map = self.tempo_map();
            let pos = self.ctl.status().position();
            let bb = map.bar_beat(map.ticks_at(pos));
            let label = if self.ctl.status().counting_in() {
                "count-in".to_string()
            } else {
                format!("{bb}   {:.2} s", map.seconds(pos))
            };
            ui.label(RichText::new(label).monospace().size(16.0));
            ui.separator();

            ui.label("Tempo");
            let t = ui
                .add(
                    egui::DragValue::new(&mut self.tempo_bpm)
                        .range(20.0..=300.0)
                        .speed(0.2)
                        .suffix(" BPM"),
                )
                .accessible_name("Tempo");
            ui.label("Beats per bar");
            let b = ui
                .add(egui::DragValue::new(&mut self.beats_per_bar).range(1..=12))
                .accessible_name("Beats per bar");
            if t.changed() || b.changed() {
                self.rebuild();
                self.send_loop();
            }
        });
        ui.horizontal(|ui| {
            let mut m = self.metronome;
            ui.checkbox(&mut m.enabled, "Metronome").on_hover_text("K");
            egui::ComboBox::from_id_salt("subdivision")
                .width(90.0)
                .selected_text(match m.subdivision {
                    1 => "Beats",
                    2 => "Eighths",
                    3 => "Triplets",
                    _ => "Sixteenths",
                })
                .show_ui(ui, |ui| {
                    for (n, label) in [
                        (1, "Beats"),
                        (2, "Eighths"),
                        (3, "Triplets"),
                        (4, "Sixteenths"),
                    ] {
                        ui.selectable_value(&mut m.subdivision, n, label);
                    }
                })
                .response
                .accessible_name("Clicks per beat");
            ui.label("Click level");
            ui.add(egui::Slider::new(&mut m.level_db, -40.0..=0.0).suffix(" dB"))
                .accessible_name("Click level");
            if m != self.metronome {
                self.metronome = m;
                self.send(Command::SetMetronome(m));
            }
            ui.checkbox(&mut self.count_in, "Count-in")
                .on_hover_text("One bar of clicks before playing (C)");
            ui.separator();

            let mut changed = ui
                .checkbox(&mut self.looping, "Loop bars")
                .on_hover_text("L")
                .changed();
            changed |= ui
                .add(egui::DragValue::new(&mut self.loop_bars.0).range(1..=999))
                .accessible_name("Loop from bar")
                .changed();
            ui.label("to");
            changed |= ui
                .add(egui::DragValue::new(&mut self.loop_bars.1).range(2..=1000))
                .accessible_name("Loop to bar")
                .changed();
            if self.loop_bars.1 <= self.loop_bars.0 {
                self.loop_bars.1 = self.loop_bars.0 + 1;
            }
            if changed {
                self.send_loop();
            }
            ui.separator();
            self.trainer.controls(ui);
        });
        ui.horizontal(|ui| self.practice_controls(ui));
    }

    fn practice(&self) -> Command {
        Command::SetPractice {
            speed: self.speed_pct / 100.0,
            semitones: self.semitones as f64 + self.cents as f64 / 100.0,
        }
    }

    /// Speed, transpose and tuning presets.
    fn practice_controls(&mut self, ui: &mut egui::Ui) {
        let before = (self.speed_pct, self.semitones, self.cents);
        ui.label("Speed");
        ui.add(
            egui::DragValue::new(&mut self.speed_pct)
                .range(50.0..=150.0)
                .speed(0.5)
                .fixed_decimals(0)
                .suffix(" %"),
        )
        .accessible_name("Speed")
        .on_hover_text("Slow down or speed up without changing pitch");
        for pct in [50.0, 75.0, 100.0] {
            if ui
                .selectable_label(self.speed_pct == pct, format!("{pct:.0}%"))
                .clicked()
            {
                self.speed_pct = pct;
            }
        }
        ui.separator();
        ui.label("Transpose");
        ui.add(
            egui::DragValue::new(&mut self.semitones)
                .range(-12..=12)
                .suffix(" st"),
        )
        .accessible_name("Transpose, semitones")
        .on_hover_text("Semitones, without changing speed");
        ui.add(
            egui::DragValue::new(&mut self.cents)
                .range(-50..=50)
                .suffix(" ¢"),
        )
        .accessible_name("Fine pitch, cents")
        .on_hover_text("Fine pitch, in cents");
        egui::ComboBox::from_id_salt("tuning-preset")
            .width(200.0)
            .selected_text("Tuning presets")
            .show_ui(ui, |ui| {
                for (label, st) in [
                    ("As recorded", 0),
                    ("Half step down → standard (+1)", 1),
                    ("Whole step down → standard (+2)", 2),
                    ("Standard → half step down (−1)", -1),
                    ("Standard → whole step down (−2)", -2),
                ] {
                    if ui.selectable_label(false, label).clicked() {
                        self.semitones = st;
                        self.cents = 0;
                    }
                }
                ui.separator();
                for t in &self.tracks {
                    // The newest clip that plays.
                    let source = t.clips.iter().rev().find(|c| !c.muted);
                    let Some(src) = source.map(|c| c.source.clone()) else {
                        continue;
                    };
                    if ui
                        .selectable_label(false, format!("Detect tuning of {}", t.name))
                        .on_hover_text(
                            "Measures how far the recording is from concert pitch \
                             (A = the tuner's reference) and suggests a fine-pitch shift. \
                             Whole-step tunings like E♭ read as in tune.",
                        )
                        .clicked()
                    {
                        let (tx, rx) = mpsc::channel();
                        let a4 = self.tuner.a4;
                        std::thread::spawn(move || {
                            // The first minute and a half is plenty.
                            let r = src
                                .read_mono(90 * src.rate as usize)
                                .ok()
                                .and_then(|x| daw_engine::tuner::tuning_offset(&x, src.rate, a4));
                            let _ = tx.send(r);
                        });
                        self.detecting = Some((t.name.clone(), rx));
                        self.detected = None;
                    }
                }
            })
            .response
            .accessible_name("Tuning presets");
        if let Some((name, rx)) = &self.detecting
            && let Ok(r) = rx.try_recv()
        {
            self.detected = Some((name.clone(), r));
            self.detecting = None;
        }
        if self.detecting.is_some() {
            ui.spinner();
        }
        if let Some((name, r)) = self.detected.clone() {
            match r {
                Some(c) if c.abs() >= 2.0 => {
                    ui.label(format!("{name} is {c:+.0} ¢ off"));
                    if ui.button(format!("Shift {:+.0} ¢", -c)).clicked() {
                        self.cents = (-c).round() as i32;
                        self.detected = None;
                    }
                }
                Some(_) => {
                    ui.label(format!("{name} is at concert pitch"));
                }
                None => {
                    ui.label(format!("{name}: no clear pitch to measure"));
                }
            }
            if ui.small_button("✖").accessible_name("Dismiss").clicked() {
                self.detected = None;
            }
        }
        if (self.speed_pct, self.semitones, self.cents) != (100.0, 0, 0)
            && ui.button("Reset").clicked()
        {
            (self.speed_pct, self.semitones, self.cents) = (100.0, 0, 0);
        }
        let loops = self.ctl.status().loops();
        if self.looping && self.ctl.status().playing() {
            self.trainer.update(loops, &mut self.speed_pct);
        }
        if (self.speed_pct, self.semitones, self.cents) != before {
            let c = self.practice();
            self.send(c);
        }
        if self.recording && (self.speed_pct, self.semitones, self.cents) != (100.0, 0, 0) {
            ui.colored_label(
                Color32::YELLOW,
                "Recording waits for normal speed and pitch",
            );
        }
    }

    fn mixer(&mut self, ui: &mut egui::Ui) {
        if self.tracks.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Drop WAV files here to add tracks.").size(18.0));
                ui.add_space(8.0);
                if ui.button("Or add an empty track to record on").clicked() {
                    self.tracks.push(Track::new("Track 1".into()));
                    self.rebuild();
                }
            });
            return;
        }
        let inputs = match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => r.input_channels as usize,
            Audio::Stopped => 0,
        };
        let direct = self.profile.as_ref().is_some_and(|p| p.direct_monitoring);
        let mut changes = Vec::new();
        let mut rebuild = false;
        let mut chosen = None;
        egui::Grid::new("tracks")
            .num_columns(10)
            .spacing(vec2(10.0, 10.0))
            .striped(true)
            .show(ui, |ui| {
                for h in [
                    "Track", "Input", "", "Level", "Volume", "Pan", "", "", "Effects", "Take",
                ] {
                    ui.strong(h);
                }
                ui.end_row();
                for (i, t) in self.tracks.iter_mut().enumerate() {
                    let before = t.heard();
                    let name = t.name.clone();
                    let label = ui.label(&t.name);
                    if let Some(c) = t.clips.iter().find(|c| c.take.is_none()) {
                        label.on_hover_text(c.path.display().to_string());
                    }
                    rebuild |= input_picker(ui, i, &t.name, &mut t.input, inputs);
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(t.input.is_some(), |ui| {
                            let arm = RichText::new("R").color(if t.arm {
                                Color32::from_rgb(240, 70, 60)
                            } else {
                                ui.visuals().text_color()
                            });
                            rebuild |= ui
                                .toggle_value(&mut t.arm, arm)
                                .accessible_name(format!("Arm {name} for recording"))
                                .on_hover_text("Arm for recording (R records)")
                                .changed();
                            let m = ui
                                .add_enabled(!direct, egui::Button::selectable(t.monitor, "M"))
                                .accessible_name(format!("Monitor {name}"));
                            if m.clicked() {
                                t.monitor = !t.monitor;
                                rebuild = true;
                            }
                            m.on_hover_text("Monitor: hear the input through this track")
                                .on_disabled_hover_text(
                                    "Direct monitoring is on (Settings → Audio device)",
                                );
                        });
                    });
                    meter(ui, t.meter);
                    ui.name_inputs(format!("{name} volume"), |ui| {
                        ui.add(
                            egui::Slider::new(&mut t.mix.gain_db, -60.0..=12.0)
                                .suffix(" dB")
                                .step_by(0.1),
                        )
                    });
                    ui.add(egui::Slider::new(&mut t.mix.pan, -1.0..=1.0).show_value(false))
                        .accessible_name(format!("{name} pan"))
                        .on_hover_text("Pan");
                    ui.toggle_value(&mut t.mix.mute, "Mute")
                        .accessible_name(format!("Mute {name}"));
                    ui.horizontal(|ui| {
                        ui.toggle_value(&mut t.mix.solo, "Solo")
                            .accessible_name(format!("Solo {name}"));
                        ui.toggle_value(&mut t.practice_mute, "🎧")
                            .accessible_name(format!("Practice mute {name}"))
                            .on_hover_text("Practice mute: silence this part to play it yourself");
                    });
                    let label = match t.fx.len() {
                        0 => "FX".to_string(),
                        n => format!("FX ({n})"),
                    };
                    let open = self.effects.open_track == Some(i);
                    if ui
                        .selectable_label(open, label)
                        .accessible_name(format!("Effects on {name}"))
                        .on_hover_text(
                            t.fx.iter()
                                .map(|f| f.name.as_str())
                                .collect::<Vec<_>>()
                                .join(" → "),
                        )
                        .clicked()
                    {
                        self.effects.open_track = (!open).then_some(i);
                    }
                    if let Some(k) = take_picker(ui, i, t) {
                        chosen = Some((i, k));
                    }
                    ui.end_row();
                    if t.heard() != before {
                        changes.push((i, t.heard()));
                    }
                }
                ui.label(RichText::new("Master").strong());
                ui.label("");
                ui.label("");
                meter(ui, self.master_meter);
                ui.end_row();
            });
        if ui.button("+ Add track").clicked() {
            self.tracks
                .push(Track::new(format!("Track {}", self.tracks.len() + 1)));
            rebuild = true;
        }
        for (track, mix) in changes {
            self.send(Command::SetMix { track, mix });
        }
        if let Some((i, k)) = chosen {
            timeline::choose_take(&mut self.tracks[i].clips, k);
            rebuild = true;
        }
        if rebuild {
            self.rebuild();
        }
    }

    fn device_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_devices;
        let mut apply = false;
        egui::Window::new("Audio device")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                if self.hosts.iter().all(|h| h.devices.is_empty()) {
                    ui.label("No audio devices were found.");
                }
                let draft = self.draft.get_or_insert_with(|| DeviceProfile {
                    host: self.hosts.first().map_or(String::new(), |h| h.name.clone()),
                    device: String::new(),
                    rate: 48_000,
                    buffer: None,
                    main: (0, 1),
                    click: None,
                    input: None,
                    record_offset: None,
                    direct_monitoring: false,
                });
                egui::Grid::new("device").num_columns(2).show(ui, |ui| {
                    ui.label("Driver");
                    egui::ComboBox::from_id_salt("host")
                        .selected_text(&draft.host)
                        .show_ui(ui, |ui| {
                            for h in &self.hosts {
                                ui.selectable_value(&mut draft.host, h.name.clone(), &h.name);
                            }
                        })
.response
.accessible_name("Driver");
                    ui.end_row();

                    let host = self.hosts.iter().find(|h| h.name == draft.host);
                    ui.label("Device");
                    egui::ComboBox::from_id_salt("device")
                        .selected_text(&draft.device)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for d in host.map(|h| h.devices.as_slice()).unwrap_or(&[]) {
                                if ui
                                    .selectable_value(&mut draft.device, d.name.clone(), &d.name)
                                    .clicked()
                                    && let Some(p) = self.saved.for_device(&draft.host, &d.name)
                                {
                                    // Used before: start from its settings.
                                    *draft = p.clone();
                                }
                            }
                        })
.response
.accessible_name("Device");
                    ui.end_row();

                    let dev = host.and_then(|h| h.devices.iter().find(|d| d.name == draft.device));
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("rate")
                        .selected_text(format!("{} Hz", draft.rate))
                        .show_ui(ui, |ui| {
                            let rates = dev.map(|d| d.rates.clone()).unwrap_or_default();
                            for r in rates {
                                ui.selectable_value(&mut draft.rate, r, format!("{r} Hz"));
                            }
                        })
.response
.accessible_name("Sample rate");
                    ui.end_row();

                    ui.label("Buffer size");
                    egui::ComboBox::from_id_salt("buffer")
                        .selected_text(draft.buffer.map_or("Driver default".into(), |b| {
                            format!("{b} samples ({:.1} ms)", b as f64 * 1000.0 / draft.rate as f64)
                        }))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut draft.buffer, None, "Driver default");
                            for b in [32, 64, 128, 256, 512, 1024, 2048] {
                                let ok = dev
                                    .and_then(|d| d.buffer_range)
                                    .is_none_or(|(lo, hi)| (lo..=hi).contains(&b));
                                if ok {
                                    ui.selectable_value(&mut draft.buffer, Some(b), format!("{b} samples"));
                                }
                            }
                        })
.response
.accessible_name("Buffer size");
                    ui.end_row();

                    let outs = dev.map_or(2, |d| d.output_channels as usize).max(1);
                    ui.label("Main outputs");
                    ui.horizontal(|ui| {
                        channel_picker(ui, "main-l", "Main output, left", &mut draft.main.0, outs);
                        channel_picker(ui, "main-r", "Main output, right", &mut draft.main.1, outs);
                    });
                    ui.end_row();

                    ui.label("Input device");
                    egui::ComboBox::from_id_salt("input")
                        .selected_text(draft.input.as_deref().unwrap_or("None"))
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut draft.input, None, "None");
                            for d in &self.input_devices {
                                ui.selectable_value(&mut draft.input, Some(d.clone()), d);
                            }
                        })
.response
.accessible_name("Input device");
                    ui.end_row();

                    ui.label("Recording offset");
                    ui.horizontal(|ui| {
                        let mut manual = draft.record_offset.is_some();
                        if ui.checkbox(&mut manual, "Measured").changed() {
                            draft.record_offset = manual.then_some(0);
                        }
                        if let Some(o) = &mut draft.record_offset {
                            ui.add(egui::DragValue::new(o).range(0..=48_000).suffix(" frames"));
                            ui.label(format!("{:.2} ms", *o as f64 * 1000.0 / draft.rate as f64));
                        } else {
                            ui.label("from the driver's reported latency");
                        }
                    });
                    ui.end_row();

                    ui.label("Monitoring");
                    ui.checkbox(&mut draft.direct_monitoring, "Direct: I listen through the interface")
                        .on_hover_text("The app never plays inputs back, so you don't hear them twice. Use when the interface's own monitor knob or switch is on.");
                    ui.end_row();

                    ui.label("Metronome output");
                    egui::ComboBox::from_id_salt("click")
                        .selected_text(draft.click.map_or("Main mix".into(), |c| format!("Output {}", c + 1)))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut draft.click, None, "Main mix");
                            for c in 0..outs {
                                ui.selectable_value(&mut draft.click, Some(c), format!("Output {}", c + 1));
                            }
                        })
.response
.accessible_name("Metronome output");
                    ui.end_row();
                });
                ui.separator();
                match &self.audio {
                    Audio::Running(r) => {
                        let s = &r.status;
                        if s.failed.load(Ordering::Relaxed) {
                            ui.colored_label(Color32::LIGHT_RED, "The device stopped with an error.");
                        } else {
                            ui.label(format!(
                                "Running: {} channels at {} Hz, {} samples per callback.",
                                r.channels,
                                r.rate,
                                s.last_frames.load(Ordering::Relaxed)
                            ));
                        }
                        let us = s.output_latency_us.load(Ordering::Relaxed);
                        let ins = s.input_latency_us.load(Ordering::Relaxed);
                        ui.label(format!(
                            "Latency reported by the driver: output {:.1} ms, input {:.1} ms.",
                            us as f64 / 1000.0,
                            ins as f64 / 1000.0
                        ));
                        match (&r.input_error, r.input_channels) {
                            (Some(e), _) => {
                                ui.colored_label(Color32::LIGHT_RED, format!("No input: {e}"));
                            }
                            (None, 0) => {
                                ui.label("No input selected.");
                            }
                            (None, n) => {
                                ui.label(format!("{n} input channels."));
                            }
                        }
                        if let Some(p) = &self.profile {
                            ui.label(format!(
                                "Recordings are moved {} frames earlier to line up.",
                                r.record_offset(p)
                            ));
                        }
                    }
                    Audio::Stopped => {}
                    Audio::Silent(_, why) => {
                        ui.colored_label(
                            Color32::LIGHT_RED,
                            format!("{why}. Playing silently, so the app still works."),
                        );
                    }
                }
                ui.label(
                    RichText::new(
                        "For exact alignment, measure the round trip with a loopback cable and the latency tool, and enter it as the measured recording offset.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        apply = true;
                    }
                    if ui.button("Rescan devices").clicked() {
                        self.hosts = audio::list();
                        self.input_devices = input_names(&draft_host(&self.draft));
                    }
                });
            });
        self.show_devices = open;
        if apply {
            self.reconnect = None;
            self.profile = self.draft.clone();
            if let Some(p) = &self.profile {
                self.saved.use_profile(p.clone());
                if let Err(e) = save_settings(&self.saved) {
                    self.message = Some(format!("Could not save the device settings: {e}"));
                }
            }
            self.restart_audio();
        }
    }
}

/// Settings are kept per user: `%APPDATA%\<name>\settings.txt` on
/// Windows, `~/.config/<name>/settings.txt` elsewhere.
fn settings_path() -> PathBuf {
    config_dir().join("settings.txt")
}

/// Per-user settings folder: `%APPDATA%\<name>` on Windows,
/// `~/.config/<name>` elsewhere.
fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_NAME)
}

fn load_settings() -> SavedProfiles {
    std::fs::read_to_string(settings_path())
        .map(|t| SavedProfiles::from_text(&t))
        .unwrap_or_default()
}

fn save_settings(s: &SavedProfiles) -> std::io::Result<()> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written whole and renamed, so a crash never leaves half a file.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, s.to_text())?;
    std::fs::rename(tmp, path)
}

fn new_engine() -> (Controller, engine::Engine, StreamerThread, RecorderThread) {
    let (mut ctl, eng) = engine::new(audio::MAX_BLOCK);
    let disk = ctl.take_streamer().expect("new controller").spawn();
    let recorder = ctl.take_recorder().expect("new controller").spawn();
    ctl.set_record_dir(takes_dir());
    (ctl, eng, disk, recorder)
}

fn draft_host(p: &Option<DeviceProfile>) -> String {
    p.as_ref().map_or(String::new(), |p| p.host.clone())
}

fn input_names(host: &str) -> Vec<String> {
    audio::input_devices(host)
        .into_iter()
        .map(|d| d.name)
        .collect()
}

/// Chooses a track's input: none, one channel, or a stereo pair.
/// Returns true if it changed.
fn input_picker(
    ui: &mut egui::Ui,
    track: usize,
    track_name: &str,
    input: &mut Option<Input>,
    inputs: usize,
) -> bool {
    let name = |i: &Option<Input>| match i {
        None => "None".to_string(),
        Some(i) if i.stereo => format!("In {}+{}", i.first + 1, i.first + 2),
        Some(i) => format!("In {}", i.first + 1),
    };
    let before = *input;
    egui::ComboBox::from_id_salt(("input", track))
        .width(70.0)
        .selected_text(name(input))
        .show_ui(ui, |ui| {
            ui.selectable_value(input, None, "None");
            for c in 0..inputs {
                let i = Some(Input::mono(c));
                ui.selectable_value(input, i, name(&i));
            }
            for c in (0..inputs.saturating_sub(1)).step_by(2) {
                let i = Some(Input::stereo(c));
                ui.selectable_value(input, i, name(&i));
            }
        })
        .response
        .accessible_name(format!("Input for {track_name}"));
    *input != before
}

/// Chooses which take plays, newest first. Returns the choice if one
/// was made.
fn take_picker(ui: &mut egui::Ui, track: usize, t: &Track) -> Option<Option<usize>> {
    let n = timeline::take_count(&t.clips);
    if n == 0 {
        ui.label("");
        return None;
    }
    let now = timeline::active_take(&t.clips);
    let mut pick = now;
    egui::ComboBox::from_id_salt(("take", track))
        .width(70.0)
        .selected_text(now.map_or("Off".into(), |i| format!("Take {}", i + 1)))
        .show_ui(ui, |ui| {
            for i in (0..n).rev() {
                ui.selectable_value(&mut pick, Some(i), format!("Take {}", i + 1));
            }
            ui.selectable_value(&mut pick, None, "Off");
        })
        .response
        .accessible_name(format!("Take on {}", t.name));
    (pick != now).then_some(pick)
}

fn channel_picker(ui: &mut egui::Ui, id: &str, name: &str, ch: &mut usize, outs: usize) {
    egui::ComboBox::from_id_salt(id)
        .width(60.0)
        .selected_text(format!("{}", *ch + 1))
        .show_ui(ui, |ui| {
            for c in 0..outs {
                ui.selectable_value(ch, c, format!("{}", c + 1));
            }
        })
        .response
        .accessible_name(name);
}

/// A stereo peak meter, −60 dB to 0 dB.
fn meter(ui: &mut egui::Ui, (l, r): (f32, f32)) {
    let (rect, _) = ui.allocate_exact_size(vec2(140.0, 14.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_gray(30));
    for (i, v) in [l, r].into_iter().enumerate() {
        let db = 20.0 * v.max(1e-6).log10();
        let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let h = rect.height() / 2.0;
        let bar = egui::Rect::from_min_size(
            rect.min + vec2(0.0, i as f32 * h),
            vec2(rect.width() * frac, h - 1.0),
        );
        let color = if db > -0.5 {
            Color32::from_rgb(230, 70, 60)
        } else if db > -12.0 {
            Color32::from_rgb(230, 200, 60)
        } else {
            Color32::from_rgb(80, 200, 110)
        };
        p.rect_filled(bar, 1.0, color);
    }
    p.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.ctl.collect_garbage();
        self.watch_device(&ctx);

        // Meters fall back at about 20 dB a second.
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let fall = 10f32.powf(-dt);
        for (i, t) in self.tracks.iter_mut().enumerate() {
            let (mut l, mut r) = self.ctl.status().take_track_peak(i);
            let (il, ir) = self.ctl.status().take_input_peak(i);
            // An armed track shows what it would record.
            if t.arm {
                (l, r) = (l.max(il), r.max(ir));
            }
            t.meter = ((t.meter.0 * fall).max(l), (t.meter.1 * fall).max(r));
        }
        let (l, r) = self.ctl.status().take_master_peak();
        self.master_meter = (
            (self.master_meter.0 * fall).max(l),
            (self.master_meter.1 * fall).max(r),
        );

        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for p in dropped {
            if project::is_project(&p) {
                self.open_dropped(&ctx, p);
            } else if daw_collab::bundle::is_bundle(&p) {
                self.import_bundle(p);
            } else {
                self.add_file(&p);
            }
        }
        let mut rebuild = self.finish_imports() | self.finish_takes();
        for t in &mut self.tracks {
            for slot in &mut t.fx {
                rebuild |= slot.poll();
            }
        }
        if let Some(e) = self.effects.poll_scan() {
            self.message = Some(e);
        }
        if rebuild {
            self.rebuild();
        }
        if self.effects.scanning().is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.tracks.iter().any(|t| t.arm || t.monitor) {
            // Input meters move while stopped.
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if !self.imports.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.keys(&ctx);

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    self.project_menu(ui);
                    ui.separator();
                    self.share_menu(ui);
                    ui.separator();
                    if ui.button("Export…").clicked() {
                        self.export.open = true;
                        ui.close();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui
                        .add_enabled(self.can_undo(), egui::Button::new("Undo"))
                        .on_hover_text("Ctrl+Z")
                        .clicked()
                    {
                        self.undo(&ctx);
                        ui.close();
                    }
                    if ui
                        .add_enabled(self.can_redo(), egui::Button::new("Redo"))
                        .on_hover_text("Ctrl+Shift+Z")
                        .clicked()
                    {
                        self.redo(&ctx);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Split at playhead").clicked() {
                        let at = self.ctl.status().position().0;
                        if self.with_song(|tl, song| tl.split_at(song, at)) {
                            self.rebuild();
                        }
                        ui.close();
                    }
                    if ui.button("Delete selected clips").clicked() {
                        if self.with_song(|tl, song| tl.delete_selected(song)) {
                            self.rebuild();
                        }
                        ui.close();
                    }
                });
                ui.menu_button("Settings", |ui| {
                    if ui.button("Audio device…").clicked() {
                        self.show_devices = true;
                        ui.close();
                    }
                    if ui.button("Plugins…").clicked() {
                        self.effects.show_manager = true;
                        ui.close();
                    }
                    if ui.button("Performance check…").clicked() {
                        self.perf.open = true;
                        ui.close();
                    }
                });
                ui.menu_button("Practice", |ui| {
                    if ui.button("Tuner…").clicked() {
                        self.tuner.open = true;
                        let c = self.tuner.command();
                        self.send(c);
                        ui.close();
                    }
                    if ui.button("Lesson notes…").clicked() {
                        self.notes_open = true;
                        ui.close();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("Keyboard shortcuts…").clicked() {
                        self.shortcuts_open = true;
                        ui.close();
                    }
                });
                ui.separator();
                match &self.audio {
                    Audio::Running(r) => {
                        ui.label(format!(
                            "{} · {} Hz",
                            self.profile.as_ref().map_or("", |p| &p.device),
                            r.rate
                        ));
                        let load = self.perf.load();
                        let text = format!("DSP {:.0}%", load * 100.0);
                        let l = if load > 0.8 {
                            ui.colored_label(Color32::LIGHT_RED, text)
                        } else {
                            ui.label(RichText::new(text).weak())
                        };
                        if l.on_hover_text("Settings → Performance check").clicked() {
                            self.perf.open = true;
                        }
                    }
                    Audio::Silent(..) | Audio::Stopped => {
                        ui.colored_label(Color32::LIGHT_RED, "No audio device: playing silently");
                    }
                }
                if let Some(s) = self.effects.scanning() {
                    ui.spinner();
                    ui.label(s);
                }
                if self.proj.loading() {
                    ui.spinner();
                    ui.label("Opening project…");
                }
                if !self.imports.is_empty() {
                    ui.spinner();
                    ui.label(match self.imports.len() {
                        1 => "Opening 1 file…".to_string(),
                        n => format!("Opening {n} files…"),
                    });
                }
                let lost = self.ctl.status().rec_dropped();
                if lost > 0 {
                    ui.colored_label(
                        Color32::LIGHT_RED,
                        format!("recording fell behind: {lost} frames lost"),
                    );
                }
                let under = self.ctl.status().underruns();
                if under > 0 {
                    ui.colored_label(
                        Color32::YELLOW,
                        format!("disk too slow: {under} frames missed"),
                    );
                }
            });
        });
        egui::Panel::top("transport").show(ui, |ui| self.transport(ui));
        if let Some(m) = self.message.clone() {
            egui::Panel::bottom("message").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::LIGHT_RED, m);
                    if ui.button("OK").clicked() {
                        self.message = None;
                    }
                });
            });
        }
        egui::Panel::bottom("mixer")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.mixer(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| self.timeline(ui));
        self.device_window(&ctx);
        self.effects.manager_window(&ctx);
        let inputs = match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => r.input_channels as usize,
            Audio::Stopped => 0,
        };
        self.export.poll();
        if self.export.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let exports = match &self.proj.folder {
            Some(f) => f.dir.join("Exports"),
            None => user_dir().join(APP_NAME).join("Exports"),
        };
        self.export.set_default_dir(exports);
        let loop_text = format!("bars {} to {}", self.loop_bars.0, self.loop_bars.1);
        if let Some(req) = self.export.window(&ctx, &loop_text)
            && let Err(e) = self.start_export(req)
        {
            self.message = Some(e);
        }
        if let Some(c) = self.tuner.window(&ctx, inputs) {
            self.send(c);
        }
        self.tuner.update(&mut self.ctl, self.rate);
        if self.tuner.open {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if let Some(i) = self.effects.open_track {
            match self.tracks.get_mut(i) {
                Some(t) => {
                    if self.effects.track_window(
                        &ctx,
                        &t.name,
                        &mut t.fx,
                        self.rate,
                        &mut self.message,
                    ) {
                        self.rebuild();
                    }
                }
                None => self.effects.open_track = None,
            }
        }
        self.perf_window(&ctx);
        self.notes_window(&ctx);
        self.shortcuts_window(&ctx);
        self.project_windows(&ctx);
        self.poll_loading(&ctx);
        self.poll_sharing(&ctx);
        self.sync_doc(&ctx, false);
        self.autosave(&ctx);
        self.title(&ctx);
        self.on_close(&ctx);
        if self.ctl.status().playing() {
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if let [flag, bundle] = args.as_slice()
        && flag == daw_plugins::scan::SCAN_ARG
    {
        std::process::exit(daw_plugins::scan::child_main(bundle.as_ref()));
    }
    let files: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1100.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "daw",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(files)))),
    )
}
