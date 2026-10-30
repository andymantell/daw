// SPDX-License-Identifier: GPL-3.0-or-later
//! The monitor section pane (roadmap 2.3): a headphone mix for each
//! performer, the speakers' controls (listen, level, dim, mono, cut and
//! switching between sets of speakers), and talkback.
//!
//! Headphone mixes are part of the project: their levels for each track
//! reach the engine with the graph (see `App::route`), and level changes
//! go without a new one. Where they are plugged in, the extra speakers
//! and the talkback microphone belong to the computer, so they are kept
//! with the device settings. The speakers' controls are for now only:
//! every start is at full level, undimmed, in stereo, talkback off.

use daw_audio_io::Speakers;
use daw_engine::engine::Command;
use daw_engine::monitor::{
    ControlRoom, CueBus, CueSend, Listen, MAX_CUES, MAX_SPEAKERS, SpeakerSet, Talkback,
};
use daw_project::{CUE_OFF_DB, CueLevel, CueMix};
use eframe::egui::{self, RichText, vec2};

use crate::{App, meters};

/// The pane's state: the project's headphone mixes, and the speakers'
/// controls.
pub(crate) struct Cues {
    pub(crate) mixes: Vec<CueMix>,
    /// What the speakers play: the main mix, or a headphone mix by id.
    pub(crate) listen: Option<String>,
    pub(crate) gain_db: f32,
    pub(crate) dim: bool,
    pub(crate) dim_db: f32,
    pub(crate) mono: bool,
    pub(crate) cut: bool,
    /// The speakers playing: 0 for those on the main outputs, then the
    /// device's extra sets in order.
    pub(crate) speakers: usize,
    /// Talkback on, held or latched.
    pub(crate) talk: bool,
    /// The Talk button stays on when let go.
    pub(crate) latch: bool,
    pub(crate) talk_db: f32,
    pub(crate) talk_dim_db: f32,
    /// Each mix's peaks, falling back.
    meters: Vec<(f32, f32)>,
}

impl Default for Cues {
    fn default() -> Cues {
        Cues {
            mixes: Vec::new(),
            listen: None,
            gain_db: 0.0,
            dim: false,
            dim_db: 20.0,
            mono: false,
            cut: false,
            speakers: 0,
            talk: false,
            latch: false,
            talk_db: 0.0,
            talk_dim_db: 20.0,
            meters: Vec::new(),
        }
    }
}

/// The engine's level for a track in a mix.
fn send(l: &CueLevel) -> CueSend {
    CueSend {
        gain_db: l.gain_db as f32,
        pan: l.pan as f32,
        mute: l.mute,
    }
}

/// What the pane asks for after drawing.
enum Action {
    /// A track's level in a mix: (mix, track).
    Level(usize, usize),
    /// A mix's own settings, or the speakers'.
    Buses,
    /// The device settings changed: save them.
    Device,
    Remove(usize),
    CopyMain(usize),
}

impl App {
    /// Each track's place in each headphone mix, as the engine numbers
    /// them.
    pub(crate) fn cue_sends(&self, key: &str) -> [CueSend; MAX_CUES] {
        let mut out = [CueSend::default(); MAX_CUES];
        let Some(t) = self.tracks.iter().find(|t| t.key == key) else {
            return out;
        };
        let track = self.track_value(t);
        for (o, m) in out.iter_mut().zip(&self.cues.mixes) {
            *o = send(&m.level(&track));
        }
        out
    }

    /// The headphone mixes' own settings for the engine.
    fn cue_buses(&self) -> [CueBus; MAX_CUES] {
        let phones = self.profile.as_ref().map(|p| &p.monitor.phones);
        std::array::from_fn(|k| match self.cues.mixes.get(k) {
            Some(m) => CueBus {
                on: true,
                gain_db: m.gain_db as f32,
                mute: m.mute,
                follow_main: m.follow_main,
                click_db: m.click_db.map(|d| d as f32),
                talkback: m.talkback,
                out: (phones.and_then(|p| p.get(k).copied().flatten()))
                    .map(|(l, r)| (l as u16, r as u16)),
            },
            None => CueBus::default(),
        })
    }

