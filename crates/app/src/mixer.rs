// SPDX-License-Identifier: GPL-3.0-or-later
//! The mixer pane (roadmap 2.3): a strip per track and bus, with where
//! each goes, its sends, its control group, and the control groups'
//! own faders.
//!
//! Routing lives on the app's tracks by key, and is turned into indexes
//! for the engine in `App::session`. Volume, pan, mute, solo, send
//! levels and control-group faders reach the engine without a new graph;
//! anything else (a new bus, a changed output, a send added) rebuilds it.

use daw_engine::engine::Command;
use daw_engine::graph::{GroupSpec, Output, Session, TrackSpec};
use daw_project::{ControlGroup, Send, TrackKind};
use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, pos2, vec2};

use crate::{App, Track, desk, spoken, theme};

/// Takes a track spec out of its routing, for rendering it on its own
/// (a stem, or frozen audio): straight to the master, with no sends,
/// control group or sidechains.
pub(crate) fn alone(spec: &mut TrackSpec) {
    spec.output = Output::Master;
    spec.sends.clear();
    spec.group = None;
    // Extra instrument outputs routed to buses (a drum machine's kick
    // to its own bus) come back through the track, so all it plays is
    // heard, and no route names a bus that isn't in the session.
    spec.instrument_outputs.clear();
    for i in &mut spec.inserts {
        i.sidechain = None;
    }
}

/// The session as it sounds coming out of bus `bus` alone, for that
/// bus's stem: everything routed or sent into it (directly or through
/// other buses), through its effects and fader, and nothing else. Every
/// other track that went to the master goes instead to a muted bus
/// added at the end, so its sends into this bus still sound. The bus
/// still goes through the master, as it does in the mix.
pub(crate) fn through_bus(s: &Session, bus: usize) -> Session {
    let mut s = s.clone();
    let silent = s.tracks.len();
    for (i, t) in s.tracks.iter_mut().enumerate() {
        if i == bus {
            t.output = Output::Master;
            t.mix.mute = false;
        } else if t.output == Output::Master {
            t.output = Output::Track(silent);
        }
    }
    let mut sink = TrackSpec::new(Vec::new());
    sink.mix.mute = true;
    s.tracks.push(sink);
    s
}

/// What the mixer asks the app to do after drawing, once the borrows
/// are over.
enum Action {
    /// A track's volume, pan, mute or solo.
    Mix(usize, daw_engine::graph::Mix),
    /// The master fader, in dB.
    MasterGain(f32),
    /// A track's send levels, as the engine numbers them.
    Sends(usize),
    Group(usize),
    Take(usize, Option<usize>),
    /// Put the track in this folder (or none), and move it under it.
    Parent(usize, Option<String>),
    Remove(usize),
    RemoveGroup(usize),
    /// The faders of the other selected strips, by this many dB.
    Follow(String, f32),
    /// Select this strip: alone, or added to (or taken from) the rest.
    Select(String, bool),
}

/// How the mixer desk is shown.
#[derive(Default)]
pub(crate) struct DeskView {
    /// Strips with only the fader, meter, pan, mute, solo and name.
    pub(crate) narrow: bool,
    /// The strips selected, by track key: their faders move together.
    pub(crate) selected: Vec<String>,
}

/// A row of controls on a strip.
const ROW: f32 = 24.0;
/// The pan pot's row.
const PAN_ROW: f32 = 36.0;
/// The name tag's row.
const TAG_ROW: f32 = 28.0;
/// Inside a strip's edge.
const INSET: f32 = 4.0;

/// Lays out a strip from the top down, each part in a rectangle of its
/// own, so a control stays put whatever else on the strip changes.
struct Rows {
    rect: Rect,
    y: f32,
    salt: egui::Id,
    n: usize,
}

impl Rows {
    fn new(rect: Rect, salt: egui::Id) -> Rows {
        Rows {
            rect,
            y: rect.top() + INSET,
            salt,
            n: 0,
        }
    }

    /// The next `h` points of the strip, for `add` to draw in.
    fn row<R>(&mut self, ui: &mut egui::Ui, h: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let r = Rect::from_min_size(
            pos2(self.rect.left() + INSET, self.y),
            vec2(self.rect.width() - 2.0 * INSET, h),
        );
        self.y += h;
        self.n += 1;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(r)
                .id_salt(self.salt.with(self.n))
                .layout(Layout::left_to_right(Align::Center).with_main_align(Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 3.0;
        add(&mut child)
    }

    /// Leaves `h` points empty.
    fn skip(&mut self, h: f32) {
        self.y += h;
        self.n += 1;
    }
}

/// Mute, solo and arm lit in their own colours when on.
fn lit<R>(ui: &mut egui::Ui, colour: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let on = theme::look(ui.ctx()).c().bg;
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        v.selection.bg_fill = colour;
        v.selection.stroke.color = on;
        add(ui)
    })
    .inner
}

/// The size of a strip's fixed parts, and its fader's travel.
struct Geometry {
    width: f32,
    /// Routing rows above the pan pot.
    rows: usize,
    travel: f32,
    narrow: bool,
    /// A short pane: arming and effects, the rest of the routing in a
    /// menu.
    compact: bool,
}

/// A part of a strip's routing section.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Source,
    Arm,
    Fx,
    Out,
    Sends,
    Group,
    Folder,
    Take,
}

impl Part {
    /// Its name in the compact strip's routing menu.
    fn title(self) -> &'static str {
        match self {
            Part::Source => "In",
            Part::Arm => "",
            Part::Fx => "",
            Part::Out => "Out",
            Part::Sends => "Sends",
            Part::Group => "Group",
            Part::Folder => "Folder",
            Part::Take => "Take",
        }
    }
}

/// What the strips' routing picks from.
struct Routing {
    inputs: usize,
    direct: bool,
    frozen: Vec<bool>,
    buses: Vec<(String, String)>,
    folders: Vec<(String, String)>,
    /// Which buses, and which folders, each track may go to.
    allowed: Vec<Vec<bool>>,
    folders_ok: Vec<Vec<bool>>,
    groups: Vec<ControlGroup>,
}

impl Geometry {
    fn height(&self) -> f32 {
        2.0 * INSET
            + self.rows as f32 * ROW
            + PAN_ROW
            + self.travel
            + desk::CAP_H
            + 2.0 * ROW
            + TAG_ROW
    }
}

impl App {
    /// Adds a control group, numbered after those there.
    pub(crate) fn add_control_group(&mut self) {
        let n = self.groups.len() + 1;
        self.groups.push(ControlGroup::new(
            daw_project::new_id(),
            format!("Group {n}"),
        ));
        self.rebuild();
    }

    /// Adds a bus or a folder, numbered after those there.
    pub(crate) fn add_bus(&mut self, kind: TrackKind) {
        let n = self.tracks.iter().filter(|t| t.kind == kind).count();
        let what = if kind == TrackKind::Folder {
            "Folder"
        } else {
            "Bus"
        };
        let mut t = Track::new(format!("{what} {}", n + 1));
        t.kind = kind;
        self.tracks.push(t);
        self.rebuild();
    }

