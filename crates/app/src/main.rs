// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop application. Phase 1 so far: open audio files, and the
//! sound of videos, as tracks; play them with a mixer, loop, metronome
//! and count-in, choose the audio device, and record: per track input, arm and monitor, with takes kept
//! and switchable; VST 3 effects as track inserts; a tuner and practice
//! mute; export of the mix and stems to WAV; a timeline to edit clips
//! and mark sections; comments pinned to times in the song; level,
//! loudness and spectrum meters.
//!
//! Usage: `daw [file ...]`, or drop audio or video files on the window:
//! WAV, MP3, FLAC, OGG, AAC/M4A, AIFF, or the sound of an MP4, MOV or MKV
//! video. Files in other formats or at another sample rate from the
//! device are converted in the background into a cache folder first.
//!
//! `daw --version` prints the version.
//!
//! `daw --scan-vst3 <bundle>` is the plugin scanner: the app runs itself
//! that way to open each plugin safely in a separate process.

mod commands;
mod comments;
mod comping;
mod crash;
mod export;
mod freeze;
mod fx;
mod groups;
mod latency;
mod meters;
mod mixer;
mod monitor;
mod notes;
mod people;
mod perf;
mod practice;
mod process;
mod project;
mod recording;
mod remote;
mod share;
mod snapshots;
mod start;
mod templates;
mod tempo;
mod theme;
mod timeline;
mod timing;
mod trainer;
mod tuner;
mod update;
mod workspace;

#[cfg(test)]
mod a11y;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use commands::Cmd;
use daw_audio_io::{self as audio, DeviceProfile, HostInfo, Running, SavedProfiles};
use daw_engine::engine::{self, Command, Controller};
use daw_engine::graph::{ClipSpec, Input, Mix, Output, Session, TrackSpec};
use daw_engine::import;
use daw_engine::metronome::Metronome;
use daw_engine::record::{RecorderThread, Take};
use daw_engine::stream::{StreamSource, StreamerThread};
use daw_engine::time::{Frames, TempoMap};
use daw_project::TrackKind;
use eframe::egui::{self, RichText};
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
    /// The track rendered through its effects, as the document has it.
    frozen: Option<daw_project::Frozen>,
    /// Its audio, opened when one of the track's plugins is missing here.
    frozen_audio: Option<Arc<StreamSource>>,
    /// Peak levels shown on the meter, decaying.
    meter: (f32, f32),
    /// Comps made from the track's takes, and the one that plays.
    comps: Vec<daw_project::Comp>,
    comp: Option<String>,
    /// Audio track, bus or folder (see [`daw_project::TrackKind`]).
    kind: TrackKind,
    /// The folder it sits in, by key.
    parent: Option<String>,
    /// The bus its audio goes to, by key: `None` for its folder, or the
    /// master when it has none.
    output: Option<String>,
    sends: Vec<daw_project::Send>,
    /// The control group it belongs to, by key.
    group: Option<String>,
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
            frozen: None,
            frozen_audio: None,
            meter: (0.0, 0.0),
            comps: Vec::new(),
            comp: None,
            kind: TrackKind::Audio,
            parent: None,
            output: None,
            sends: Vec::new(),
            group: None,
        }
    }

    /// Where its audio goes: the bus it is routed to, else its folder,
    /// else (`None`) the master.
    fn goes_to(&self) -> Option<&str> {
        self.output.as_deref().or(self.parent.as_deref())
    }
}