    /// The speakers' controls for the engine.
    fn control_room(&self) -> ControlRoom {
        let c = &self.cues;
        let dev = self.profile.as_ref().map(|p| &p.monitor);
        let mut speakers = [SpeakerSet::default(); MAX_SPEAKERS];
        speakers[0].trim_db = dev.map_or(0.0, |d| d.main_trim_db);
        for (s, extra) in speakers[1..]
            .iter_mut()
            .zip(dev.iter().flat_map(|d| &d.speakers))
        {
            *s = SpeakerSet {
                out: Some((extra.out.0 as u16, extra.out.1 as u16)),
                trim_db: extra.trim_db,
            };
        }
        let sets = 1 + dev.map_or(0, |d| d.speakers.len()).min(MAX_SPEAKERS - 1);
        ControlRoom {
            listen: (c.listen.as_deref())
                .and_then(|id| c.mixes.iter().position(|m| m.id == id))
                .map_or(Listen::Main, Listen::Cue),
            gain_db: c.gain_db,
            dim: c.dim,
            dim_db: c.dim_db,
            mono: c.mono,
            cut: c.cut,
            speakers,
            active: c.speakers.min(sets - 1),
            talkback: Talkback {
                input: dev.and_then(|d| d.talkback).map(|ch| ch as u16),
                gain_db: c.talk_db,
                on: c.talk,
                dim_db: c.talk_dim_db,
            },
        }
    }

    /// Sends the headphone mixes' settings and the speakers' to the
    /// engine.
    pub(crate) fn send_monitor(&mut self) {
        for (cue, bus) in self.cue_buses().into_iter().enumerate() {
            self.send(Command::SetCueBus { cue, bus });
        }
        let room = self.control_room();
        self.send(Command::SetControlRoom(room));
    }

    /// Adds a headphone mix, with every audio track in at 0 dB.
    pub(crate) fn add_cue_mix(&mut self) {
        if self.cues.mixes.len() >= MAX_CUES {
            return;
        }
        let n = self.cues.mixes.len() + 1;
        let mix = CueMix::new(daw_project::new_id(), format!("Headphones {n}"));
        self.cues.mixes.push(mix);
        self.rebuild();
    }

    /// Toggles talkback, for its key.
    pub(crate) fn toggle_talkback(&mut self) {
        self.cues.talk = !self.cues.talk;
        let room = self.control_room();
        self.send(Command::SetControlRoom(room));
    }

    /// Toggles a speaker control for its key, and sends it.
    pub(crate) fn toggle_room(&mut self, f: impl FnOnce(&mut Cues)) {
        f(&mut self.cues);
        let room = self.control_room();
        self.send(Command::SetControlRoom(room));
    }

    /// The device's output channels.
    fn output_count(&self) -> usize {
        let Some(p) = &self.profile else { return 2 };
        (self.hosts.iter())
            .filter(|h| h.name == p.host)
            .flat_map(|h| &h.devices)
            .find(|d| d.name == p.device)
            .map_or(2, |d| d.output_channels as usize)
            .max(2)
    }