    /// The mixer pane: a mixing desk, one vertical strip per track and
    /// bus in the timeline's order, the control groups and the master
    /// pinned on the right.
    pub(crate) fn mixer(&mut self, ui: &mut egui::Ui) {
        if self.tracks.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Drop audio or video files here to add tracks.").size(18.0));
                ui.add_space(8.0);
                // ai: add-audio-track
                if ui.button("Or add an empty track to record on").clicked() {
                    self.tracks.push(Track::new("Track 1".into()));
                    self.rebuild();
                }
                // ai: add-instrument-track
                if ui
                    .button("Or add an instrument track for notes")
                    .on_hover_text(
                        "A track that plays notes through an instrument: from a MIDI \
                         keyboard, or drawn in the note editor",
                    )
                    .clicked()
                {
                    let t = Track::instrument("Instrument 1".into());
                    if let Some(i) = &t.instrument {
                        i.make_into(self.rate, &mut self.pending_instruments);
                    }
                    self.tracks.push(t);
                    self.rebuild();
                }
            });
            return;
        }
        let mut actions = Vec::new();
        let mut rebuild = self.desk_toolbar(ui);
        ui.add_space(4.0);

        let narrow = self.desk.narrow;
        let mut g = Geometry {
            width: if narrow {
                desk::NARROW_W
            } else {
                desk::STRIP_W
            },
            rows: if narrow { 0 } else { self.parts(false).len() },
            travel: 0.0,
            narrow,
            compact: false,
        };
        // The fader takes what height is left, less room for a scroll
        // bar. A pane too short for the whole strip shows the routing
        // compactly; shorter still, the desk scrolls, starting at its
        // foot so the faders show.
        let height = ui.available_height() - 14.0;
        if !narrow && height < g.height() + desk::MIN_TRAVEL {
            g.rows = 2;
            g.compact = true;
        }
        g.travel = desk::travel(height, g.height());

        egui::ScrollArea::vertical()
            .id_salt("desk")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), g.height() + 14.0),
                    Layout::right_to_left(Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        self.master_strip(ui, &g, &mut actions);
                        for gi in (0..self.groups.len()).rev() {
                            self.group_strip(ui, &g, gi, &mut actions);
                        }
                        ui.separator();
                        egui::ScrollArea::horizontal()
                            .id_salt("desk strips")
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    rebuild |= self.track_strips(ui, &g, &mut actions);
                                });
                            });
                    },
                );
            });
        rebuild |= self.desk_actions(actions);
        if rebuild {
            self.rebuild();
        }
    }

    /// The buttons along the top of the desk. Returns true if the tracks
    /// changed.
    fn desk_toolbar(&mut self, ui: &mut egui::Ui) -> bool {
        let simple = self.simple.on;
        let inputs = self.input_count();
        let mut rebuild = false;
        ui.horizontal_wrapped(|ui| {
            // ai: add-audio-track
            if ui.button("+ Add track").clicked() {
                self.tracks
                    .push(Track::new(format!("Track {}", self.tracks.len() + 1)));
                rebuild = true;
            }
            // ai: add-instrument-track
            if ui
                .button("+ Add instrument track")
                .on_hover_text(
                    "A track that plays notes through an instrument: from a MIDI keyboard, \
                     or drawn in the note editor",
                )
                .clicked()
            {
                let n = self.tracks.iter().filter(|t| t.kind.plays_notes()).count() + 1;
                let t = Track::instrument(format!("Instrument {n}"));
                if let Some(i) = &t.instrument {
                    i.make_into(self.rate, &mut self.pending_instruments);
                }
                self.tracks.push(t);
                rebuild = true;
            }
            if !simple {
                // ai: add-bus
                if ui
                    .button("+ Add bus")
                    .on_hover_text(
                        "A bus: tracks can go to it (a group), or send a copy of their audio to \
                         it for an effect they share (a return)",
                    )
                    .clicked()
                {
                    self.add_bus(TrackKind::Bus);
                }
                // ai: add-folder
                if ui
                    .button("+ Add folder")
                    .on_hover_text(
                        "A folder: tracks put in it sit under it and go through its fader and \
                         effects",
                    )
                    .clicked()
                {
                    self.add_bus(TrackKind::Folder);
                }
                // ai: add-control-group
                if ui
                    .button("+ Add control group")
                    .on_hover_text(
                        "One fader and mute over several tracks, without summing their audio",
                    )
                    .clicked()
                {
                    self.add_control_group();
                }
            }
            rebuild |= self.input_tracks_button(ui, inputs);
            ui.separator();
            // ai: mixer-narrow
            ui.toggle_value(&mut self.desk.narrow, "Narrow strips")
                .on_hover_text("Only the fader, meter, pan, mute, solo and name, so more fit");
        });
        rebuild
    }

    /// What every strip's routing picks from.
    fn routing(&self) -> Routing {
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let folders: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind == TrackKind::Folder)
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        // Which buses (and folders) each track may go to without its
        // audio coming back round to it.
        let can = |list: &[(String, String)]| -> Vec<Vec<bool>> {
            (0..self.tracks.len())
                .map(|i| {
                    list.iter()
                        .map(|(k, _)| !self.feeds(k, &self.tracks[i].key))
                        .collect()
                })
                .collect()
        };
        Routing {
            inputs: self.input_count(),
            direct: self.profile.as_ref().is_some_and(|p| p.direct_monitoring),
            frozen: (self.tracks.iter())
                .map(|t| self.frozen_playing(t).is_some())
                .collect(),
            allowed: can(&buses),
            folders_ok: can(&folders),
            buses,
            folders,
            groups: self.groups.clone(),
        }
    }

    /// The routing parts a strip shows, top down.
    fn parts(&self, compact: bool) -> Vec<Part> {
        let mut parts = vec![Part::Source];
        if !compact {
            parts.extend([Part::Arm, Part::Fx]);
        }
        if !self.simple.on {
            parts.extend([Part::Out, Part::Sends]);
            if !self.groups.is_empty() {
                parts.push(Part::Group);
            }
            if self.tracks.iter().any(|t| t.kind == TrackKind::Folder) {
                parts.push(Part::Folder);
            }
        }
        parts.push(Part::Take);
        parts
    }

    /// Draws one part of track `i`'s strip. Returns true if the graph
    /// must be rebuilt.
    fn strip_part(
        &mut self,
        ui: &mut egui::Ui,
        i: usize,
        part: Part,
        r: &Routing,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut rebuild = false;
        let t = &mut self.tracks[i];
        let name = t.name.clone();
        match part {
            // Where its sound comes from: an input, an instrument, or for
            // a bus its name.
            Part::Source if t.kind.is_bus() => {
                let mut head = t.name.clone();
                // ai: edit_track
                let e = ui
                    .add(egui::TextEdit::singleline(&mut head).desired_width(desk::STRIP_W - 14.0))
                    .accessible_name(format!("Name of {name}"));
                if e.changed() {
                    t.name = head;
                }
            }
            Part::Source if t.kind.plays_notes() => {
                let open = self.instrument_open == Some(i);
                let label = t.instrument.as_ref().map_or(
                    if t.layers.is_empty() {
                        "Instrument"
                    } else {
                        "Layers"
                    },
                    |i| match i.kind() {
                        None | Some(daw_instruments::Kind::Synth) => "Synth",
                        Some(daw_instruments::Kind::Sampler) => "Sampler",
                        Some(daw_instruments::Kind::DrumSampler) => "Drum",
                        Some(daw_instruments::Kind::DrumMachine) => "Drums",
                        Some(daw_instruments::Kind::Slicer) => "Slicer",
                        Some(daw_instruments::Kind::PatchTwenty) => "Patch",
                        Some(daw_instruments::Kind::OhOne) => "Oh-One",
                        Some(daw_instruments::Kind::AcidThree) => "Acid",
                        Some(daw_instruments::Kind::ModelMono) => "Mono",
                        Some(daw_instruments::Kind::June) => "June",
                        Some(daw_instruments::Kind::Eighty) => "Eighty",
                    },
                );
                // ai: view
                if ui
                    .selectable_label(open, label)
                    .accessible_name(format!("Instrument on {name}"))
                    .on_hover_text(
                        "The instrument that plays this track's notes, and its MIDI output",
                    )
                    .clicked()
                {
                    self.instrument_open = (!open).then_some(i);
                }
                // An instrument that plays the track's input through
                // itself (a guitar through a synth's filters) takes an
                // input like an audio track; Monitor sends it in.
                if t.instrument.as_ref().is_some_and(|i| i.takes_input()) {
                    // ai: set_track_input
                    rebuild |= crate::input_picker(ui, i, &t.name, &mut t.input, r.inputs);
                }
            }
            Part::Source => {
                // ai: set_track_input
                rebuild |= crate::input_picker(ui, i, &t.name, &mut t.input, r.inputs);
            }
            // Arm, monitor and practice mute; for a bus, what it is.
            Part::Arm if t.kind.is_bus() => {
                ui.label(
                    RichText::new(match t.kind {
                        TrackKind::Folder => "folder",
                        _ => "bus",
                    })
                    .weak(),
                );
            }
            Part::Arm => {
                let record = theme::look(ui.ctx()).c().record;
                let direct = r.direct;
                ui.add_enabled_ui(t.input.is_some() || t.kind.plays_notes(), |ui| {
                    // ai: set_track_input
                    rebuild |= lit(ui, record, |ui| ui.toggle_value(&mut t.arm, "R"))
                        .accessible_name(format!("Arm {name} for recording"))
                        .on_hover_text(if t.kind.plays_notes() {
                            "Arm: record notes from MIDI devices (R records)"
                        } else {
                            "Arm for recording (R records)"
                        })
                        .changed();
                    // ai: set_track_input
                    let m = ui
                        .add_enabled(!direct, egui::Button::selectable(t.monitor, "M"))
                        .accessible_name(format!("Monitor {name}"));
                    if m.clicked() {
                        t.monitor = !t.monitor;
                        rebuild = true;
                    }
                    m.on_hover_text(if t.kind.plays_notes() {
                        "Monitor: play MIDI devices through this track's instrument"
                    } else {
                        "Monitor: hear the input through this track"
                    })
                    .on_disabled_hover_text("Direct monitoring is on (Settings → Audio device)");
                });
                if matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) {
                    ui.toggle_value(&mut t.practice_mute, "P")
                        .accessible_name(format!("Practice mute {name}"))
                        .on_hover_text(
                            "P: practice mute, to silence this part and play it yourself",
                        );
                }
            }
            // The insert effects.
            Part::Fx => {
                let fx = t.fx.len() + self.proj.missing_fx.get(&t.key).map_or(0, Vec::len);
                let label = match (fx, r.frozen[i]) {
                    (0, _) => "FX".to_string(),
                    (n, false) => format!("FX ({n})"),
                    // Plays its frozen audio.
                    (n, true) => format!("Frozen ({n})"),
                };
                let open = self.effects.open_track == Some(i);
                // ai: view
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
                    self.effects.open_master = false;
                }
            }
            Part::Out => rebuild |= output_picker(ui, i, t, &r.buses, &r.allowed[i]),
            Part::Sends => {
                if let Some(changed) = sends_menu(ui, i, t, &r.buses, &r.allowed[i]) {
                    if changed {
                        rebuild = true;
                    } else {
                        actions.push(Action::Sends(i));
                    }
                }
            }
            Part::Group => rebuild |= group_picker(ui, i, t, &r.groups),
            Part::Folder => {
                if let Some(p) = folder_picker(ui, i, t, &r.folders, &r.folders_ok[i]) {
                    actions.push(Action::Parent(i, p));
                }
            }
            // The take that plays, or for a bus, removing it.
            Part::Take if t.kind.is_bus() => {
                // ai: delete_track
                if ui
                    .button("Remove")
                    .accessible_name(format!("Remove {name}"))
                    .on_hover_text("Remove this bus; tracks on it go to the master")
                    .clicked()
                {
                    actions.push(Action::Remove(i));
                }
            }
            Part::Take => {
                if let Some(k) = crate::take_picker(ui, i, t) {
                    actions.push(Action::Take(i, k));
                }
            }
        }
        rebuild
    }

    /// Every track's and bus's strip. Returns true if the routing
    /// changed.
    fn track_strips(&mut self, ui: &mut egui::Ui, g: &Geometry, actions: &mut Vec<Action>) -> bool {
        let r = self.routing();
        let parts = self.parts(g.compact);
        let mut rebuild = false;
        let n = self.tracks.len();
        for i in 0..n {
            let rms = self.ctl.status().track_rms(i);
            let colour = theme::look(ui.ctx()).c().track(i);
            let selected = self.desk.selected.contains(&self.tracks[i].key);
            let before = self.tracks[i].heard();
            let name = self.tracks[i].name.clone();
            let key = self.tracks[i].key.clone();
            let (_, rect) = ui.allocate_space(vec2(g.width, g.height()));
            desk::paint_strip(ui, rect);
            let mut rows = Rows::new(rect, egui::Id::unique(("strip", &key)));
            if g.compact {
                // A short pane: the effects, with the rest of the routing
                // in a menu beside them, then arming.
                rows.row(ui, ROW, |ui| {
                    rebuild |= self.strip_part(ui, i, Part::Fx, &r, actions);
                    let menu = ui.menu_button("…", |ui| {
                        ui.set_min_width(200.0);
                        for &p in &parts {
                            ui.horizontal(|ui| {
                                ui.label(p.title());
                                rebuild |= self.strip_part(ui, i, p, &r, actions);
                            });
                        }
                    });
                    menu.response
                        .accessible_name(format!("Routing of {name}"))
                        .on_hover_text("Input, out, sends, group, folder and take");
                });
                rows.row(ui, ROW, |ui| {
                    rebuild |= self.strip_part(ui, i, Part::Arm, &r, actions);
                });
            } else if !g.narrow {
                for &p in &parts {
                    rows.row(ui, ROW, |ui| {
                        rebuild |= self.strip_part(ui, i, p, &r, actions);
                    });
                }
            }
            let t = &mut self.tracks[i];
            // The pan pot.
            rows.row(ui, PAN_ROW, |ui| {
                let first = spoken::start(ui);
                // ai: set_parameter
                desk::pan_pot(
                    ui,
                    egui::Id::unique(("pan", &key)),
                    &mut t.mix.pan,
                    &format!("{name} pan"),
                    30.0,
                );
                spoken::pan(ui, first, t.mix.pan as f64);
                if !g.narrow {
                    ui.label(RichText::new(desk::pan_short(t.mix.pan)).small().weak());
                }
            });
            // The fader, its scale and the meter beside it.
            let mut db = t.mix.gain_db as f64;
            let moved = rows.row(ui, g.travel + desk::CAP_H, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                desk::scale(ui, g.travel, g.narrow);
                let first = spoken::start(ui);
                // ai: set_parameter
                let moved = desk::fader(
                    ui,
                    egui::Id::unique(("fader", &key)),
                    &mut db,
                    g.travel,
                    &format!("{name} volume"),
                );
                spoken::level(ui, first, db);
                let reading = desk::Reading { peak: t.meter, rms };
                // ai: view
                let m = desk::meter(
                    ui,
                    egui::Id::unique(("meter", &key)),
                    g.travel,
                    reading,
                    &name,
                );
                if m.meter.clicked() {
                    self.meters.show(Some(key.clone()));
                }
                moved
            });
            // Its level as a number, to read or type.
            rows.row(ui, ROW, |ui| {
                let first = spoken::start(ui);
                // ai: set_parameter
                ui.name_inputs(format!("{name} level in dB"), |ui| {
                    ui.add_sized(
                        [g.width - 2.0 * INSET - 6.0, 18.0],
                        // ai: set_parameter
                        egui::DragValue::new(&mut db)
                            .range(desk::MIN_DB..=desk::MAX_DB)
                            .speed(0.1)
                            .max_decimals(1)
                            .custom_formatter(|v, _| {
                                if v <= desk::MIN_DB {
                                    "off".into()
                                } else {
                                    format!("{v:.1} dB")
                                }
                            }),
                    )
                });
                spoken::level(ui, first, db);
            });
            if db != t.mix.gain_db as f64 {
                let old = t.mix.gain_db;
                t.mix.gain_db = db as f32;
                if moved.dragged && selected {
                    actions.push(Action::Follow(key.clone(), t.mix.gain_db - old));
                }
            }
            // Mute and solo, lit when on.
            rows.row(ui, ROW, |ui| {
                let c = theme::look(ui.ctx()).c().clone();
                let (mute, solo) = if g.narrow {
                    ("M", "S")
                } else {
                    ("Mute", "Solo")
                };
                // ai: set_parameter
                lit(ui, c.warn, |ui| ui.toggle_value(&mut t.mix.mute, mute))
                    .accessible_name(format!("Mute {name}"));
                // ai: set_parameter
                lit(ui, c.good, |ui| ui.toggle_value(&mut t.mix.solo, solo))
                    .accessible_name(format!("Solo {name}"));
            });
            // The name tag: click to select.
            let tag = rows.row(ui, TAG_ROW, |ui| {
                // ai: view
                desk::scribble(
                    ui,
                    egui::Id::unique(("tag", &key)),
                    g.width - 2.0 * INSET,
                    &name,
                    colour,
                    Some(selected),
                )
            });
            if tag.clicked() {
                let add = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                actions.push(Action::Select(key.clone(), add));
            }
            if let Some(c) = t.clips.iter().find(|c| c.take.is_none()) {
                tag.on_hover_text(c.path.display().to_string());
            }
            if t.heard() != before {
                actions.push(Action::Mix(i, t.heard()));
            }
        }
        rebuild
    }

    /// The master strip: the whole mix's effects, fader and meter.
    fn master_strip(&mut self, ui: &mut egui::Ui, g: &Geometry, actions: &mut Vec<Action>) {
        let (_, rect) = ui.allocate_space(vec2(g.width, g.height()));
        desk::paint_strip(ui, rect);
        let mut rows = Rows::new(rect, egui::Id::unique("master strip"));
        if !g.narrow {
            if !g.compact {
                rows.row(ui, ROW, |ui| {
                    ui.label(RichText::new("Master").strong());
                });
            }
            rows.row(ui, ROW, |ui| {
                // ai: view
                if ui.button("Meters…").clicked() {
                    self.meters.show(None);
                }
            });
            rows.row(ui, ROW, |ui| {
                let fx = self.master.fx.len()
                    + (self.proj.missing_fx.get(daw_project::Master::KEY)).map_or(0, Vec::len);
                let label = match fx {
                    0 => "FX".to_string(),
                    n => format!("FX ({n})"),
                };
                let open = self.effects.open_master;
                // ai: view
                if ui
                    .selectable_label(open, label)
                    .accessible_name("Effects on the master")
                    .on_hover_text(if self.master.fx.is_empty() {
                        "Effects on the whole mix, after every track and bus".to_string()
                    } else {
                        (self.master.fx.iter())
                            .map(|f| f.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" → ")
                    })
                    .clicked()
                {
                    self.effects.open_master = !open;
                    self.effects.open_track = None;
                }
            });
            rows.skip(g.rows.saturating_sub(3) as f32 * ROW);
        }
        rows.skip(PAN_ROW);
        let mut db = self.master.mix.gain_db as f64;
        let rms = self.ctl.status().master_rms();
        let reading = desk::Reading {
            peak: self.master_meter,
            rms,
        };
        rows.row(ui, g.travel + desk::CAP_H, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            desk::scale(ui, g.travel, g.narrow);
            let first = spoken::start(ui);
            // ai: set_parameter
            desk::fader(
                ui,
                egui::Id::unique("master fader"),
                &mut db,
                g.travel,
                "Master volume",
            );
            spoken::level(ui, first, db);
            // ai: view
            let m = desk::meter(
                ui,
                egui::Id::unique("master meter"),
                g.travel,
                reading,
                "Master",
            );
            if m.meter.clicked() {
                self.meters.show(None);
            }
        });
        rows.row(ui, ROW, |ui| {
            let first = spoken::start(ui);
            // ai: set_parameter
            ui.name_inputs("Master level in dB", |ui| {
                ui.add_sized(
                    [g.width - 2.0 * INSET - 6.0, 18.0],
                    // ai: set_parameter
                    egui::DragValue::new(&mut db)
                        .range(desk::MIN_DB..=desk::MAX_DB)
                        .speed(0.1)
                        .max_decimals(1)
                        .suffix(" dB"),
                )
            });
            spoken::level(ui, first, db);
        });
        if db != self.master.mix.gain_db as f64 {
            self.master.mix.gain_db = db as f32;
            actions.push(Action::MasterGain(self.master.mix.gain_db));
        }
        rows.skip(ROW);
        let accent = theme::look(ui.ctx()).c().accent;
        rows.row(ui, TAG_ROW, |ui| {
            desk::scribble(
                ui,
                egui::Id::unique("master tag"),
                g.width - 2.0 * INSET,
                "Master",
                accent,
                None,
            );
        });
    }

    /// A control group's strip: its name, fader and mute.
    fn group_strip(
        &mut self,
        ui: &mut egui::Ui,
        g: &Geometry,
        gi: usize,
        actions: &mut Vec<Action>,
    ) {
        let (_, rect) = ui.allocate_space(vec2(g.width, g.height()));
        desk::paint_strip(ui, rect);
        let grp = &mut self.groups[gi];
        let before = (grp.gain_db, grp.mute);
        let id = grp.id.clone();
        let mut rows = Rows::new(rect, egui::Id::unique(("group strip", &id)));
        if !g.narrow {
            rows.row(ui, ROW, |ui| {
                // ai: edit_control_group
                ui.add(egui::TextEdit::singleline(&mut grp.name).desired_width(g.width - 14.0))
                    .accessible_name(format!("Name of control group {}", gi + 1));
            });
            if !g.compact {
                rows.row(ui, ROW, |ui| {
                    ui.label(RichText::new("control group").weak().small());
                });
            }
            rows.row(ui, ROW, |ui| {
                // ai: edit_control_group
                if ui
                    .button("Remove")
                    .accessible_name(format!("Remove {}", grp.name))
                    .clicked()
                {
                    actions.push(Action::RemoveGroup(gi));
                }
            });
            rows.skip(g.rows.saturating_sub(3) as f32 * ROW);
        }
        rows.skip(PAN_ROW);
        let name = grp.name.clone();
        rows.row(ui, g.travel + desk::CAP_H, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            desk::scale(ui, g.travel, g.narrow);
            let first = spoken::start(ui);
            // ai: edit_control_group
            desk::fader(
                ui,
                egui::Id::unique(("group fader", &id)),
                &mut grp.gain_db,
                g.travel,
                &format!("{name} volume"),
            );
            spoken::level(ui, first, grp.gain_db);
        });
        rows.row(ui, ROW, |ui| {
            let first = spoken::start(ui);
            // ai: edit_control_group
            ui.name_inputs(format!("{name} level in dB"), |ui| {
                ui.add_sized(
                    [g.width - 2.0 * INSET - 6.0, 18.0],
                    // ai: edit_control_group
                    egui::DragValue::new(&mut grp.gain_db)
                        .range(desk::MIN_DB..=desk::MAX_DB)
                        .speed(0.1)
                        .max_decimals(1)
                        .suffix(" dB"),
                )
            });
            spoken::level(ui, first, grp.gain_db);
        });
        rows.row(ui, ROW, |ui| {
            let warn = theme::look(ui.ctx()).c().warn;
            // ai: edit_control_group
            lit(ui, warn, |ui| {
                ui.toggle_value(&mut grp.mute, if g.narrow { "M" } else { "Mute" })
            })
            .accessible_name(format!("Mute {name}"));
        });
        let dim = theme::look(ui.ctx()).c().text_dim;
        rows.row(ui, TAG_ROW, |ui| {
            desk::scribble(
                ui,
                egui::Id::unique(("group tag", &id)),
                g.width - 2.0 * INSET,
                &name,
                dim,
                None,
            );
        });
        if (grp.gain_db, grp.mute) != before {
            actions.push(Action::Group(gi));
        }
    }

    /// Does what the strips asked. Returns true if the graph must be
    /// rebuilt.
    fn desk_actions(&mut self, actions: Vec<Action>) -> bool {
        let mut rebuild = false;
        for a in actions {
            match a {
                Action::Mix(track, mix) => self.send(Command::SetMix { track, mix }),
                Action::MasterGain(db) => self.send(Command::SetMasterGain(db)),
                Action::Sends(i) => {
                    let sends: Vec<(f32, bool)> = (self.sends_of(&self.tracks[i]).iter())
                        .map(|(s, _)| (s.gain_db as f32, s.mute))
                        .collect();
                    for (send, (gain_db, mute)) in sends.into_iter().enumerate() {
                        self.send(Command::SetSend {
                            track: i,
                            send,
                            gain_db,
                            mute,
                        });
                    }
                }
                Action::Group(group) => {
                    let g = &self.groups[group];
                    let spec = GroupSpec {
                        gain_db: g.gain_db as f32,
                        mute: g.mute,
                    };
                    self.send(Command::SetGroup { group, spec });
                }
                Action::Take(i, k) => {
                    let rate = self.rate;
                    self.tracks[i].takes(rate).choose_take(k);
                    rebuild = true;
                }
                Action::Parent(i, parent) => {
                    self.put_in_folder(i, parent);
                    rebuild = true;
                }
                Action::Remove(i) => {
                    self.remove_bus(i);
                    rebuild = true;
                }
                Action::RemoveGroup(gi) => {
                    let id = self.groups.remove(gi).id;
                    for t in &mut self.tracks {
                        if t.group.as_deref() == Some(&id) {
                            t.group = None;
                        }
                    }
                    rebuild = true;
                }
                Action::Follow(key, by) => self.follow_fader(&key, by),
                Action::Select(key, add) => {
                    let sel = &mut self.desk.selected;
                    if !add {
                        let only = sel.len() == 1 && sel[0] == key;
                        sel.clear();
                        if !only {
                            sel.push(key);
                        }
                    } else if let Some(at) = sel.iter().position(|k| *k == key) {
                        sel.remove(at);
                    } else {
                        sel.push(key);
                    }
                }
            }
        }
        rebuild
    }

    /// Moves the faders of the selected strips other than `key` by `by`
    /// dB, keeping their levels apart as they were.
    pub(crate) fn follow_fader(&mut self, key: &str, by: f32) {
        let keys = self.desk.selected.clone();
        for k in keys.iter().filter(|k| *k != key) {
            let Some(i) = self.track_index(k) else {
                continue;
            };
            let t = &mut self.tracks[i];
            t.mix.gain_db = (t.mix.gain_db + by).clamp(desk::MIN_DB as f32, desk::MAX_DB as f32);
            let mix = t.heard();
            self.send(Command::SetMix { track: i, mix });
        }
    }

    /// Whether audio from the track `from` reaches the track `to`, by
    /// its output, sends or folder, directly or through other buses.
    pub(crate) fn feeds(&self, from: &str, to: &str) -> bool {
        let mut seen = Vec::new();
        let mut todo = vec![from.to_string()];
        while let Some(k) = todo.pop() {
            if k == to {
                return true;
            }
            if seen.contains(&k) {
                continue;
            }
            seen.push(k.clone());
            if let Some(t) = self.tracks.iter().find(|t| t.key == k) {
                todo.extend(t.goes_to().map(str::to_string));
                todo.extend(t.sends.iter().map(|s| s.to.clone()));
                todo.extend(t.instrument_outputs.iter().map(|o| o.to.clone()));
            }
        }
        false
    }

    /// Puts track `i` in a folder (or takes it out), and moves it to sit
    /// under the folder, after the tracks already in it.
    fn put_in_folder(&mut self, i: usize, parent: Option<String>) {
        let mut t = self.tracks.remove(i);
        t.parent = parent.clone();
        let at = match parent.as_deref().and_then(|p| self.track_index(p)) {
            Some(f) => {
                let mut at = f + 1;
                while at < self.tracks.len() && self.tracks[at].parent == parent {
                    at += 1;
                }
                at
            }
            // Out of any folder: after the last track.
            None => self.tracks.len(),
        };
        self.tracks.insert(at, t);
        self.effects.open_track = None;
    }

    /// Removes a bus or folder: what went to it goes to the master, and
    /// the folder's tracks come out of it.
    fn remove_bus(&mut self, i: usize) {
        if !self.tracks[i].kind.is_bus() {
            return;
        }
        let key = self.tracks.remove(i).key;
        for t in &mut self.tracks {
            if t.parent.as_deref() == Some(&key) {
                t.parent = None;
            }
            if t.output.as_deref() == Some(&key) {
                t.output = None;
            }
            t.sends.retain(|s| s.to != key);
            t.instrument_outputs.retain(|o| o.to != key);
            for f in &mut t.fx {
                if f.sidechain.as_deref() == Some(&key) {
                    f.sidechain = None;
                }
            }
        }
        self.effects.open_track = None;
    }
}

