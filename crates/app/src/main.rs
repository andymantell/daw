// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop application. Phase 1 so far: open WAV files as tracks,
//! play them with a mixer, loop, metronome and count-in, and choose the
//! audio device.
//!
//! Usage: `daw [file.wav ...]`, or drop WAV files on the window. Files at
//! another sample rate from the device are converted in the background
//! into a cache folder first.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daw_audio_io::{self as audio, DeviceProfile, HostInfo, Running};
use daw_engine::engine::{self, Command, Controller};
use daw_engine::graph::{ClipSpec, Mix, Session, TrackSpec};
use daw_engine::import;
use daw_engine::metronome::Metronome;
use daw_engine::stream::{StreamSource, StreamerThread};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};
use eframe::egui::{self, Color32, Key, RichText, Sense, Stroke, vec2};

struct Track {
    name: String,
    /// The file as imported; `source` may be a converted copy of it.
    original: PathBuf,
    source: Arc<StreamSource>,
    mix: Mix,
    /// Peak levels shown on the meter, decaying.
    meter: (f32, f32),
}

/// A file being opened (and converted, if needed) on a thread of its own.
struct Import {
    path: PathBuf,
    /// Replaces this track's audio, rather than adding a track.
    track: Option<usize>,
    rate: u32,
    done: mpsc::Receiver<Result<Arc<StreamSource>, String>>,
}

/// Where converted copies of files go until projects have folders of
/// their own.
fn cache_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("daw").join("converted")
}

/// What the audio side is doing.
enum Audio {
    Running(Running),
    /// No device could be opened, so the engine runs without sound; the
    /// message says why. The `Running` is held to keep it going.
    Silent(#[allow(dead_code)] Running, String),
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
    audio: Audio,
    profile: Option<DeviceProfile>,
    /// The settings being edited in the device window.
    draft: Option<DeviceProfile>,
    hosts: Vec<HostInfo>,
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
}

/// How often to look for a lost device.
const RECONNECT_EVERY: Duration = Duration::from_secs(2);

impl App {
    fn new(files: Vec<String>) -> App {
        let profile = audio::default_profile();
        let rate = profile.as_ref().map_or(48_000, |p| p.rate);
        let (mut ctl, eng) = engine::new(audio::MAX_BLOCK);
        let disk = ctl.take_streamer().expect("new controller").spawn();
        let mut app = App {
            tracks: Vec::new(),
            imports: Vec::new(),
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            rate,
            ctl,
            _disk: disk,
            audio: Audio::Stopped,
            draft: profile.clone(),
            profile,
            hosts: audio::list(),
            show_devices: false,
            metronome: Metronome::default(),
            count_in: false,
            looping: false,
            loop_bars: (1, 5),
            master_meter: (0.0, 0.0),
            message: None,
            reconnect: None,
        };
        app.audio = app.open_device(eng);
        for f in files {
            app.add_file(Path::new(&f));
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
                let (mut ctl, eng) = engine::new(audio::MAX_BLOCK);
                self._disk = ctl.take_streamer().expect("new controller").spawn();
                self.ctl = ctl;
                self.rebuild();
                self.send(Command::SetMetronome(self.metronome));
                self.send_loop();
                Audio::Silent(audio::start_silent(self.rate, 256, eng), e)
            }
        }
    }

    /// Starts a fresh engine on the chosen device, keeping the session.
    fn restart_audio(&mut self) {
        // Stop the old audio before starting the new, keeping the place.
        let pos = self.ctl.status().position();
        self.audio = Audio::Stopped;
        let (mut ctl, eng) = engine::new(audio::MAX_BLOCK);
        let disk = ctl.take_streamer().expect("new controller").spawn();
        self.ctl = ctl;
        self._disk = disk;
        self.rate = self.profile.as_ref().map_or(self.rate, |p| p.rate);
        // Tracks at the old rate are opened again at the new one.
        let stale: Vec<_> = (self.tracks.iter().enumerate())
            .filter(|(_, t)| t.source.rate != self.rate)
            .map(|(i, t)| (i, t.original.clone()))
            .collect();
        for (i, path) in stale {
            self.start_import(path, Some(i));
        }
        self.rebuild();
        self.send(Command::SetMetronome(self.metronome));
        self.send_loop();
        self.send(Command::Seek(pos));
        self.audio = self.open_device(eng);
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
        self.start_import(path.to_path_buf(), None);
    }