    pub(crate) fn monitor_pane(&mut self, ui: &mut egui::Ui) {
        // Meters fall back at about 20 dB a second.
        let dt = ui.ctx().input(|i| i.stable_dt).min(0.1);
        let fall = 10f32.powf(-dt);
        self.cues.meters.resize(self.cues.mixes.len(), (0.0, 0.0));
        for (k, m) in self.cues.meters.iter_mut().enumerate() {
            let (l, r) = self.ctl.status().monitor.take_cue_peak(k);
            *m = ((m.0 * fall).max(l), (m.1 * fall).max(r));
        }
        let mut actions = Vec::new();
        ui.heading("Speakers");
        self.speakers_row(ui, &mut actions);
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("Headphone mixes");
            let full = self.cues.mixes.len() >= MAX_CUES;
            if ui
                .add_enabled(!full, egui::Button::new("+ Add headphone mix"))
                .on_hover_text("A mix of its own for one performer")
                .on_disabled_hover_text(format!("{MAX_CUES} mixes is the most"))
                .clicked()
            {
                self.add_cue_mix();
            }
        });
        if self.cues.mixes.is_empty() {
            ui.weak(
                "Each performer can have a mix of their own in their headphones, \
                 apart from what the speakers play.",
            );
        } else {
            self.cue_grid(ui, &mut actions);
        }
        ui.add_space(8.0);
        egui::CollapsingHeader::new("Outputs and talkback microphone (this computer)")
            .id_salt("monitor setup")
            .show(ui, |ui| self.monitor_setup(ui, &mut actions));
        self.monitor_actions(actions);
    }

    fn speakers_row(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let names: Vec<String> = std::iter::once("Main outputs".to_string())
            .chain(
                (self.profile.iter())
                    .flat_map(|p| p.monitor.speakers.iter().map(|s| s.name.clone())),
            )
            .take(MAX_SPEAKERS)
            .collect();
        let mic = self.profile.as_ref().and_then(|p| p.monitor.talkback);
        let mixes: Vec<(String, String)> = (self.cues.mixes.iter())
            .map(|m| (m.id.clone(), m.name.clone()))
            .collect();
        let c = &mut self.cues;
        let before = (
            c.listen.clone(),
            c.gain_db,
            c.dim,
            c.mono,
            c.cut,
            c.speakers,
            c.talk,
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Listen to");
            let shown = (c.listen.as_deref())
                .and_then(|id| mixes.iter().find(|(k, _)| k == id))
                .map_or("Main mix".to_string(), |(_, n)| n.clone());
            egui::ComboBox::from_id_salt("listen")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut c.listen, None, "Main mix");
                    for (id, name) in &mixes {
                        ui.selectable_value(&mut c.listen, Some(id.clone()), name);
                    }
                })
                .response
                .accessible_name("Speakers listen to")
                .on_hover_text("Hear a performer's headphone mix on the speakers");
            ui.name_inputs("Speaker level", |ui| {
                ui.add(
                    egui::Slider::new(&mut c.gain_db, -60.0..=12.0)
                        .suffix(" dB")
                        .step_by(0.5),
                )
            });
            ui.toggle_value(&mut c.dim, "Dim")
                .on_hover_text(format!("Turn the speakers down {} dB", c.dim_db));
            ui.toggle_value(&mut c.mono, "Mono")
                .on_hover_text("Left and right together, to check the mix in mono");
            ui.toggle_value(&mut c.cut, "Cut")
                .accessible_name("Cut the speakers")
                .on_hover_text("Silence the speakers (the headphones go on)");
            if names.len() > 1 {
                ui.separator();
                for (k, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut c.speakers, k, n)
                        .on_hover_text("Play on these speakers");
                }
            }
            ui.separator();
            let talk = ui
                .add_enabled(
                    mic.is_some(),
                    egui::Button::selectable(c.talk, "🎙 Talk").min_size(vec2(64.0, 0.0)),
                )
                .accessible_name("Talkback")
                .on_hover_text(if c.latch {
                    "Click to talk to the headphones, click again to stop"
                } else {
                    "Hold to talk to the headphones"
                })
                .on_disabled_hover_text(
                    "Choose the talkback microphone under Outputs and talkback microphone",
                );
            if c.latch {
                if talk.clicked() {
                    c.talk = !c.talk;
                }
            } else if mic.is_some() {
                c.talk = talk.is_pointer_button_down_on();
            }
            if ui
                .checkbox(&mut c.latch, "Latch")
                .on_hover_text("The Talk button stays on until clicked again")
                .changed()
            {
                c.talk = false;
            }
            ui.name_inputs("Talkback level", |ui| {
                ui.add(
                    egui::Slider::new(&mut c.talk_db, -40.0..=12.0)
                        .suffix(" dB")
                        .step_by(0.5),
                )
            });
        });
        if before
            != (
                c.listen.clone(),
                c.gain_db,
                c.dim,
                c.mono,
                c.cut,
                c.speakers,
                c.talk,
            )
        {
            actions.push(Action::Buses);
        }
    }

    fn cue_grid(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let outs = self.output_count();
        let tracks: Vec<daw_project::Track> =
            self.tracks.iter().map(|t| self.track_value(t)).collect();
        let mut phones = (self.profile.as_ref())
            .map(|p| p.monitor.phones.clone())
            .unwrap_or_default();
        phones.resize(self.cues.mixes.len().max(phones.len()), None);
        let has_device = self.profile.is_some();
        let meters_now = self.cues.meters.clone();
        let mixes = &mut self.cues.mixes;
        let n = mixes.len();
        egui::Grid::new("cue mixes")
            .num_columns(n + 1)
            .spacing(vec2(16.0, 8.0))
            .striped(true)
            .show(ui, |ui| {
                ui.label("");
                for m in mixes.iter_mut() {
                    let before = m.name.clone();
                    if ui
                        .add(egui::TextEdit::singleline(&mut m.name).desired_width(140.0))
                        .accessible_name(format!("Name of {before}"))
                        .changed()
                    {
                        // The name only; nothing to send.
                    }
                }
                ui.end_row();

                ui.label("Plays on");
                for (k, m) in mixes.iter().enumerate() {
                    ui.add_enabled_ui(has_device, |ui| {
                        if out_picker(ui, k, &m.name, &mut phones[k], outs) {
                            actions.push(Action::Device);
                        }
                    });
                }
                ui.end_row();

                ui.label("Level");
                for (k, m) in mixes.iter_mut().enumerate() {
                    ui.vertical(|ui| {
                        meters::strip(
                            ui,
                            meters_now.get(k).copied().unwrap_or_default(),
                            (0.0, 0.0),
                        )
                        .on_hover_text("Peak level in these headphones");
                        let mut db = m.gain_db as f32;
                        let r = ui.name_inputs(format!("{} level", m.name), |ui| {
                            ui.add(
                                egui::Slider::new(&mut db, -60.0..=12.0)
                                    .suffix(" dB")
                                    .step_by(0.5),
                            )
                        });
                        if r.changed() {
                            m.gain_db = db as f64;
                            actions.push(Action::Buses);
                        }
                    });
                }
                ui.end_row();

                ui.label("");
                for (k, m) in mixes.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let name = m.name.clone();
                        let mut changed = ui
                            .toggle_value(&mut m.mute, "Mute")
                            .accessible_name(format!("Mute {name}"))
                            .changed();
                        changed |= ui
                            .checkbox(&mut m.follow_main, "Same as main")
                            .accessible_name(format!("{name} is the same as the main mix"))
                            .on_hover_text("Play the main mix, as the speakers do")
                            .changed();
                        changed |= ui
                            .checkbox(&mut m.talkback, "Talkback")
                            .accessible_name(format!("Talkback to {name}"))
                            .on_hover_text("Hear the talkback microphone")
                            .changed();
                        if changed {
                            actions.push(Action::Buses);
                        }
                        if ui
                            .small_button("Copy main mix")
                            .on_hover_text("Set every track as it is in the main mix")
                            .clicked()
                        {
                            actions.push(Action::CopyMain(k));
                        }
                        if ui
                            .small_button("Remove")
                            .accessible_name(format!("Remove {name}"))
                            .clicked()
                        {
                            actions.push(Action::Remove(k));
                        }
                    });
                }
                ui.end_row();

                ui.label("Click");
                for m in mixes.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!m.follow_main, |ui| {
                            let mut on = m.click_db.is_some();
                            let name = m.name.clone();
                            if ui
                                .checkbox(&mut on, "")
                                .accessible_name(format!("Click in {name}"))
                                .on_hover_text("The metronome in these headphones")
                                .changed()
                            {
                                m.click_db = on.then_some(0.0);
                                actions.push(Action::Buses);
                            }
                            if let Some(db) = &mut m.click_db {
                                let mut v = *db as f32;
                                let r = ui.name_inputs(format!("Click level in {name}"), |ui| {
                                    ui.add(
                                        egui::Slider::new(&mut v, -40.0..=12.0)
                                            .suffix(" dB")
                                            .step_by(0.5),
                                    )
                                });
                                if r.changed() {
                                    *db = v as f64;
                                    actions.push(Action::Buses);
                                }
                            }
                        })
                        .response
                        .on_disabled_hover_text("Same as the main mix, click and all");
                    });
                }
                ui.end_row();

                for (ti, t) in tracks.iter().enumerate() {
                    let tag = if t.kind.is_bus() {
                        RichText::new(&t.name).italics()
                    } else {
                        RichText::new(&t.name)
                    };
                    ui.label(tag);
                    for (k, m) in mixes.iter_mut().enumerate() {
                        if m.follow_main {
                            ui.weak("as in the main mix");
                            continue;
                        }
                        let mut l = m.level(t);
                        if level_cell(ui, &m.name, &t.name, &mut l) {
                            m.set_level(l);
                            actions.push(Action::Level(k, ti));
                        }
                    }
                    ui.end_row();
                }
            });
        if let Some(p) = &mut self.profile
            && p.monitor.phones != phones
        {
            while phones.last() == Some(&None) {
                phones.pop();
            }
            p.monitor.phones = phones;
        }
    }

    /// Where the speakers and headphones are plugged in, and the
    /// talkback microphone: kept with this computer's device settings.
    fn monitor_setup(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let outs = self.output_count();
        let ins = self.input_count();
        let Some(p) = &mut self.profile else {
            ui.weak("Choose an audio device first (Settings → Audio device).");
            return;
        };
        let main = p.main;
        let m = &mut p.monitor;
        let before = m.clone();
        let c = &mut self.cues;
        egui::Grid::new("monitor setup grid")
            .num_columns(4)
            .spacing(vec2(10.0, 6.0))
            .show(ui, |ui| {
                ui.label("Main outputs");
                ui.weak(format!("{} and {}", main.0 + 1, main.1 + 1));
                ui.name_inputs("Main speakers trim", |ui| {
                    ui.add(
                        egui::Slider::new(&mut m.main_trim_db, -12.0..=12.0)
                            .suffix(" dB trim")
                            .step_by(0.5),
                    )
                });
                ui.label("");
                ui.end_row();
                let mut remove = None;
                for (k, s) in m.speakers.iter_mut().enumerate() {
                    let before = s.name.clone();
                    ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(110.0))
                        .accessible_name(format!("Name of speakers {}", k + 2));
                    let mut pair = Some(s.out);
                    if out_picker(ui, 100 + k, &before, &mut pair, outs)
                        && let Some(pr) = pair
                    {
                        s.out = pr;
                    }
                    ui.name_inputs(format!("{before} trim"), |ui| {
                        ui.add(
                            egui::Slider::new(&mut s.trim_db, -12.0..=12.0)
                                .suffix(" dB trim")
                                .step_by(0.5),
                        )
                    });
                    if ui
                        .small_button("Remove")
                        .accessible_name(format!("Remove {before}"))
                        .clicked()
                    {
                        remove = Some(k);
                    }
                    ui.end_row();
                }
                if let Some(k) = remove {
                    m.speakers.remove(k);
                    c.speakers = 0;
                }
                if m.speakers.len() < MAX_SPEAKERS - 1
                    && ui
                        .button("+ Add speakers")
                        .on_hover_text("Another pair of speakers to switch to")
                        .clicked()
                {
                    let first = (2 + 2 * m.speakers.len()).min(outs.saturating_sub(2));
                    m.speakers.push(Speakers {
                        name: format!("Speakers {}", m.speakers.len() + 2),
                        out: (first, first + 1),
                        trim_db: 0.0,
                    });
                }
                ui.end_row();

                ui.label("Talkback microphone");
                let shown = m
                    .talkback
                    .map_or("None".to_string(), |ch| format!("Input {}", ch + 1));
                egui::ComboBox::from_id_salt("talkback input")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut m.talkback, None, "None");
                        for ch in 0..ins {
                            ui.selectable_value(
                                &mut m.talkback,
                                Some(ch),
                                format!("Input {}", ch + 1),
                            );
                        }
                    })
                    .response
                    .accessible_name("Talkback microphone input");
                ui.label("");
                ui.label("");
                ui.end_row();
            });
        let room = (c.dim_db, c.talk_dim_db);
        ui.horizontal(|ui| {
            ui.name_inputs("Dim by", |ui| {
                ui.add(
                    egui::Slider::new(&mut c.dim_db, 0.0..=40.0)
                        .text("Dim by")
                        .suffix(" dB"),
                )
            });
            ui.name_inputs("Talkback dims the speakers by", |ui| {
                ui.add(
                    egui::Slider::new(&mut c.talk_dim_db, 0.0..=40.0)
                        .text("Talkback dims by")
                        .suffix(" dB"),
                )
            });
        });
        if *m != before {
            actions.push(Action::Device);
        }
        if room != (c.dim_db, c.talk_dim_db) {
            actions.push(Action::Buses);
        }
    }

    fn monitor_actions(&mut self, actions: Vec<Action>) {
        let mut buses = false;
        for a in actions {
            match a {
                Action::Level(k, ti) => {
                    let sends = self.tracks.get(ti).map(|t| self.cue_sends(&t.key));
                    if let Some(s) = sends {
                        self.send(Command::SetCueSend {
                            track: ti,
                            cue: k,
                            send: s[k],
                        });
                    }
                }
                Action::Buses => buses = true,
                Action::Device => {
                    if let Some(p) = &self.profile {
                        self.saved.use_profile(p.clone());
                        if let Err(e) = crate::save_settings(&self.saved) {
                            self.message = Some(format!("Could not save the device settings: {e}"));
                        }
                        self.draft = Some(p.clone());
                    }
                    buses = true;
                }
                Action::Remove(k) => {
                    let gone = self.cues.mixes.remove(k);
                    if self.cues.listen.as_deref() == Some(gone.id.as_str()) {
                        self.cues.listen = None;
                    }
                    // Its outputs pass to the mix that takes its place.
                    if let Some(p) = &mut self.profile
                        && k < p.monitor.phones.len()
                    {
                        p.monitor.phones.remove(k);
                    }
                    self.rebuild();
                }
                Action::CopyMain(k) => {
                    let levels: Vec<CueLevel> = (self.tracks.iter())
                        .map(|t| CueLevel {
                            track: t.key.clone(),
                            // A bus that tracks go through is heard
                            // through them already.
                            gain_db: if t.mix.mute || is_group(self, &t.key) {
                                CUE_OFF_DB
                            } else {
                                t.mix.gain_db as f64
                            },
                            pan: t.mix.pan as f64,
                            mute: false,
                        })
                        .collect();
                    if let Some(m) = self.cues.mixes.get_mut(k) {
                        m.levels = levels;
                        m.follow_main = false;
                    }
                    self.rebuild();
                }
            }
        }
        if buses {
            self.send_monitor();
        }
    }
}