/// Where the track's output goes. Returns true if it changed.
fn output_picker(
    ui: &mut egui::Ui,
    i: usize,
    t: &mut Track,
    buses: &[(String, String)],
    allowed: &[bool],
) -> bool {
    let name = |k: &Option<String>| match k {
        None if t.parent.is_some() => "Folder".to_string(),
        None => "Master".to_string(),
        Some(k) => buses
            .iter()
            .find(|(key, _)| key == k)
            .map_or("Master".into(), |(_, n)| n.clone()),
    };
    let before = t.output.clone();
    egui::ComboBox::from_id_salt(("output", i))
        .width(76.0)
        .selected_text(name(&t.output))
        .show_ui(ui, |ui| {
            // ai: edit_track
            ui.selectable_value(&mut t.output, None, name(&None));
            for ((key, n), ok) in buses.iter().zip(allowed) {
                if *key != t.key && *ok {
                    // ai: edit_track
                    ui.selectable_value(&mut t.output, Some(key.clone()), n);
                }
            }
        })
        .response
        .accessible_name(format!("Output of {}", t.name))
        .on_hover_text("Where this track's audio goes after its fader");
    t.output != before
}

/// The track's sends, as a menu. Returns `Some(true)` if a send was added
/// or removed or changed pre/post (the graph must be rebuilt),
/// `Some(false)` if only levels or mutes changed, `None` for nothing.
fn sends_menu(
    ui: &mut egui::Ui,
    i: usize,
    t: &mut Track,
    buses: &[(String, String)],
    allowed: &[bool],
) -> Option<bool> {
    let label = match t.sends.len() {
        0 => "Sends".to_string(),
        n => format!("Sends ({n})"),
    };
    let mut result = None;
    let menu = ui.menu_button(label, |ui| {
        ui.set_min_width(260.0);
        let mut remove = None;
        for (si, s) in t.sends.iter_mut().enumerate() {
            let to = buses
                .iter()
                .find(|(k, _)| *k == s.to)
                .map_or("Missing bus", |(_, n)| n.as_str());
            ui.horizontal(|ui| {
                ui.label(RichText::new(to).strong());
                let before = (s.gain_db, s.mute, s.pre_fader);
                let first = spoken::start(ui);
                ui.add(
                    // ai: set_send
                    egui::Slider::new(&mut s.gain_db, -60.0..=12.0)
                        .suffix(" dB")
                        .step_by(0.1),
                )
                .accessible_name(format!("Send to {to} level"));
                spoken::level(ui, first, s.gain_db);
                // ai: set_send
                ui.toggle_value(&mut s.pre_fader, "Pre")
                    .accessible_name(format!("Send to {to} before the fader"))
                    .on_hover_text("Pre-fader: the track's volume leaves this send alone");
                // ai: set_send
                ui.toggle_value(&mut s.mute, "Mute")
                    .accessible_name(format!("Mute send to {to}"));
                if ui
                    // ai: set_send
                    .button("✕")
                    .accessible_name(format!("Remove send to {to}"))
                    .clicked()
                {
                    remove = Some(si);
                }
                if (s.gain_db, s.mute, s.pre_fader) != before {
                    let structural = s.pre_fader != before.2;
                    result = Some(result.unwrap_or(false) || structural);
                }
            });
        }
        if let Some(si) = remove {
            t.sends.remove(si);
            result = Some(true);
        }
        let free: Vec<&(String, String)> = (buses.iter().zip(allowed))
            .filter(|((k, _), ok)| **ok && *k != t.key && !t.sends.iter().any(|s| s.to == *k))
            .map(|(b, _)| b)
            .collect();
        if free.is_empty() {
            if buses.is_empty() {
                ui.label(RichText::new("Add a bus to send to.").weak());
            }
        } else {
            ui.menu_button("Add send to…", |ui| {
                for (k, n) in free {
                    // ai: set_send
                    if ui.button(n).clicked() {
                        t.sends.push(Send::to(k.clone()));
                        result = Some(true);
                        ui.close();
                    }
                }
            });
        }
    });
    menu.response
        .accessible_name(format!("Sends from {}", t.name))
        .on_hover_text("Copies of this track's audio sent to buses, with their own levels");
    let _ = i;
    result
}