    fn start_import(&mut self, path: PathBuf, track: Option<usize>) {
        let (tx, rx) = mpsc::channel();
        let (p, rate) = (path.clone(), self.rate);
        std::thread::spawn(move || {
            let _ = tx.send(import::open_at_rate(&p, rate, &cache_dir()));
        });
        self.imports.push(Import {
            path,
            track,
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
            let new_track = self.imports[i].track.is_none();
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
                Ok(_) if im.rate != self.rate => self.start_import(im.path, im.track),
                Ok(source) => {
                    changed = true;
                    match im.track.and_then(|t| self.tracks.get_mut(t)) {
                        Some(t) => t.source = source,
                        None => self.tracks.push(Track {
                            name: im
                                .path
                                .file_stem()
                                .map_or("Track".into(), |s| s.to_string_lossy().into_owned()),
                            original: im.path,
                            source,
                            mix: Mix::default(),
                            meter: (0.0, 0.0),
                        }),
                    }
                }
                Err(e) => self.message = Some(e),
            }
        }
        changed
    }

    /// Compiles the session and sends it to the engine.
    fn rebuild(&mut self) {
        let mut s = Session::new(self.tempo_map());
        for t in &self.tracks {
            // A track still being converted to a new rate plays nothing
            // until it is ready, but keeps its place in the mixer.
            let clips = if t.source.rate == self.rate {
                vec![ClipSpec::new(Ticks(0), t.source.clone())]
            } else {
                Vec::new()
            };
            let mut spec = TrackSpec::new(clips);
            spec.mix = t.mix;
            s.tracks.push(spec);
        }
        match s.compile(audio::MAX_BLOCK) {
            Ok(g) => {
                self.send_graph(g);
            }
            Err(e) => self.message = Some(e.to_string()),
        }
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

    fn toggle_play(&mut self) {
        if self.ctl.status().playing() {
            self.send(Command::Stop);
        } else if self.count_in {
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
                .on_hover_text("Space")
                .clicked()
            {
                self.toggle_play();
            }
            if ui.button("⏮").on_hover_text("Back to the start").clicked() {
                self.send(Command::Seek(Frames(0)));
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
            let t = ui.add(
                egui::DragValue::new(&mut self.tempo_bpm)
                    .range(20.0..=300.0)
                    .speed(0.2)
                    .suffix(" BPM"),
            );
            ui.label("Beats per bar");
            let b = ui.add(egui::DragValue::new(&mut self.beats_per_bar).range(1..=12));
            if t.changed() || b.changed() {
                self.rebuild();
                self.send_loop();
            }
        });
        ui.horizontal(|ui| {
            let mut m = self.metronome;
            ui.checkbox(&mut m.enabled, "Metronome");
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
                });
            ui.label("Click level");
            ui.add(egui::Slider::new(&mut m.level_db, -40.0..=0.0).suffix(" dB"));
            if m != self.metronome {
                self.metronome = m;
                self.send(Command::SetMetronome(m));
            }
            ui.checkbox(&mut self.count_in, "Count-in");
            ui.separator();

            let mut changed = ui.checkbox(&mut self.looping, "Loop bars").changed();
            changed |= ui
                .add(egui::DragValue::new(&mut self.loop_bars.0).range(1..=999))
                .changed();
            ui.label("to");
            changed |= ui
                .add(egui::DragValue::new(&mut self.loop_bars.1).range(2..=1000))
                .changed();
            if self.loop_bars.1 <= self.loop_bars.0 {
                self.loop_bars.1 = self.loop_bars.0 + 1;
            }
            if changed {
                self.send_loop();
            }
        });
    }

    fn mixer(&mut self, ui: &mut egui::Ui) {
        if self.tracks.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Drop WAV files here to add tracks.").size(18.0));
            });
            return;
        }
        let mut changes = Vec::new();
        egui::Grid::new("tracks")
            .num_columns(6)
            .spacing(vec2(12.0, 10.0))
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Track");
                ui.strong("Level");
                ui.strong("Volume");
                ui.strong("Pan");
                ui.strong("");
                ui.strong("");
                ui.end_row();
                for (i, t) in self.tracks.iter_mut().enumerate() {
                    let before = t.mix;
                    ui.label(&t.name)
                        .on_hover_text(t.source.path.display().to_string());
                    meter(ui, t.meter);
                    ui.add(
                        egui::Slider::new(&mut t.mix.gain_db, -60.0..=12.0)
                            .suffix(" dB")
                            .step_by(0.1),
                    );
                    ui.add(egui::Slider::new(&mut t.mix.pan, -1.0..=1.0).show_value(false))
                        .on_hover_text("Pan");
                    ui.toggle_value(&mut t.mix.mute, "Mute");
                    ui.toggle_value(&mut t.mix.solo, "Solo");
                    ui.end_row();
                    if t.mix != before {
                        changes.push((i, t.mix));
                    }
                }
                ui.label(RichText::new("Master").strong());
                meter(ui, self.master_meter);
                ui.end_row();
            });
        for (track, mix) in changes {
            self.send(Command::SetMix { track, mix });
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
                });
                egui::Grid::new("device").num_columns(2).show(ui, |ui| {
                    ui.label("Driver");
                    egui::ComboBox::from_id_salt("host")
                        .selected_text(&draft.host)
                        .show_ui(ui, |ui| {
                            for h in &self.hosts {
                                ui.selectable_value(&mut draft.host, h.name.clone(), &h.name);
                            }
                        });
                    ui.end_row();

                    let host = self.hosts.iter().find(|h| h.name == draft.host);
                    ui.label("Device");
                    egui::ComboBox::from_id_salt("device")
                        .selected_text(&draft.device)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for d in host.map(|h| h.devices.as_slice()).unwrap_or(&[]) {
                                ui.selectable_value(&mut draft.device, d.name.clone(), &d.name);
                            }
                        });
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
                        });
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
                        });
                    ui.end_row();

                    let outs = dev.map_or(2, |d| d.output_channels as usize).max(1);
                    ui.label("Main outputs");
                    ui.horizontal(|ui| {
                        channel_picker(ui, "main-l", &mut draft.main.0, outs);
                        channel_picker(ui, "main-r", &mut draft.main.1, outs);
                    });
                    ui.end_row();

                    ui.label("Metronome output");
                    egui::ComboBox::from_id_salt("click")
                        .selected_text(draft.click.map_or("Main mix".into(), |c| format!("Output {}", c + 1)))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut draft.click, None, "Main mix");
                            for c in 0..outs {
                                ui.selectable_value(&mut draft.click, Some(c), format!("Output {}", c + 1));
                            }
                        });
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
                        ui.label(format!("Output latency reported by the driver: {:.1} ms", us as f64 / 1000.0));
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
                        "The true round trip is measured with a loopback cable; that test comes with recording.",
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
                    }
                });
            });
        self.show_devices = open;
        if apply {
            self.reconnect = None;
            self.profile = self.draft.clone();
            self.restart_audio();
        }
    }
}

fn channel_picker(ui: &mut egui::Ui, id: &str, ch: &mut usize, outs: usize) {
    egui::ComboBox::from_id_salt(id)
        .width(60.0)
        .selected_text(format!("{}", *ch + 1))
        .show_ui(ui, |ui| {
            for c in 0..outs {
                ui.selectable_value(ch, c, format!("{}", c + 1));
            }
        });
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
            let (l, r) = self.ctl.status().take_track_peak(i);
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
            self.add_file(&p);
        }
        if self.finish_imports() {
            self.rebuild();
        }
        if !self.imports.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Space))
        {
            self.toggle_play();
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Settings", |ui| {
                    if ui.button("Audio device…").clicked() {
                        self.show_devices = true;
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
                    }
                    Audio::Silent(..) | Audio::Stopped => {
                        ui.colored_label(Color32::LIGHT_RED, "No audio device: playing silently");
                    }
                }
                if !self.imports.is_empty() {
                    ui.spinner();
                    ui.label(match self.imports.len() {
                        1 => "Opening 1 file…".to_string(),
                        n => format!("Opening {n} files…"),
                    });
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
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.mixer(ui));
        });
        self.device_window(&ctx);
        if self.ctl.status().playing() {
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DAW")
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