/// Whether other tracks output to this bus (rather than only sending to
/// it, as to an effect return).
fn is_group(app: &App, key: &str) -> bool {
    app.tracks.iter().any(|t| t.goes_to() == Some(key))
}

/// A pair of outputs, or none. True if changed.
fn out_picker(
    ui: &mut egui::Ui,
    id: usize,
    name: &str,
    pair: &mut Option<(usize, usize)>,
    outs: usize,
) -> bool {
    let text = |p: Option<(usize, usize)>| match p {
        Some((l, r)) => format!("Outputs {}–{}", l + 1, r + 1),
        None => "Nowhere".to_string(),
    };
    let before = *pair;
    egui::ComboBox::from_id_salt(("cue out", id))
        .selected_text(text(*pair))
        .show_ui(ui, |ui| {
            ui.selectable_value(pair, None, "Nowhere");
            for l in (0..outs.saturating_sub(1)).step_by(2) {
                ui.selectable_value(pair, Some((l, l + 1)), text(Some((l, l + 1))));
            }
        })
        .response
        .accessible_name(format!("Outputs for {name}"));
    *pair != before
}

/// A track's level, pan and mute in one mix. True if changed.
fn level_cell(ui: &mut egui::Ui, mix: &str, track: &str, l: &mut CueLevel) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let off = l.gain_db <= CUE_OFF_DB;
        let mut db = if off { -60.0 } else { l.gain_db as f32 };
        let r = ui.name_inputs(format!("{track} in {mix}"), |ui| {
            ui.add(
                egui::Slider::new(&mut db, -60.0..=12.0)
                    .custom_formatter(|v, _| {
                        if v <= -60.0 {
                            "off".into()
                        } else {
                            format!("{v:.1} dB")
                        }
                    })
                    .step_by(0.5),
            )
        });
        if r.changed() {
            l.gain_db = if db <= -60.0 { CUE_OFF_DB } else { db as f64 };
            changed = true;
        }
        let mut pan = l.pan as f32;
        if ui
            .add(egui::Slider::new(&mut pan, -1.0..=1.0).show_value(false))
            .accessible_name(format!("{track} pan in {mix}"))
            .on_hover_text("Pan")
            .changed()
        {
            l.pan = pan as f64;
            changed = true;
        }
        changed |= ui
            .toggle_value(&mut l.mute, "M")
            .accessible_name(format!("Mute {track} in {mix}"))
            .changed();
    });
    changed
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::App;
    use crate::workspace::Pane;

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| {
                let mut app = App::new(Vec::new());
                app.workspace.layout.open(Pane::Monitor);
                app
            });
        h.run();
        h
    }

    fn settle(h: &mut Harness<'_, App>) {
        for _ in 0..8 {
            h.step();
        }
    }

    fn until(h: &mut Harness<'_, App>, what: impl Fn(&App) -> bool) {
        for _ in 0..1000 {
            h.step();
            if what(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    fn tone(dir: &Path, rate: u32) -> PathBuf {
        let path = dir.join("guitar.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate {
            let x = (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin();
            w.write_sample((x * 16_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    #[test]
    fn a_headphone_mix_is_made_saved_and_undone() {
        let dir = std::env::temp_dir().join(format!("daw-monitor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = harness();
        let rate = h.state().rate;
        h.state_mut().add_file(&tone(&dir, rate));
        until(&mut h, |a| !a.tracks.is_empty());

        h.get_by_label("+ Add headphone mix").click();
        settle(&mut h);
        let app = h.state_mut();
        assert_eq!(app.cues.mixes.len(), 1);
        let key = app.tracks[0].key.clone();
        // An audio track starts in the mix at 0 dB.
        assert_eq!(app.cue_sends(&key)[0].gain_db, 0.0);

        // Turned down in the mix, without touching the main mix.
        let mut l = app.cues.mixes[0].level(&app.track_value(&app.tracks[0]));
        l.gain_db = -12.0;
        app.cues.mixes[0].set_level(l);
        app.monitor_actions(vec![super::Action::Level(0, 0)]);
        settle(&mut h);
        let app = h.state_mut();
        assert_eq!(app.cue_sends(&key)[0].gain_db, -12.0);
        assert_eq!(app.tracks[0].mix.gain_db, 0.0);

        // In the project, and undone a step at a time.
        let ctx = h.ctx.clone();
        let app = h.state_mut();
        app.sync_doc(&ctx, true);
        let p = app.project();
        assert_eq!(p.cue_mixes.len(), 1);
        assert_eq!(p.cue_mixes[0].levels[0].gain_db, -12.0);
        app.undo(&ctx);
        settle(&mut h);
        assert_eq!(h.state().cues.mixes[0].levels.len(), 0, "level undone");
        let app = h.state_mut();
        app.undo(&ctx);
        settle(&mut h);
        assert!(h.state().cues.mixes.is_empty(), "mix undone");
    }

    #[test]
    fn the_speaker_controls_reach_the_engine_state() {
        let mut h = harness();
        settle(&mut h);
        h.get_by_label("Dim").click();
        h.get_by_label("Mono").click();
        settle(&mut h);
        let room = h.state().control_room();
        assert!(room.dim && room.mono && !room.cut);
        // No microphone chosen: talkback can't come on.
        assert!(!room.talkback.on);
        assert_eq!(room.talkback.input, None);
    }
}