/// The control group the track belongs to. Returns true if it changed.
fn group_picker(ui: &mut egui::Ui, i: usize, t: &mut Track, groups: &[ControlGroup]) -> bool {
    if groups.is_empty() {
        ui.label("");
        return false;
    }
    let name = |k: &Option<String>| match k {
        None => "None".to_string(),
        Some(k) => groups
            .iter()
            .find(|g| g.id == *k)
            .map_or("None".into(), |g| g.name.clone()),
    };
    let before = t.group.clone();
    egui::ComboBox::from_id_salt(("group", i))
        .width(76.0)
        .selected_text(name(&t.group))
        .show_ui(ui, |ui| {
            // ai: edit_track
            ui.selectable_value(&mut t.group, None, "None");
            for g in groups {
                // ai: edit_track
                ui.selectable_value(&mut t.group, Some(g.id.clone()), &g.name);
            }
        })
        .response
        .accessible_name(format!("Control group of {}", t.name))
        .on_hover_text("The control group whose fader and mute act on this track");
    t.group != before
}

/// The folder the track sits in. Returns the choice if it changed.
fn folder_picker(
    ui: &mut egui::Ui,
    i: usize,
    t: &Track,
    folders: &[(String, String)],
    allowed: &[bool],
) -> Option<Option<String>> {
    let name = |k: &Option<String>| match k {
        None => "No folder".to_string(),
        Some(k) => folders
            .iter()
            .find(|(key, _)| key == k)
            .map_or("No folder".into(), |(_, n)| n.clone()),
    };
    let mut pick = t.parent.clone();
    egui::ComboBox::from_id_salt(("folder", i))
        .width(76.0)
        .selected_text(name(&pick))
        .show_ui(ui, |ui| {
            // ai: edit_track
            ui.selectable_value(&mut pick, None, "No folder");
            for ((k, n), ok) in folders.iter().zip(allowed) {
                if *k != t.key && *ok {
                    // ai: edit_track
                    ui.selectable_value(&mut pick, Some(k.clone()), n);
                }
            }
        })
        .response
        .accessible_name(format!("Folder of {}", t.name))
        .on_hover_text(
            "The folder this track sits in; it goes to the folder unless routed elsewhere",
        );
    (pick != t.parent).then_some(pick)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use daw_engine::graph::Output;
    use daw_project::{ProjectDoc, ProjectFolder, TrackKind};
    use eframe::egui;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::App;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-mixer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    /// A few frames, so the grid and the document settle.
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

    /// A second of a tone, mono, at `rate`.
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
    fn routing_reaches_the_engine_and_survives_saving_and_undo() {
        let dir = scratch("routing");
        let mut h = harness();
        let rate = h.state().rate;
        h.state_mut().add_file(&tone(&dir, rate));
        until(&mut h, |a| !a.tracks.is_empty());

        // A bus, a folder and a control group, from the mixer's buttons.
        h.get_by_label("+ Add bus").click();
        settle(&mut h);
        h.get_by_label("+ Add folder").click();
        settle(&mut h);
        h.get_by_label("+ Add control group").click();
        settle(&mut h);
        let app = h.state_mut();
        assert_eq!(app.tracks[1].kind, TrackKind::Bus);
        assert_eq!(app.tracks[2].kind, TrackKind::Folder);
        assert_eq!(app.groups.len(), 1);
        let (bus, folder, group) = (
            app.tracks[1].key.clone(),
            app.tracks[2].key.clone(),
            app.groups[0].id.clone(),
        );
        // The guitar in the folder, sending to the bus, in the group.
        app.put_in_folder(0, Some(folder.clone()));
        let guitar = (app.tracks.iter_mut())
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap();
        guitar.sends.push(daw_project::Send {
            to: bus.clone(),
            gain_db: -6.0,
            pre_fader: true,
            mute: false,
        });
        guitar.group = Some(group.clone());
        app.rebuild();
        settle(&mut h);

        // The folder comes first, with its track under it.
        let app = h.state();
        let names: Vec<(&str, TrackKind)> = (app.tracks.iter())
            .map(|t| (t.key.as_str(), t.kind))
            .collect();
        assert_eq!(names[1].1, TrackKind::Folder);
        assert_eq!(names[2].1, TrackKind::Audio);
        let s = app.session();
        let bus_at = app.track_index(&bus).unwrap();
        let folder_at = app.track_index(&folder).unwrap();
        let g = &s.tracks[2];
        assert_eq!(g.output, Output::Track(folder_at), "goes to its folder");
        assert_eq!(g.sends.len(), 1);
        assert_eq!(g.sends[0].to, bus_at);
        assert!(g.sends[0].pre_fader);
        assert_eq!(g.group, Some(0));
        assert_eq!(s.groups.len(), 1);
        assert!(s.compile(256).is_ok());

        // Saved and reopened: the same routing.
        let song = dir.join("Song");
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &song).unwrap();
        settle(&mut h);
        let (_, bytes) = ProjectFolder::open(&song).unwrap();
        let saved = ProjectDoc::from_bytes(&bytes, 1).unwrap().get();
        assert_eq!(saved.control_groups.len(), 1);
        let st = saved
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap();
        assert_eq!(st.parent.as_deref(), Some(folder.as_str()));
        assert_eq!(st.sends[0].to, bus);
        assert_eq!(st.group.as_deref(), Some(group.as_str()));

        // Taking the send off is one undo step.
        let app = h.state_mut();
        let guitar = (app.tracks.iter_mut())
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap();
        guitar.sends.clear();
        app.rebuild();
        settle(&mut h);
        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        settle(&mut h);
        let app = h.state();
        let guitar = app
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap();
        assert_eq!(guitar.sends.len(), 1, "the send is back");
        // Saved on the way out, as the app does when closing. Without
        // this, on a slow run, this computer's shared copy can hold
        // the cleared send (written two seconds after the save) while
        // the undo was never shared, and reopening merges it back in.
        let ctx = h.ctx.clone();
        h.state_mut().save(&ctx).unwrap();
        settle(&mut h);
        drop(h);

        // Reopened: still routed, and the engine takes it.
        let mut h = harness();
        h.state_mut().open_project(song.clone());
        until(&mut h, |a| a.tracks.len() == 3 && !a.proj.loading());
        let app = h.state();
        let s = app.session();
        let guitar_at = app
            .tracks
            .iter()
            .position(|t| t.kind == TrackKind::Audio)
            .unwrap();
        assert_eq!(s.tracks[guitar_at].sends.len(), 1);
        assert!(matches!(s.tracks[guitar_at].output, Output::Track(_)));
        assert!(s.compile(256).is_ok());
    }

    #[test]
    fn removing_a_bus_sends_its_tracks_to_the_master() {
        let mut h = harness();
        // Tall enough for the strips to show their routing.
        h.state_mut().workspace.layout = crate::workspace::Layout::mixing();
        settle(&mut h);
        h.get_by_label("Or add an empty track to record on").click();
        settle(&mut h);
        h.get_by_label("+ Add bus").click();
        settle(&mut h);
        let app = h.state_mut();
        let bus = app.tracks[1].key.clone();
        app.tracks[0].output = Some(bus.clone());
        app.tracks[0].sends.push(daw_project::Send::to(bus.clone()));
        app.rebuild();
        settle(&mut h);
        h.get_by_label("Remove Bus 1").click();
        settle(&mut h);
        let app = h.state();
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.tracks[0].output, None);
        assert!(app.tracks[0].sends.is_empty());
        assert_eq!(app.session().tracks[0].output, Output::Master);
    }

    #[test]
    fn a_bus_cannot_be_routed_back_into_what_feeds_it() {
        let mut h = harness();
        h.get_by_label("Or add an empty track to record on").click();
        settle(&mut h);
        h.get_by_label("+ Add bus").click();
        settle(&mut h);
        h.get_by_label("+ Add bus").click();
        settle(&mut h);
        let app = h.state_mut();
        let (a, b) = (app.tracks[1].key.clone(), app.tracks[2].key.clone());
        app.tracks[1].output = Some(b.clone());
        assert!(app.feeds(&a, &b));
        assert!(!app.feeds(&b, &a));
        // Even if the document said so, the engine refuses the loop and
        // the app says why, rather than playing nothing silently.
        app.tracks[2].sends.push(daw_project::Send::to(a.clone()));
        let err = app.session().compile(256).err().unwrap();
        assert!(err.to_string().contains("come back round"), "{err}");
    }

    /// A bus's stem holds what goes through it and nothing else: the
    /// mix with the bus muted, plus its stem, is the mix.
    #[test]
    fn a_bus_stem_is_its_part_of_the_mix() {
        use std::sync::Arc;

        use daw_engine::engine::render_offline;
        use daw_engine::graph::{AudioData, ClipSpec, SendSpec, Session, TrackSpec};
        use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

        let rate = 48_000;
        let clip = |level: f32| {
            ClipSpec::new(
                Ticks(0),
                Arc::new(AudioData {
                    channels: vec![vec![level; 4096]],
                    rate,
                }),
            )
        };
        let mut s = Session::new(TempoMap::new(rate, Tempo::bpm(120), TimeSignature::COMMON));
        // 0 goes into the bus (1); 2 goes to the master and sends to the
        // bus; 3 goes to the master only.
        let mut into = TrackSpec::new(vec![clip(0.1)]);
        into.output = Output::Track(1);
        let mut bus = TrackSpec::new(Vec::new());
        bus.mix.gain_db = -6.0;
        let mut sends = TrackSpec::new(vec![clip(0.2)]);
        sends.sends.push(SendSpec::to(1));
        let direct = TrackSpec::new(vec![clip(0.4)]);
        s.tracks = vec![into, bus, sends, direct];

        let render = |s: &Session| render_offline(s.compile(256).unwrap(), Frames(0), 2048, 256);
        let mix = render(&s);
        let stem = render(&super::through_bus(&s, 1));
        let mut without = s.clone();
        without.tracks[1].mix.mute = true;
        let rest = render(&without);
        for ch in 0..2 {
            for i in 0..2048 {
                let sum = stem[ch][i] + rest[ch][i];
                assert!((sum - mix[ch][i]).abs() < 1e-5, "channel {ch} frame {i}");
            }
        }
        assert!(stem[0][1000] > 0.01, "the bus sounds");

        // With nothing going into the bus, its stem is silent: the track
        // that only goes to the master is left out.
        let mut only_direct = s.clone();
        only_direct.tracks[0].clips.clear();
        only_direct.tracks[2].clips.clear();
        let stem = render(&super::through_bus(&only_direct, 1));
        assert!(stem.iter().flatten().all(|x| x.abs() < 1e-6));
    }

    /// The desk end to end, as an assistant and a person share it: what
    /// an assistant sets shows on the fader and is read out; the fader,
    /// dragged or stepped from the keyboard, is what the assistant then
    /// reads (the test harness spreads clicks over frames, so it can't
    /// double-click); selected strips move together; and the
    /// narrow strips are a command.
    #[test]
    fn the_desk_and_an_assistant_work_the_same_faders() {
        use egui::Key;
        use egui::accesskit::Role;
        use egui_kittest::Node;
        use egui_kittest::kittest::NodeT;
        use serde_json::json;

        use crate::agent::driver::Driver;

        let mut d = Driver::new();
        let id = |v: serde_json::Value| v["track"].as_str().unwrap().to_string();
        let a = id(d.call("add_track", json!({"kind": "audio"})).unwrap());
        let b = id(d.call("add_track", json!({"kind": "audio"})).unwrap());
        d.app().workspace.layout = crate::workspace::Layout::mixing();
        d.h.run();
        let gain = |d: &mut Driver, t: &str| -> f64 {
            let p = d.call("get_project", json!({"track": t})).unwrap();
            p["tracks"][0]["gain_db"].as_f64().unwrap()
        };
        fn slider<'a>(h: &'a Harness<'static, App>, name: &'a str) -> Node<'a> {
            h.query_all_by_label(name)
                .find(|n| n.accesskit_node().role() == Role::Slider)
                .unwrap_or_else(|| panic!("no fader named {name}"))
        }

        // An assistant sets a level: the fader shows it, in words.
        d.call(
            "set_parameter",
            json!({"track": a, "param": "gain", "value": -6.0}),
        )
        .unwrap();
        d.h.run();
        let f = slider(&d.h, "Audio 1 volume");
        assert_eq!(f.accesskit_node().value().as_deref(), Some("-6.0 dB"));

        // Dragged up, it rises from where it was (no jump to the
        // pointer), and the assistant reads the new level.
        let r = slider(&d.h, "Audio 1 volume").rect();
        let from = egui::pos2(r.center().x, r.bottom() - 4.0);
        d.h.drag_at(from);
        d.h.run();
        for i in 1..=4 {
            d.h.hover_at(from - egui::vec2(0.0, i as f32 * 5.0));
            d.h.run();
        }
        d.h.drop_at(from - egui::vec2(0.0, 20.0));
        d.h.run();
        let up = gain(&mut d, &a);
        assert!(up > -6.0 && up < 0.0, "pushed up a little from -6 dB: {up}");
        assert_eq!(
            d.app().session().tracks[0].mix.gain_db as f64,
            up,
            "and the engine has it"
        );

        // From the keyboard: Up raises it half a dB.
        slider(&d.h, "Audio 1 volume").focus();
        d.h.run();
        d.h.key_press(Key::ArrowUp);
        d.h.run();
        assert!((gain(&mut d, &a) - (up + 0.5)).abs() < 1e-6);

        // Two strips selected by their name tags: dragging one fader
        // moves the other by as much.
        d.call(
            "set_parameter",
            json!({"track": b, "param": "gain", "value": -10.0}),
        )
        .unwrap();
        d.h.get_by_label("Select Audio 1").click();
        d.h.run();
        d.h.get_by_label("Select Audio 2")
            .click_modifiers(egui::Modifiers::COMMAND);
        d.h.run();
        let sel = d.call("get_selection", json!({})).unwrap();
        assert_eq!(sel["mixer_strips"], json!([a, b]));
        let a0 = gain(&mut d, &a);
        let r = slider(&d.h, "Audio 1 volume").rect();
        let from = r.center();
        d.h.drag_at(from);
        d.h.run();
        d.h.hover_at(from + egui::vec2(0.0, 10.0));
        d.h.run();
        d.h.drop_at(from + egui::vec2(0.0, 30.0));
        d.h.run();
        let (ga, gb) = (gain(&mut d, &a), gain(&mut d, &b));
        assert!(ga < a0, "pulled down: {ga}");
        assert!(
            (gb - (-10.0 + ga - a0)).abs() < 1e-4,
            "followed by as much: {gb} vs {ga} from {a0}"
        );

        // Narrow strips, from the command an assistant runs.
        let wide = slider(&d.h, "Audio 1 volume").rect().center().x;
        d.command("mixer-narrow");
        d.h.run();
        assert_eq!(
            d.call("get_selection", json!({})).unwrap()["mixer_narrow"],
            true
        );
        let x = slider(&d.h, "Audio 1 volume").rect().center().x;
        assert!(x < wide, "the strip is narrower");
        assert!(
            d.h.query_by_label("Sends from Audio 1").is_none(),
            "routing hidden"
        );
    }
}