/// A file being opened (and converted, if needed) on a thread of its own.
struct Import {
    path: PathBuf,
    /// Opened again at a new rate for the clips already using it, rather
    /// than added as a track.
    reopen: bool,
    rate: u32,
    /// The file the clip plays (a video's sound is taken out into one of
    /// its own), opened.
    done: mpsc::Receiver<Result<(PathBuf, Arc<StreamSource>), String>>,
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
pub(crate) const APP_NAME: &str = "DAW";

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
    /// Control groups: one fader and mute over several tracks.
    groups: Vec<daw_project::ControlGroup>,
    /// Headphone mixes and the speakers' controls.
    cues: monitor::Cues,
    imports: Vec<Import>,
    tempo_bpm: f64,
    beats_per_bar: u32,
    /// Tempo and time-signature changes, and the start's beat unit.
    time: tempo::Changes,
    tempo_ui: tempo::TempoWindow,
    rate: u32,
    ctl: Controller,
    _disk: StreamerThread,
    _recorder: RecorderThread,
    recording: bool,
    /// Punch range, pre-roll and post-roll, and retrospective capture.
    record: recording::RecordOptions,
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
    meters: meters::Meters,
    export: export::Export,
    freezer: freeze::Freezer,
    processing: process::Processing,
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
    /// Lesson notes and tab blocks, as in the project, and their window.
    notes: String,
    tabs: Vec<daw_project::TabBlock>,
    notes_ui: notes::Notes,
    /// Tracks edited together, as in the project, and their window.
    edit_groups: Vec<daw_project::EditGroup>,
    groups_ui: groups::Groups,
    /// The Fix timing window.
    timing: timing::Timing,
    takes_ui: comping::TakesPane,
    /// Pinned comments, as in the project, and their panel.
    comments: Vec<daw_project::Comment>,
    comment_ui: comments::Comments,
    /// The snapshots window.
    snapshot_ui: snapshots::Snapshots,
    /// The New from template and Save as template windows.
    template_ui: templates::Templates,
    /// The start screen, and the recent projects it lists.
    start: start::Start,
    /// Named commands, their keys, and their windows.
    commands: commands::Commands,
    proj: project::Proj,
    latency: latency::Latency,
    updates: update::Updates,
    crashes: crash::Crashes,
    /// This install's identity, and the people windows.
    me: daw_project::Identity,
    people: people::People,
    /// The phone remote: its window and the phones connected.
    remote: remote::Remote,
    /// The panes, and the layouts saved on this computer.
    workspace: workspace::Workspace,
    /// The panes are being drawn, so the workspace is out of `self`.
    drawing_panes: bool,
    /// Workspace commands that came while it was out.
    workspace_cmds: Vec<Cmd>,
    /// The look, chosen per computer.
    themes: theme::Themes,
}

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
            groups: Vec::new(),
            cues: monitor::Cues::default(),
            imports: Vec::new(),
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            time: tempo::Changes::default(),
            tempo_ui: tempo::TempoWindow::default(),
            rate,
            ctl,
            _disk: disk,
            _recorder: recorder,
            recording: false,
            record: recording::RecordOptions::default(),
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
            meters: meters::Meters::new(rate),
            export: export::Export::default(),
            freezer: freeze::Freezer::default(),
            processing: process::Processing::default(),
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
            tabs: Vec::new(),
            notes_ui: notes::Notes::default(),
            edit_groups: Vec::new(),
            groups_ui: groups::Groups::default(),
            timing: timing::Timing::default(),
            takes_ui: comping::TakesPane::default(),
            comments: Vec::new(),
            comment_ui: comments::Comments::default(),
            snapshot_ui: snapshots::Snapshots::default(),
            template_ui: templates::Templates::default(),
            start: start::Start::new((!cfg!(test)).then(|| config_dir().join("start.txt"))),
            commands: commands::Commands::new((!cfg!(test)).then(|| config_dir().join("keys.txt"))),
            proj: project::Proj::new(),
            latency: latency::Latency::default(),
            updates: update::Updates::new((!cfg!(test)).then(|| config_dir().join("updates.txt"))),
            crashes: crash::Crashes::new(if cfg!(test) {
                std::env::temp_dir().join("daw-test-no-crash-reports")
            } else {
                crash::reports_dir()
            }),
            me: people::load_identity(),
            people: people::People::default(),
            remote: remote::Remote::load(),
            workspace: workspace::Workspace::new(
                (!cfg!(test)).then(|| config_dir().join("layout.txt")),
            ),
            drawing_panes: false,
            workspace_cmds: Vec::new(),
            themes: theme::Themes::new((!cfg!(test)).then(config_dir).as_deref()),
        };
        if !cfg!(test) {
            app.effects.start_scan();
        }
        app.look_for_recovery();
        app.audio = app.open_device(eng);
        app.send_capture();
        app.sign_in();
        if people::ask_name_at_start(&app.me) {
            app.ask_name();
        }
        // The empty project as it starts is not an edit.
        let start = app.project();
        app.proj.doc.set_quietly(&start);
        app.apply(start);
        // Nothing to open, so the start screen offers what to do (unless
        // turned off, or a project to recover is offered first).
        if files.is_empty() && app.proj.found.is_none() && app.start.settings.show && !cfg!(test) {
            app.show_start();
        }
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
        self.send(self.meters.command());
        self.send(self.practice());
        self.send_loop();
        self.send(Command::Seek(pos));
        self.send_capture();
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
        tempo::map(self.rate, self.tempo_bpm, self.beats_per_bar, &self.time)
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
            let open = || {
                let p = if reopen {
                    p
                } else {
                    import::soundtrack(&p, &cache_dir())?
                };
                let s = import::open_at_rate(&p, rate, &cache_dir())?;
                Ok((p, s))
            };
            let _ = tx.send(open());
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
                Ok((path, _)) if im.rate != self.rate => self.start_import(path, im.reopen),
                Ok((path, source)) => {
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
                            path.file_stem()
                                .map_or("Track".into(), |s| s.to_string_lossy().into_owned()),
                        );
                        t.clips.push(Clip::new(source, path, 0, None));
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
        self.send_monitor();
    }

    /// The session as the engine should play it now.
    fn session(&self) -> Session {
        let mut s = Session::new(self.tempo_map());
        s.groups = (self.groups.iter())
            .map(|g| daw_engine::graph::GroupSpec {
                gain_db: g.gain_db as f32,
                mute: g.mute,
            })
            .collect();
        for t in &self.tracks {
            if let Some(clip) = self.frozen_playing(t) {
                // Its clips and effects as rendered where it has every
                // plugin.
                let mut spec = TrackSpec::new(vec![clip]);
                spec.mix = t.heard();
                spec.input = t.input;
                spec.arm = t.arm;
                spec.monitor =
                    t.monitor && !self.profile.as_ref().is_some_and(|p| p.direct_monitoring);
                // Its plugins that are here stay in the graph, bypassed,
                // for when the frozen audio stops fitting.
                spec.inserts = (t.fx.iter())
                    .map(|f| daw_engine::graph::InsertSpec {
                        bypass: true,
                        ..f.spec()
                    })
                    .collect();
                self.route(t, &mut spec);
                s.tracks.push(spec);
                continue;
            }
            // A file still being converted to a new rate plays nothing
            // until it is ready, but keeps its place.
            let clips = (t.clips.iter())
                .filter(|c| !c.muted && c.source.rate == self.rate)
                .map(|c| ClipSpec {
                    offset: c.offset,
                    len: Some(c.len),
                    gain_db: c.gain_db,
                    fade_in: c.fade_in,
                    fade_out: c.fade_out,
                    envelope: c.envelope.as_slice().into(),
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
            self.route(t, &mut spec);
            s.tracks.push(spec);
        }
        s
    }

    /// The index of the track with this key.
    fn track_index(&self, key: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.key == key)
    }

    /// A bus the track can be routed to: a bus or folder that is there.
    fn bus_index(&self, key: &str) -> Option<usize> {
        self.track_index(key)
            .filter(|&i| self.tracks[i].kind.is_bus())
    }

    /// Fills in where `t`'s audio goes, by index: its output, sends,
    /// control group and effects' sidechains. Anything naming a track
    /// that isn't there goes to the master, or is left out.
    fn route(&self, t: &Track, spec: &mut TrackSpec) {
        spec.output = match t.goes_to().and_then(|k| self.bus_index(k)) {
            Some(i) if self.tracks[i].key != t.key => Output::Track(i),
            _ => Output::Master,
        };
        spec.sends = (self.sends_of(t).iter())
            .map(|(s, to)| daw_engine::graph::SendSpec {
                to: *to,
                gain_db: s.gain_db as f32,
                pre_fader: s.pre_fader,
                mute: s.mute,
            })
            .collect();
        spec.group = (t.group.as_deref()).and_then(|g| self.groups.iter().position(|x| x.id == g));
        for (slot, spec) in t.fx.iter().zip(&mut spec.inserts) {
            spec.sidechain = (slot.sidechain.as_deref())
                .and_then(|k| self.track_index(k))
                .filter(|&i| self.tracks[i].key != t.key);
        }
        spec.cues = self.cue_sends(&t.key);
    }

    /// The track's sends that go somewhere, with the bus's index: the
    /// order the engine has them in.
    fn sends_of<'a>(&self, t: &'a Track) -> Vec<(&'a daw_project::Send, usize)> {
        (t.sends.iter())
            .filter_map(|s| Some((s, self.bus_index(&s.to)?)))
            .filter(|(_, i)| self.tracks[*i].key != t.key)
            .collect()
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
                plugins.extend(p);
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
                // On its own: no bus, sends, group or sidechains.
                mixer::alone(&mut spec);
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
        let mut keys = Vec::new();
        for take in takes {
            if let Some(e) = &take.error {
                self.message = Some(format!("Recording problem: {e}"));
            }
            if take.frames == 0 {
                continue;
            }
            match self.place_take(&take, offset) {
                Ok(k) => {
                    any = true;
                    keys.push(k);
                }
                Err(e) => self.message = Some(e),
            }
        }
        // For "undo last take" from a phone.
        self.remote.note_takes(keys);
        any
    }

    /// Returns the track's and the new clip's keys.
    fn place_take(&mut self, take: &Take, offset: u32) -> Result<(String, String), String> {
        let source = StreamSource::open(&take.path)?;
        let t = self
            .tracks
            .get_mut(take.track)
            .ok_or("a take arrived for a track that is gone")?;
        let start = take.clip_start(Frames(offset as i64)).0;
        let clip = Clip::new(source, take.path.clone(), start, None);
        let keys = (t.key.clone(), clip.key.clone());
        t.takes(self.rate).add_take(clip);
        Ok(keys)
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
        if frames > 0 {
            // Helper processes for new effects run one such block behind.
            self.effects.block = frames as usize;
        }
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

    /// Lesson notes and tab blocks (roadmap 1.7).
    fn notes_pane(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let end_bar = self.with_song(|_, song| timeline::Timeline::end_bar(&map, song));
        let playhead = self.ctl.status().position();
        let me = self.my_author_id();
        let cx = notes::Ctx {
            markers: &self.markers,
            end_bar,
            playhead_bar: map.bar_beat(map.ticks_at(playhead)).bar,
            me: &me,
            may_edit: self.may_edit_song(),
        };
        let out = self.notes_ui.pane(ui, &mut self.notes, &mut self.tabs, &cx);
        if let Some(bar) = out.go_to_bar {
            let f = map.frames_at(map.bar_start(bar));
            self.send(Command::Seek(f));
        }
        if let Some(bars) = out.loop_bars {
            self.loop_bars = bars;
            self.looping = true;
            self.send_loop();
        }
    }

    /// Whether this user may change the song, not only comment on it.
    fn may_edit_song(&self) -> bool {
        self.proj.doc.my_role() != Some(daw_project::Role::Commenter)
    }

    /// Who this user is, for the comments they write: this computer's id
    /// until projects have members, when it becomes the member's key.
    pub(crate) fn my_author_id(&self) -> String {
        if cfg!(test) {
            "me".into()
        } else {
            self.proj.doc.my_key().unwrap_or_default().to_string()
        }
    }

    /// The comments panel, at the side.
    fn comments_panel(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let tracks: Vec<(String, String)> = (self.tracks.iter())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let at = |bar: i64| map.frames_at(map.bar_start(bar)).0;
        let me = self.my_author_id();
        let members = self.proj.doc.members();
        // Commenters, and people not let in yet, change only their own.
        let any = match self.proj.doc.my_role() {
            Some(r) => r != daw_project::Role::Commenter,
            None => members.is_empty(),
        };
        let cx = comments::Ctx {
            map: &map,
            playhead: self.ctl.status().position().0,
            loop_frames: self
                .looping
                .then(|| (at(self.loop_bars.0), at(self.loop_bars.1))),
            tracks: &tracks,
            me: &me,
            members: &members,
            may_change_any: any,
        };
        let out = self.comment_ui.panel(ui, &mut self.comments, &cx);
        if let Some(f) = out.play_from {
            self.send(Command::Seek(Frames(f)));
            if !self.ctl.status().playing() {
                self.toggle_play();
            }
        }
    }

    /// Runs `f` on the timeline and the song it edits.
    fn with_song<R>(&mut self, f: impl FnOnce(&mut timeline::Timeline, &mut Song) -> R) -> R {
        let mut song = Song {
            lanes: (self.tracks.iter_mut())
                .map(|t| Lane {
                    name: &t.name,
                    clips: &mut t.clips,
                    kind: t.kind,
                    in_folder: t.parent.is_some(),
                })
                .collect(),
            markers: &mut self.markers,
        };
        f(&mut self.timeline, &mut song)
    }

    /// The timeline, with its outcome acted on.
    fn timeline(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let ids: Vec<String> = self.tracks.iter().map(|t| t.key.clone()).collect();
        let pins = self.comment_ui.pins(&self.comments, &ids);
        let commands = std::mem::take(&mut self.commands.timeline);
        let view = timeline::View {
            map: &map,
            rate: self.rate,
            playhead: self.ctl.status().position().0,
            playing: self.ctl.status().playing(),
            loop_bars: self.looping.then_some(self.loop_bars),
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            pins: &pins,
            commands: &commands,
        };
        self.timeline.groups = self.lane_groups();
        let out = self.with_song(|tl, song| tl.show(ui, song, &view));
        if let Some(id) = out.pin {
            self.comment_ui.show_thread(&id);
        }
        if let Some(marker) = out.tab_for {
            if self.may_edit_song() {
                let me = self.my_author_id();
                self.notes_ui.add_tab(&mut self.tabs, Some(marker), &me);
            } else {
                self.notes_ui.open = true;
            }
        }
        if let Some((lane, at)) = out.comment_at {
            self.comment_ui.write_at(comments::Target {
                track: lane.and_then(|l| self.tracks.get(l)).map(|t| t.key.clone()),
                start: at,
                end: None,
            });
        }
        if let Some(how) = out.process {
            self.processing
                .start(&self.tracks, &self.timeline.selected, how);
        }
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
        if let Some(ask) = out.tempo {
            self.ask_tempo(ask);
        }
    }

    fn toggle_play(&mut self) {
        if self.ctl.status().playing() {
            self.send(Command::Stop);
            return;
        }
        self.prepare_play();
        if self.count_in {
            self.send(Command::CountIn { bars: 1 });
        } else {
            self.send(Command::Play);
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.ctl.status().playing();
            if ui
                .button(if playing { "⏹ Stop" } else { "▶ Play" })
                .on_hover_text(self.commands.hover(Cmd::PlayStop))
                .clicked()
            {
                self.toggle_play();
            }
            if ui
                .button("⏮")
                .accessible_name("Back to the start")
                .on_hover_text(self.commands.hover(Cmd::ToStart))
                .clicked()
            {
                self.send(Command::Seek(Frames(0)));
            }
            let rec = RichText::new("⏺ Record").color(if self.recording {
                crate::theme::look(ui.ctx()).c().record
            } else {
                ui.visuals().text_color()
            });
            if ui
                .selectable_label(self.recording, rec)
                .on_hover_text(self.commands.hover(Cmd::Record))
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
            let font = theme::look(ui.ctx()).numerals_font();
            ui.label(RichText::new(label).font(font));
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
            ui.label("Time signature");
            let b = ui
                .add(egui::DragValue::new(&mut self.beats_per_bar).range(1..=12))
                .accessible_name("Beats per bar");
            ui.label("/");
            let u = tempo::unit_picker(ui, &mut self.time.beat_unit);
            if t.changed() || b.changed() || u {
                self.time_changed();
            }
            if ui
                .button("Changes…")
                .on_hover_text("Tempo and time signature changes later in the song")
                .clicked()
            {
                self.tempo_ui.open = !self.tempo_ui.open;
            }
        });
        ui.horizontal(|ui| {
            let mut m = self.metronome;
            ui.checkbox(&mut m.enabled, "Metronome")
                .on_hover_text(self.commands.hover(Cmd::Metronome));
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
                .on_hover_text(self.commands.hover(Cmd::CountIn));
            ui.separator();

            let mut changed = ui
                .checkbox(&mut self.looping, "Loop bars")
                .on_hover_text(self.commands.hover(Cmd::Loop))
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
        ui.horizontal(|ui| self.recording_controls(ui));
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
                ui.visuals().warn_fg_color,
                "Recording waits for normal speed and pitch",
            );
        }
    }

    /// The device's input channels.
    fn input_count(&self) -> usize {
        match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => r.input_channels as usize,
            Audio::Stopped => 0,
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
                    measured_at: None,
                    direct_monitoring: false,
                    monitor: daw_audio_io::MonitorOutputs::default(),
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
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let mut manual = draft.record_offset.is_some();
                            if ui.checkbox(&mut manual, "Measured").changed() {
                                draft.record_offset = manual.then_some(0);
                                draft.measured_at = None;
                            }
                            if let Some(o) = &mut draft.record_offset {
                                if ui
                                    .add(egui::DragValue::new(o).range(0..=48_000).suffix(" frames"))
                                    .changed()
                                {
                                    draft.measured_at = None;
                                }
                                ui.label(format!("{:.2} ms", *o as f64 * 1000.0 / draft.rate as f64));
                            } else {
                                ui.label("from the driver's reported latency");
                            }
                            if ui
                                .button("Measure…")
                                .on_hover_text("Measure the round trip with a loopback cable")
                                .clicked()
                            {
                                self.latency.open = true;
                                self.latency.inputs = input_channels(&draft.host, draft.input.as_deref());
                            }
                        });
                        if let Some((rate, buffer)) = draft.measured_at
                            && (rate, buffer) != (draft.rate, draft.buffer)
                        {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!(
                                    "Measured at {rate} Hz, {}: measure again for these settings.",
                                    buffer.map_or("driver default buffer".into(), |b| format!("{b} samples"))
                                ),
                            );
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
                            ui.colored_label(ui.visuals().error_fg_color, "The device stopped with an error.");
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
                                ui.colored_label(ui.visuals().error_fg_color, format!("No input: {e}"));
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
                            ui.visuals().error_fg_color,
                            format!("{why}. Playing silently, so the app still works."),
                        );
                    }
                }
                ui.label(
                    RichText::new(
                        "For exact alignment, measure the round trip with a loopback cable: Recording offset → Measure….",
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
            self.apply_draft();
        }
    }

    /// Uses the settings in the device window, saves them and restarts
    /// the audio.
    fn apply_draft(&mut self) {
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

    /// What to measure latency on: the device in the device window, or,
    /// with `DAW_TEST_LOOPBACK` set, the silent stand-in's simulated
    /// cable.
    fn measure_target(&self) -> Result<audio::latency::Target, String> {
        let draft = self.draft.as_ref().ok_or("Choose an audio device first.")?;
        match audio::SilentInput::from_env() {
            input @ audio::SilentInput::Loopback { .. } => Ok(audio::latency::Target::Silent {
                rate: draft.rate,
                buffer: draft.buffer.unwrap_or(256),
                input,
            }),
            _ if draft.device.is_empty() => Err("Choose an audio device first.".into()),
            _ => Ok(audio::latency::Target::Device(draft.clone())),
        }
    }

    /// Stops the app's audio and starts measuring on `target`.
    fn start_measuring(&mut self, target: Result<audio::latency::Target, String>) {
        let target = match target {
            Ok(t) => t,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        self.reconnect = None;
        // Frees the device for the measurement.
        self.audio = Audio::Stopped;
        if !self.latency.begin(target) {
            self.restart_audio();
        }
    }

    fn latency_window(&mut self, ctx: &egui::Context) {
        if let Some(done) = self.latency.poll() {
            match (done, &mut self.draft) {
                (Ok(m), Some(d)) => {
                    d.record_offset = Some(m.frames);
                    d.measured_at = Some((d.rate, d.buffer));
                    self.apply_draft();
                }
                _ => self.restart_audio(),
            }
        }
        if self.latency.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if !self.latency.open {
            return;
        }
        let Some(draft) = self.draft.clone() else {
            return;
        };
        match self.latency.window(ctx, &draft, self.recording) {
            Some(latency::Action::Start) => {
                let target = self.measure_target();
                self.start_measuring(target);
            }
            Some(latency::Action::Cancel) => {
                self.latency.cancel();
                self.restart_audio();
            }
            None => {}
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
pub(crate) fn config_dir() -> PathBuf {
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
    // Tests never touch the user's settings.
    if cfg!(test) {
        return Ok(());
    }
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

/// Channels on the input device `name`, for choosing one; 2 if it can't
/// be found (so the simulated cable has some).
fn input_channels(host: &str, name: Option<&str>) -> usize {
    audio::input_devices(host)
        .into_iter()
        .find(|d| Some(d.name.as_str()) == name)
        .map_or(2, |d| d.input_channels as usize)
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
    let shown = match t.comp_name() {
        Some(comp) => comp.to_string(),
        None => now.map_or("Off".into(), |i| format!("Take {}", i + 1)),
    };
    egui::ComboBox::from_id_salt(("take", track))
        .width(70.0)
        .selected_text(shown)
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.themes.apply(&ctx);
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
        rebuild |= self.processing.finish(&mut self.tracks, &mut self.message);
        for t in &mut self.tracks {
            for slot in &mut t.fx {
                let was_fine = slot.crashed.is_none();
                rebuild |= slot.poll();
                if was_fine && let Some(why) = &slot.crashed {
                    self.message = Some(format!(
                        "{}: {why}. Its audio passes through; open the track's effects to \
                         start it again.",
                        slot.name
                    ));
                }
                // Its buses changed: made again, with its settings.
                if slot.needs_reload {
                    slot.needs_reload = false;
                    match self.effects.reload(slot, self.rate) {
                        Ok(()) => rebuild = true,
                        Err(e) => self.message = Some(format!("{}: {e}", slot.name)),
                    }
                }
            }
        }
        let slots = self.tracks.iter_mut().flat_map(|t| t.fx.iter_mut());
        if let Some(m) = self.effects.cap_editors(slots) {
            self.message = Some(m);
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
        if !self.imports.is_empty() || self.processing.busy() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.command_keys(&ctx);

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    for c in [
                        Cmd::NewProject,
                        Cmd::NewFromTemplate,
                        Cmd::OpenProject,
                        Cmd::StartScreen,
                        Cmd::Save,
                        Cmd::SaveAs,
                        Cmd::SaveAsTemplate,
                    ] {
                        self.menu_item(ui, c);
                    }
                    ui.separator();
                    for c in [
                        Cmd::ExportBundle,
                        Cmd::ImportBundle,
                        Cmd::Snapshots,
                        Cmd::People,
                    ] {
                        self.menu_item(ui, c);
                    }
                    ui.separator();
                    self.menu_item(ui, Cmd::Export);
                });
                ui.menu_button("Edit", |ui| {
                    self.menu_item(ui, Cmd::Undo);
                    self.menu_item(ui, Cmd::Redo);
                    ui.separator();
                    for c in [Cmd::Split, Cmd::Delete, Cmd::Marker, Cmd::ClearSelection] {
                        self.menu_item(ui, c);
                    }
                    ui.separator();
                    for c in [Cmd::Crossfade, Cmd::ClipFades] {
                        self.menu_item(ui, c);
                    }
                    ui.separator();
                    for c in [Cmd::EditGroups, Cmd::FixTiming] {
                        self.menu_item(ui, c);
                    }
                });
                ui.menu_button("View", |ui| {
                    for c in [Cmd::ZoomIn, Cmd::ZoomOut, Cmd::ZoomSelection] {
                        self.menu_item(ui, c);
                    }
                    ui.separator();
                    for c in [Cmd::Snap, Cmd::Ripple, Cmd::VolumeLines, Cmd::GroupEdit] {
                        self.menu_item(ui, c);
                    }
                });
                ui.menu_button("Transport", |ui| {
                    for c in [
                        Cmd::PlayStop,
                        Cmd::ToStart,
                        Cmd::Record,
                        Cmd::Loop,
                        Cmd::Metronome,
                        Cmd::CountIn,
                    ] {
                        self.menu_item(ui, c);
                    }
                    ui.separator();
                    for c in [Cmd::Talkback, Cmd::Dim, Cmd::Mono] {
                        self.menu_item(ui, c);
                    }
                });
                ui.menu_button("View", |ui| {
                    self.view_menu(ui);
                });
                ui.menu_button("Settings", |ui| {
                    for c in [
                        Cmd::AudioDevice,
                        Cmd::Plugins,
                        Cmd::PerformanceCheck,
                        Cmd::KeyCommands,
                        Cmd::Theme,
                    ] {
                        self.menu_item(ui, c);
                    }
                    if ui
                        .button("Phone remote…")
                        .on_hover_text("Record and run the transport from your phone")
                        .clicked()
                    {
                        self.remote.open = true;
                        ui.close();
                    }
                });
                ui.menu_button("Practice", |ui| {
                    for c in [Cmd::Tuner, Cmd::Meters, Cmd::LessonNotes, Cmd::Comments] {
                        self.menu_item(ui, c);
                    }
                });
                ui.menu_button("Help", |ui| {
                    self.menu_item(ui, Cmd::FindCommand);
                    self.menu_item(ui, Cmd::KeyCommands);
                    ui.separator();
                    self.menu_item(ui, Cmd::CheckUpdates);
                    self.menu_item(ui, Cmd::CrashReports);
                    self.menu_item(ui, Cmd::SoundCredits);
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
                            ui.colored_label(ui.visuals().error_fg_color, text)
                        } else {
                            ui.label(RichText::new(text).weak())
                        };
                        if l.on_hover_text("Settings → Performance check").clicked() {
                            self.perf.open = true;
                        }
                    }
                    Audio::Silent(..) | Audio::Stopped => {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            "No audio device: playing silently",
                        );
                    }
                }
                if let Some(s) = self.effects.scanning() {
                    ui.spinner();
                    ui.label(s);
                }
                if self.proj.loading() {
                    ui.spinner();
                    ui.label(match self.proj.progress.get() {
                        (_, 0) => "Opening project…".to_string(),
                        (done, total) => format!("Opening project: {done} of {total} files…"),
                    });
                }
                if !self.imports.is_empty() {
                    ui.spinner();
                    ui.label(match self.imports.len() {
                        1 => "Opening 1 file…".to_string(),
                        n => format!("Opening {n} files…"),
                    });
                }
                if self.processing.busy() > 0 {
                    ui.spinner();
                    ui.label(match self.processing.busy() {
                        1 => "Processing 1 clip…".to_string(),
                        n => format!("Processing {n} clips…"),
                    });
                }
                let lost = self.ctl.status().rec_dropped();
                if lost > 0 {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("recording fell behind: {lost} frames lost"),
                    );
                }
                let under = self.ctl.status().underruns();
                if under > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("disk too slow: {under} frames missed"),
                    );
                }
            });
        });
        egui::Panel::top("transport").show(ui, |ui| self.transport(ui));
        if let Some(m) = self.message.clone() {
            egui::Panel::bottom("message").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, m);
                    if ui.button("OK").clicked() {
                        self.message = None;
                    }
                });
            });
        }
        if self.crashes.new > 0 {
            egui::Panel::bottom("crash note").show(ui, |ui| self.crashes.note(ui));
        }
        if self.updates.newer().is_some() {
            egui::Panel::bottom("update note").show(ui, |ui| self.updates.note(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(2.0))
            .show(ui, |ui| self.panes(ui));
        self.device_window(&ctx);
        self.latency_window(&ctx);
        self.effects.manager_window(&ctx);
        self.effects.credits_window(&ctx);
        self.poll_freezing(&ctx);
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
        self.tuner.update(&mut self.ctl, self.rate);
        if self.tuner.open {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        let names: Vec<_> = (self.tracks.iter())
            .map(|t| (t.key.as_str(), t.name.as_str()))
            .collect();
        if let Some(c) = self.meters.window(&ctx, &names) {
            self.send(c);
        }
        let playing = self.ctl.status().playing();
        self.meters.update(&mut self.ctl, self.rate, playing);
        if self.meters.open {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if let Some(i) = self.effects.open_track {
            let note = self.tracks.get(i).map(|t| self.frozen_note(t));
            let others: Vec<(String, String)> = (self.tracks.iter().enumerate())
                .filter(|(j, _)| *j != i)
                .map(|(_, t)| (t.key.clone(), t.name.clone()))
                .collect();
            match (self.tracks.get_mut(i), note) {
                (Some(t), Some((note, missing))) => {
                    if self.effects.track_window(
                        &ctx,
                        &t.name,
                        &mut t.fx,
                        &missing,
                        note.as_deref(),
                        self.rate,
                        &others,
                        &mut self.message,
                    ) {
                        self.rebuild();
                    }
                }
                _ => self.effects.open_track = None,
            }
        }
        self.perf_window(&ctx);
        self.layout_windows(&ctx);
        self.themes.window(&ctx);
        self.command_windows(&ctx);
        self.updates.poll(&ctx);
        self.updates.window(&ctx);
        self.updates.ask_window(&ctx);
        self.crashes.window(&ctx);
        self.start_window(&ctx);
        self.project_windows(&ctx);
        self.people_windows(&ctx);
        self.snapshots_window(&ctx);
        self.tempo_window(&ctx);
        self.groups_window(&ctx);
        self.timing_window(&ctx);
        self.remote_tick(&ctx);
        self.template_windows(&ctx);
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

/// `--check-plugins`: scans the standard folders, checks each plugin
/// found and writes the report to `out`. Returns the exit code.
fn check_plugins(out: &std::path::Path) -> i32 {
    let Ok(exe) = std::env::current_exe() else {
        return 1;
    };
    let mut list = daw_plugins::scan::PluginList::default();
    list.rescan(
        &daw_plugins::scan::default_dirs(),
        |b| daw_plugins::scan::scan_in_child(&exe, b, daw_plugins::scan::SCAN_TIMEOUT),
        |b| eprintln!("scanning {}", b.display()),
    );
    let reports: Vec<_> = (list.plugins().iter())
        .map(|p| {
            eprintln!("checking {}", p.class.name);
            daw_plugins::check::check(&exe, &p.bundle, &p.class)
        })
        .collect();
    let mut text = fx::check_report(&reports);
    for (bundle, why) in list.failures() {
        text.push_str(&format!(
            "\nCould not be loaded: {} ({why})",
            bundle.display()
        ));
    }
    match std::fs::write(out, text) {
        Ok(()) => {
            eprintln!("report written to {}", out.display());
            0
        }
        Err(e) => {
            eprintln!("could not write {}: {e}", out.display());
            1
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
    // Plugins run in a copy of the app (roadmap 2.4), so a crash in one
    // ends that copy, not this one.
    if let [flag] = args.as_slice()
        && flag == daw_plugins::host::HOST_ARG
    {
        std::process::exit(daw_plugins::host::helper_main());
    }
    // Checks every installed plugin for the compatibility list and
    // writes the report (to the file named, or plugin-check.md here).
    if let [flag, rest @ ..] = args.as_slice()
        && flag == "--check-plugins"
        && rest.len() <= 1
    {
        let out = rest
            .first()
            .map_or("plugin-check.md".into(), std::path::PathBuf::from);
        std::process::exit(check_plugins(&out));
    }
    if let [flag] = args.as_slice()
        && flag == "--version"
    {
        println!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    crash::install(crash::reports_dir());
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
