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
use eframe::egui::{self, RichText, vec2};

use crate::{App, Track, meters, theme};

/// Takes a track spec out of its routing, for rendering it on its own
/// (a stem, or frozen audio): straight to the master, with no sends,
/// control group or sidechains.
pub(crate) fn alone(spec: &mut TrackSpec) {
    spec.output = Output::Master;
    spec.sends.clear();
    spec.group = None;
    for i in &mut spec.inserts {
        i.sidechain = None;
    }
}

/// The session as it sounds coming out of bus `bus` alone, for that
/// bus's stem: everything routed or sent into it (directly or through
/// other buses), through its effects and fader, and nothing else. Every
/// other track that went to the master goes instead to a muted bus
/// added at the end, so its sends into this bus still sound.
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
    /// A track's send levels, as the engine numbers them.
    Sends(usize),
    Group(usize),
    Take(usize, Option<usize>),
    /// Put the track in this folder (or none), and move it under it.
    Parent(usize, Option<String>),
    Remove(usize),
    RemoveGroup(usize),
}

impl App {
    pub(crate) fn mixer(&mut self, ui: &mut egui::Ui) {
        if self.tracks.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Drop audio or video files here to add tracks.").size(18.0));
                ui.add_space(8.0);
                if ui.button("Or add an empty track to record on").clicked() {
                    self.tracks.push(Track::new("Track 1".into()));
                    self.rebuild();
                }
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
                        self.pending_instruments.push((i.id, i.make(self.rate)));
                    }
                    self.tracks.push(t);
                    self.rebuild();
                }
            });
            return;
        }
        let inputs = self.input_count();
        let direct = self.profile.as_ref().is_some_and(|p| p.direct_monitoring);
        let frozen: Vec<bool> = (self.tracks.iter())
            .map(|t| self.frozen_playing(t).is_some())
            .collect();
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let folders: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind == TrackKind::Folder)
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        // Which buses each track may go to without its audio coming
        // back round to it.
        let allowed: Vec<Vec<bool>> = (0..self.tracks.len())
            .map(|i| {
                buses
                    .iter()
                    .map(|(k, _)| !self.feeds(k, &self.tracks[i].key))
                    .collect()
            })
            .collect();
        // Which folders each track may sit in, likewise.
        let folders_ok: Vec<Vec<bool>> = (0..self.tracks.len())
            .map(|i| {
                folders
                    .iter()
                    .map(|(k, _)| !self.feeds(k, &self.tracks[i].key))
                    .collect()
            })
            .collect();
        let mut actions = Vec::new();
        let mut rebuild = false;
        let groups = self.groups.clone();
        egui::Grid::new("tracks")
            .num_columns(13)
            .spacing(vec2(10.0, 10.0))
            .striped(true)
            .show(ui, |ui| {
                for h in [
                    "Track", "Input", "", "Level", "Volume", "Pan", "", "", "Effects", "Out",
                    "Sends", "Group", "Take",
                ] {
                    ui.strong(h);
                }
                ui.end_row();
                let n = self.tracks.len();
                for i in 0..n {
                    let (before, mut head) = {
                        let t = &self.tracks[i];
                        (t.heard(), t.name.clone())
                    };
                    let t = &mut self.tracks[i];
                    let name = t.name.clone();
                    ui.horizontal(|ui| {
                        if t.parent.is_some() {
                            ui.add_space(14.0);
                        }
                        // The track's colour, as on its lane.
                        let (tag, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                        let c = theme::look(ui.ctx()).c().track(i);
                        ui.painter().rect_filled(tag, 2.0, c);
                        if t.kind.is_bus() {
                            let r = ui
                                .add(egui::TextEdit::singleline(&mut head).desired_width(90.0))
                                .accessible_name(format!("Name of {name}"));
                            if r.changed() {
                                t.name = head.clone();
                            }
                            ui.label(
                                RichText::new(match t.kind {
                                    TrackKind::Folder => "folder",
                                    _ => "bus",
                                })
                                .weak(),
                            );
                        } else {
                            let label = ui.label(&t.name);
                            if let Some(c) = t.clips.iter().find(|c| c.take.is_none()) {
                                label.on_hover_text(c.path.display().to_string());
                            }
                        }
                    });
                    if t.kind.is_bus() {
                        // No input, arming or monitoring on a bus.
                        ui.label("");
                        ui.label("");
                    } else {
                        if t.kind.plays_notes() {
                            let open = self.instrument_open == Some(i);
                            let label = t.instrument.as_ref().map_or("Instrument", |_| "Synth");
                            if ui
                                .selectable_label(open, label)
                                .accessible_name(format!("Instrument on {name}"))
                                .on_hover_text("The instrument that plays this track's notes, and its MIDI output")
                                .clicked()
                            {
                                self.instrument_open = (!open).then_some(i);
                            }
                        } else {
                            rebuild |= crate::input_picker(ui, i, &t.name, &mut t.input, inputs);
                        }
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(t.input.is_some() || t.kind.plays_notes(), |ui| {
                                let arm = RichText::new("R").color(if t.arm {
                                    theme::look(ui.ctx()).c().record
                                } else {
                                    ui.visuals().text_color()
                                });
                                rebuild |= ui
                                    .toggle_value(&mut t.arm, arm)
                                    .accessible_name(format!("Arm {name} for recording"))
                                    .on_hover_text(if t.kind.plays_notes() {
                                        "Arm: record notes from MIDI devices (R records)"
                                    } else {
                                        "Arm for recording (R records)"
                                    })
                                    .changed();
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
                                    .on_disabled_hover_text(
                                        "Direct monitoring is on (Settings → Audio device)",
                                    );
                            });
                        });
                    }
                    let rms = self.ctl.status().track_rms(i);
                    if meters::strip(ui, t.meter, rms)
                        .on_hover_text("Peak and RMS. Click for the meters")
                        .clicked()
                    {
                        self.meters.show(Some(t.key.clone()));
                    }
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
                        if t.kind == TrackKind::Audio {
                            ui.toggle_value(&mut t.practice_mute, "🎧")
                                .accessible_name(format!("Practice mute {name}"))
                                .on_hover_text(
                                    "Practice mute: silence this part to play it yourself",
                                );
                        }
                    });
                    let fx = t.fx.len() + self.proj.missing_fx.get(&t.key).map_or(0, Vec::len);
                    let label = match (fx, frozen[i]) {
                        (0, _) => "FX".to_string(),
                        (n, false) => format!("FX ({n})"),
                        // Plays its frozen audio.
                        (n, true) => format!("FX ({n}) ❄"),
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
                    rebuild |= output_picker(ui, i, t, &buses, &allowed[i]);
                    if let Some(changed) = sends_menu(ui, i, t, &buses, &allowed[i]) {
                        if changed {
                            rebuild = true;
                        } else {
                            actions.push(Action::Sends(i));
                        }
                    }
                    rebuild |= group_picker(ui, i, t, &groups);
                    if t.kind.is_bus() {
                        ui.horizontal(|ui| {
                            if !folders.is_empty()
                                && let Some(p) = folder_picker(ui, i, t, &folders, &folders_ok[i])
                            {
                                actions.push(Action::Parent(i, p));
                            }
                            if ui
                                .button("Remove")
                                .accessible_name(format!("Remove {name}"))
                                .on_hover_text("Remove this bus; tracks on it go to the master")
                                .clicked()
                            {
                                actions.push(Action::Remove(i));
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if !folders.is_empty()
                                && let Some(p) = folder_picker(ui, i, t, &folders, &folders_ok[i])
                            {
                                actions.push(Action::Parent(i, p));
                            }
                            if let Some(k) = crate::take_picker(ui, i, t) {
                                actions.push(Action::Take(i, k));
                            }
                        });
                    }
                    ui.end_row();
                    if t.heard() != before {
                        actions.push(Action::Mix(i, t.heard()));
                    }
                }
                ui.label(RichText::new("Master").strong());
                ui.label("");
                if ui.button("Meters…").clicked() {
                    self.meters.show(None);
                }
                let rms = self.ctl.status().master_rms();
                if meters::strip(ui, self.master_meter, rms)
                    .on_hover_text("Peak and RMS. Click for the meters")
                    .clicked()
                {
                    self.meters.show(None);
                }
                ui.end_row();
            });
        ui.horizontal(|ui| {
            if ui.button("+ Add track").clicked() {
                self.tracks
                    .push(Track::new(format!("Track {}", self.tracks.len() + 1)));
                rebuild = true;
            }
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
                    self.pending_instruments.push((i.id, i.make(self.rate)));
                }
                self.tracks.push(t);
                rebuild = true;
            }
            if ui
                .button("+ Add bus")
                .on_hover_text(
                    "A bus: tracks can go to it (a group), or send a copy of their audio to \
                     it for an effect they share (a return)",
                )
                .clicked()
            {
                let n = self
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Bus)
                    .count();
                let mut t = Track::new(format!("Bus {}", n + 1));
                t.kind = TrackKind::Bus;
                self.tracks.push(t);
                rebuild = true;
            }
            if ui
                .button("+ Add folder")
                .on_hover_text(
                    "A folder: tracks put in it sit under it and go through its fader and \
                     effects",
                )
                .clicked()
            {
                let n = self
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Folder)
                    .count();
                let mut t = Track::new(format!("Folder {}", n + 1));
                t.kind = TrackKind::Folder;
                self.tracks.push(t);
                rebuild = true;
            }
            if ui
                .button("+ Add control group")
                .on_hover_text(
                    "One fader and mute over several tracks, without summing their audio",
                )
                .clicked()
            {
                let n = self.groups.len() + 1;
                self.groups.push(ControlGroup::new(
                    daw_project::new_id(),
                    format!("Group {n}"),
                ));
                rebuild = true;
            }
            rebuild |= self.input_tracks_button(ui, inputs);
        });
        if !self.groups.is_empty() {
            ui.add_space(8.0);
            ui.strong("Control groups");
            egui::Grid::new("control-groups")
                .num_columns(4)
                .spacing(vec2(10.0, 6.0))
                .show(ui, |ui| {
                    for (gi, g) in self.groups.iter_mut().enumerate() {
                        let before = (g.gain_db, g.mute);
                        ui.add(egui::TextEdit::singleline(&mut g.name).desired_width(100.0))
                            .accessible_name(format!("Name of control group {}", gi + 1));
                        ui.name_inputs(format!("{} volume", g.name), |ui| {
                            ui.add(
                                egui::Slider::new(&mut g.gain_db, -60.0..=12.0)
                                    .suffix(" dB")
                                    .step_by(0.1),
                            )
                        });
                        ui.toggle_value(&mut g.mute, "Mute")
                            .accessible_name(format!("Mute {}", g.name));
                        if ui
                            .button("Remove")
                            .accessible_name(format!("Remove {}", g.name))
                            .clicked()
                        {
                            actions.push(Action::RemoveGroup(gi));
                        }
                        ui.end_row();
                        if (g.gain_db, g.mute) != before {
                            actions.push(Action::Group(gi));
                        }
                    }
                });
        }
        for a in actions {
            match a {
                Action::Mix(track, mix) => self.send(Command::SetMix { track, mix }),
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
            }
        }
        if rebuild {
            self.rebuild();
        }
    }

    /// Whether audio from the track `from` reaches the track `to`, by
    /// its output, sends or folder, directly or through other buses.
    fn feeds(&self, from: &str, to: &str) -> bool {
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
        .width(90.0)
        .selected_text(name(&t.output))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut t.output, None, name(&None));
            for ((key, n), ok) in buses.iter().zip(allowed) {
                if *key != t.key && *ok {
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
                ui.add(
                    egui::Slider::new(&mut s.gain_db, -60.0..=12.0)
                        .suffix(" dB")
                        .step_by(0.1),
                )
                .accessible_name(format!("Send to {to} level"));
                ui.toggle_value(&mut s.pre_fader, "Pre")
                    .accessible_name(format!("Send to {to} before the fader"))
                    .on_hover_text("Pre-fader: the track's volume leaves this send alone");
                ui.toggle_value(&mut s.mute, "Mute")
                    .accessible_name(format!("Mute send to {to}"));
                if ui
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
        .width(80.0)
        .selected_text(name(&t.group))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut t.group, None, "None");
            for g in groups {
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
        .width(90.0)
        .selected_text(name(&pick))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut pick, None, "No folder");
            for ((k, n), ok) in folders.iter().zip(allowed) {
                if *k != t.key && *ok {
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
}
