// SPDX-License-Identifier: GPL-3.0-or-later
//! The inspector (UI polish plan, section 1): a pane on the right that
//! shows the selected track and clip, so the everyday settings of one
//! track are in one place without opening the mixer or a window.
//!
//! Track: its name, mute, solo and arm, volume, pan, output and sends.
//! Instrument: its name, the sound with previous and next, and Open.
//! Effects: the chain with each effect's on/off, and Open.
//! Clip: its name, where it starts, its length, gain, fades and
//! transpose, and its linked copies.
//!
//! The track shown follows what was clicked last: a track header, a
//! clip, a note clip or a mixer strip. With nothing chosen it shows the
//! first track. In the simpler mode it shows only volume, pan and the
//! sound.
//!
//! Every control here runs the same thing a command or an assistant
//! tool does; `select_track` chooses the track and `get_selection`
//! reports it.

use eframe::egui::{self, RichText};

use crate::App;
use crate::timeline::{Head, HeadAsk};
use daw_engine::engine::Command;
use daw_engine::time::{Frames, Ticks};

/// The track it shows, and what was selected when it last looked, to
/// follow a new selection.
#[derive(Default)]
pub(crate) struct Inspector {
    /// Chosen by key: a header clicked, or `select_track`.
    pub(crate) track: Option<String>,
    seen_clips: std::collections::HashSet<u64>,
    seen_note: Option<String>,
    seen_desk: Vec<String>,
    /// A clip setting is being dragged: the graph is rebuilt once it
    /// stops.
    dirty: bool,
}

/// The clip the inspector shows.
pub(crate) enum Shown {
    Audio(u64),
    Notes(String),
}

impl App {
    /// Each track header's buttons and meter, for the timeline.
    pub(crate) fn heads(&self) -> Vec<Head> {
        (self.tracks.iter())
            .map(|t| Head {
                mute: t.mix.mute,
                solo: t.mix.solo,
                arm: (!t.kind.is_bus() && (t.input.is_some() || t.kind.plays_notes()))
                    .then_some(t.arm),
                meter: t.meter,
            })
            .collect()
    }

    /// Follows a new selection: a clip, a note clip or a mixer strip
    /// chosen since last time puts its track in the inspector.
    fn follow_selection(&mut self) {
        let ins = &mut self.inspector;
        if self.timeline.selected != ins.seen_clips {
            // A clip newly selected, else any still selected.
            let new = (self.timeline.selected.iter())
                .find(|id| !ins.seen_clips.contains(id))
                .or(self.timeline.selected.iter().next())
                .copied();
            ins.seen_clips = self.timeline.selected.clone();
            if let Some(id) = new
                && let Some(t) = (self.tracks.iter()).find(|t| t.clips.iter().any(|c| c.id == id))
            {
                ins.track = Some(t.key.clone());
            }
        }
        if self.timeline.note_sel != ins.seen_note {
            ins.seen_note = self.timeline.note_sel.clone();
            if let Some(key) = &ins.seen_note
                && let Some(t) =
                    (self.tracks.iter()).find(|t| t.note_clips.iter().any(|c| c.id == *key))
            {
                ins.track = Some(t.key.clone());
            }
        }
        if self.desk.selected != ins.seen_desk {
            ins.seen_desk = self.desk.selected.clone();
            if let Some(key) = ins.seen_desk.last() {
                ins.track = Some(key.clone());
            }
        }
    }

    /// The track the inspector shows, by index: the one chosen, or the
    /// first.
    pub(crate) fn inspected(&mut self) -> Option<usize> {
        self.follow_selection();
        (self.inspector.track.as_deref())
            .and_then(|k| self.track_index(k))
            .or((!self.tracks.is_empty()).then_some(0))
    }

    /// The clip the inspector shows: the first selected clip, or the
    /// selected note clip, if it is on the inspected track.
    pub(crate) fn inspected_clip(&mut self) -> Option<Shown> {
        let i = self.inspected()?;
        let t = &self.tracks[i];
        if let Some(c) = (t.clips.iter()).find(|c| self.timeline.selected.contains(&c.id)) {
            return Some(Shown::Audio(c.id));
        }
        let note = self.timeline.note_sel.as_ref()?;
        (t.note_clips.iter().any(|c| c.id == *note)).then(|| Shown::Notes(note.clone()))
    }

    /// Shows a track in the inspector, by key.
    pub(crate) fn inspect_track(&mut self, key: &str) -> bool {
        if self.track_index(key).is_none() {
            return false;
        }
        self.follow_selection();
        self.inspector.track = Some(key.to_string());
        true
    }

    /// Does what a track header asked.
    pub(crate) fn header_asked(&mut self, i: usize, ask: HeadAsk) {
        let Some(t) = self.tracks.get_mut(i) else {
            return;
        };
        match ask {
            HeadAsk::Select => {
                let key = t.key.clone();
                self.inspect_track(&key);
            }
            HeadAsk::Mute => {
                t.mix.mute = !t.mix.mute;
                let mix = t.heard();
                self.send(Command::SetMix { track: i, mix });
            }
            HeadAsk::Solo => {
                t.mix.solo = !t.mix.solo;
                let mix = t.heard();
                self.send(Command::SetMix { track: i, mix });
            }
            HeadAsk::Arm => {
                t.arm = !t.arm;
                self.rebuild();
            }
        }
    }

    /// What `get_selection` says of the inspector.
    pub(crate) fn inspector_value(&mut self) -> serde_json::Value {
        let open = self
            .workspace
            .layout
            .is_open(crate::workspace::Pane::Inspector);
        let track = self.inspected().map(|i| self.tracks[i].key.clone());
        let clip = match self.inspected_clip() {
            Some(Shown::Audio(id)) => (self.tracks.iter())
                .flat_map(|t| &t.clips)
                .find(|c| c.id == id)
                .map(|c| c.key.clone()),
            Some(Shown::Notes(key)) => Some(key),
            None => None,
        };
        serde_json::json!({"open": open, "track": track, "clip": clip})
    }

    /// The inspector pane.
    pub(crate) fn inspector_pane(&mut self, ui: &mut egui::Ui) {
        if self.inspector.dirty && !ui.input(|i| i.pointer.any_down()) {
            self.inspector.dirty = false;
            self.rebuild();
        }
        egui::ScrollArea::vertical()
            .id_salt("inspector")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let Some(i) = self.inspected() else {
                    ui.label(RichText::new("Add a track to see its settings here.").weak());
                    return;
                };
                self.inspector_track(ui, i);
                if self.tracks[i].kind.plays_notes() {
                    ui.separator();
                    self.inspector_instrument(ui, i);
                }
                if !self.simple.on {
                    ui.separator();
                    self.inspector_effects(ui, i);
                    if let Some(shown) = self.inspected_clip() {
                        ui.separator();
                        self.inspector_clip(ui, shown);
                    }
                }
            });
    }

    /// The track: name, mute, solo and arm, volume, pan, output and
    /// sends.
    fn inspector_track(&mut self, ui: &mut egui::Ui, i: usize) {
        let colour = crate::theme::look(ui.ctx()).c().track(i);
        let lit = crate::theme::look(ui.ctx()).c().clone();
        let head = self.heads()[i];
        let simple = self.simple.on;
        let t = &mut self.tracks[i];
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 18.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, colour);
            ui.label(RichText::new(&t.name).strong().size(15.0));
            ui.label(
                RichText::new(match t.kind {
                    daw_project::TrackKind::Audio => "audio",
                    daw_project::TrackKind::Instrument => "instrument",
                    daw_project::TrackKind::Bus => "bus",
                    daw_project::TrackKind::Folder => "folder",
                })
                .weak(),
            );
        });
        let mut ask = None;
        ui.horizontal(|ui| {
            // ai: set_parameter
            if crate::mixer::lit(ui, lit.warn, |ui| ui.selectable_label(head.mute, "Mute"))
                .accessible_name("Track mute")
                .clicked()
            {
                ask = Some(HeadAsk::Mute);
            }
            // ai: set_parameter
            if crate::mixer::lit(ui, lit.good, |ui| ui.selectable_label(head.solo, "Solo"))
                .accessible_name("Track solo")
                .clicked()
            {
                ask = Some(HeadAsk::Solo);
            }
            if let Some(arm) = head.arm {
                // ai: set_track_input
                if crate::mixer::lit(ui, lit.record, |ui| ui.selectable_label(arm, "Arm"))
                    .accessible_name("Track arm")
                    .on_hover_text("Arm for recording (R records)")
                    .clicked()
                {
                    ask = Some(HeadAsk::Arm);
                }
            }
        });
        let before = t.heard();
        egui::Grid::new("inspector track")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Volume");
                let first = crate::spoken::start(ui);
                ui.add(
                    // ai: set_parameter
                    egui::Slider::new(
                        &mut t.mix.gain_db,
                        crate::desk::MIN_DB as f32..=crate::desk::MAX_DB as f32,
                    )
                    .suffix(" dB")
                    .step_by(0.1),
                )
                .accessible_name("Track volume");
                crate::spoken::level(ui, first, t.mix.gain_db as f64);
                ui.end_row();
                ui.label("Pan");
                let first = crate::spoken::start(ui);
                ui.add(
                    // ai: set_parameter
                    egui::Slider::new(&mut t.mix.pan, -1.0..=1.0)
                        .custom_formatter(|v, _| crate::desk::pan_short(v as f32))
                        .step_by(0.01),
                )
                .accessible_name("Track pan");
                crate::spoken::pan(ui, first, t.mix.pan as f64);
                ui.end_row();
            });
        if t.heard() != before {
            let mix = t.heard();
            self.send(Command::SetMix { track: i, mix });
        }
        if let Some(a) = ask {
            self.header_asked(i, a);
        }
        if !simple {
            let actions = self.inspector_routing(ui, i);
            if actions {
                self.rebuild();
            }
        }
    }

    /// The instrument: its name, its sound with previous and next, and
    /// Open.
    fn inspector_instrument(&mut self, ui: &mut egui::Ui, i: usize) {
        ui.label(RichText::new("Instrument").strong());
        let t = &self.tracks[i];
        let name = match (&t.instrument, t.layers.len()) {
            (Some(s), _) => s.name.clone(),
            (None, 0) => "None".to_string(),
            (None, n) => format!("{n} layers"),
        };
        let open = self.instrument_open == Some(i);
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.label(name);
            // ai: instrument-window
            if ui
                .selectable_label(open, "Open")
                .accessible_name("Open the track's instrument")
                .on_hover_text("Its window, with all of its settings")
                .clicked()
            {
                toggle = true;
            }
        });
        if toggle {
            self.instrument_open = (!open).then_some(i);
        }
        let Some(b) = (self.tracks[i].instrument.as_ref()).and_then(|s| s.builtin_ref()) else {
            return;
        };
        let sounds = b.kind().sounds();
        if sounds.is_empty() {
            return;
        }
        let slot = self.tracks[i].instrument.as_ref().expect("has a builtin");
        // The sound whose settings it has now, if any.
        let now = sounds.iter().position(|s| {
            s.settings.iter().all(|(k, v)| {
                slot.get_setting(k)
                    .is_some_and(|x| (x - v).abs() <= 1e-4 * v.abs().max(1.0))
            })
        });
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.label("Sound");
            // ai: set_instrument
            if ui
                .add(crate::icons::IconButton::new(
                    crate::icons::Icon::MoveUp,
                    "Previous sound",
                ))
                .clicked()
            {
                pick =
                    Some(now.map_or(sounds.len() - 1, |n| (n + sounds.len() - 1) % sounds.len()));
            }
            // ai: set_instrument
            egui::ComboBox::from_id_salt(("inspector sound", slot.id))
                .selected_text(now.map_or("Your own", |n| sounds[n].name))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (n, s) in sounds.iter().enumerate() {
                        // ai: set_instrument
                        if ui.selectable_label(now == Some(n), s.name).clicked() {
                            pick = Some(n);
                        }
                    }
                })
                .response
                .accessible_name("Track sound")
                .on_hover_text("Sounds made from the settings: choosing one sets them all");
            // ai: set_instrument
            if ui
                .add(crate::icons::IconButton::new(
                    crate::icons::Icon::MoveDown,
                    "Next sound",
                ))
                .clicked()
            {
                pick = Some(now.map_or(0, |n| (n + 1) % sounds.len()));
            }
        });
        if let Some(n) = pick {
            b.set_sound(sounds[n].name);
        }
    }

    /// The track's effects: each with on and off, then Open.
    fn inspector_effects(&mut self, ui: &mut egui::Ui, i: usize) {
        let open = self.effects.open_track == Some(i);
        let mut toggle = false;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Effects").strong());
            // ai: view
            if ui
                .selectable_label(open, "Open")
                .accessible_name("Open the track's effects")
                .on_hover_text("The effects window: add, reorder and change them")
                .clicked()
            {
                toggle = true;
            }
        });
        let t = &mut self.tracks[i];
        if t.fx.is_empty() {
            ui.label(RichText::new("None yet: Open to add one.").weak());
        }
        for s in t.fx.iter_mut().filter(|s| s.place.is_none()) {
            ui.horizontal(|ui| {
                let mut on = !s.bypass;
                // ai: edit_effect
                if ui
                    .toggle_value(&mut on, "On")
                    .accessible_name(format!("{} on", s.name))
                    .on_hover_text("Switch off to bypass this effect")
                    .changed()
                {
                    s.bypass = !on;
                    changed = true;
                }
                ui.label(&s.name);
            });
        }
        if toggle {
            self.effects.open_track = (!open).then_some(i);
            self.effects.open_master = false;
        }
        if changed {
            self.rebuild();
        }
    }

    /// The selected clip on the track.
    fn inspector_clip(&mut self, ui: &mut egui::Ui, shown: Shown) {
        let map = self.tempo_map();
        let rate = self.rate as f64;
        match shown {
            Shown::Audio(id) => {
                let Some(c) = (self.tracks.iter_mut())
                    .flat_map(|t| t.clips.iter_mut())
                    .find(|c| c.id == id)
                else {
                    return;
                };
                ui.label(RichText::new("Clip").strong());
                let mut dragged = false;
                let mut changed = false;
                egui::Grid::new("inspector clip")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.label(&c.label);
                        ui.end_row();
                        ui.label("Starts");
                        ui.label(format!(
                            "bar {}",
                            map.bar_beat(map.ticks_at(Frames(c.start)))
                        ));
                        ui.end_row();
                        ui.label("Length");
                        ui.label(format!("{:.2} s", c.span as f64 / rate));
                        ui.end_row();
                        ui.label("Gain");
                        let r = ui
                            .add(
                                // ai: edit_clip
                                egui::DragValue::new(&mut c.gain_db)
                                    .range(crate::timeline::MIN_DB..=crate::timeline::MAX_DB)
                                    .speed(0.1)
                                    .max_decimals(1)
                                    .suffix(" dB"),
                            )
                            .accessible_name("Clip gain");
                        changed |= r.changed();
                        dragged |= r.dragged();
                        ui.end_row();
                        // Fades in milliseconds, together never longer
                        // than the clip.
                        let ms = |frames: usize| frames as f64 * 1000.0 / rate;
                        let frames = |ms: f64| (ms * rate / 1000.0).round().max(0.0) as usize;
                        let (mut fin, mut fout) = (ms(c.fade_in.len), ms(c.fade_out.len));
                        ui.label("Fade in");
                        let r = ui
                            .add(
                                // ai: edit_clip
                                egui::DragValue::new(&mut fin)
                                    .range(0.0..=ms(c.len))
                                    .speed(5.0)
                                    .max_decimals(0)
                                    .suffix(" ms"),
                            )
                            .accessible_name("Clip fade in");
                        changed |= r.changed();
                        dragged |= r.dragged();
                        ui.end_row();
                        ui.label("Fade out");
                        let r = ui
                            .add(
                                // ai: edit_clip
                                egui::DragValue::new(&mut fout)
                                    .range(0.0..=ms(c.len))
                                    .speed(5.0)
                                    .max_decimals(0)
                                    .suffix(" ms"),
                            )
                            .accessible_name("Clip fade out");
                        changed |= r.changed();
                        dragged |= r.dragged();
                        ui.end_row();
                        let fin = frames(fin).min(c.len);
                        let fout = frames(fout).min(c.len - fin);
                        c.fade_in.len = fin;
                        c.fade_out.len = fout;
                        ui.label("Transpose");
                        let mut semis = c.cents as f64 / 100.0;
                        let r = ui
                            .add(
                                // ai: edit_clip
                                egui::DragValue::new(&mut semis)
                                    .range(-12.0..=12.0)
                                    .speed(0.05)
                                    .max_decimals(2)
                                    .suffix(" st"),
                            )
                            .accessible_name("Clip transpose")
                            .on_hover_text("Semitones; keeps its timing");
                        if r.changed() {
                            c.cents = (semis * 100.0).round() as i32;
                        }
                        changed |= r.changed();
                        dragged |= r.dragged();
                        ui.end_row();
                    });
                let link = c.link.clone();
                if let Some(l) = link {
                    let n = (self.tracks.iter())
                        .flat_map(|t| &t.clips)
                        .filter(|c| c.link.as_deref() == Some(l.as_str()))
                        .count();
                    ui.label(
                        RichText::new(format!(
                            "One of {n} linked copies: a change to one changes them all."
                        ))
                        .weak(),
                    );
                }
                if changed {
                    if dragged {
                        self.inspector.dirty = true;
                    } else {
                        self.rebuild();
                    }
                }
            }
            Shown::Notes(key) => {
                let Some(c) = (self.tracks.iter())
                    .flat_map(|t| &t.note_clips)
                    .find(|c| c.id == key)
                else {
                    return;
                };
                ui.label(RichText::new("Note clip").strong());
                let start = map.bar_beat(Ticks(c.start));
                let beats = c.len as f64 / daw_project::PPQ as f64;
                let (name, notes, pattern, link) = (
                    c.name.clone(),
                    c.notes.len(),
                    c.pattern.is_some(),
                    c.link.clone(),
                );
                egui::Grid::new("inspector note clip")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.label(name);
                        ui.end_row();
                        ui.label("Starts");
                        ui.label(format!("bar {start}"));
                        ui.end_row();
                        ui.label("Length");
                        ui.label(format!("{beats:.2} beats"));
                        ui.end_row();
                        ui.label("Notes");
                        ui.label(if pattern {
                            "from a pattern".to_string()
                        } else {
                            notes.to_string()
                        });
                        ui.end_row();
                    });
                if let Some(l) = link {
                    let n = (self.tracks.iter())
                        .flat_map(|t| &t.note_clips)
                        .filter(|c| c.link.as_deref() == Some(l.as_str()))
                        .count();
                    ui.label(
                        RichText::new(format!(
                            "One of {n} linked copies: a change to one changes them all."
                        ))
                        .weak(),
                    );
                }
                // ai: view
                if ui
                    .button(if pattern {
                        "Edit pattern"
                    } else {
                        "Edit notes"
                    })
                    .accessible_name("Edit the clip's notes")
                    .clicked()
                {
                    if pattern {
                        self.edit_pattern_of(&key);
                    } else {
                        self.notes_editor.edit(&key);
                        self.workspace.open(crate::workspace::Pane::NotesEditor);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use crate::agent::driver::Driver;

    /// The headers' buttons, the inspector and an assistant act on the
    /// same settings, and the assistant can see what the inspector shows.
    #[test]
    fn headers_and_the_inspector_work_with_an_assistant() {
        let mut d = Driver::new();
        let id = |v: serde_json::Value| v["track"].as_str().unwrap().to_string();
        let a = id(d
            .call("add_track", json!({"kind": "audio", "name": "Guitar"}))
            .unwrap());
        let b = id(d
            .call("add_track", json!({"kind": "instrument", "name": "Keys"}))
            .unwrap());
        d.h.run();
        let inspector =
            |d: &mut Driver| d.call("get_selection", json!({})).unwrap()["inspector"].clone();
        let param = |d: &mut Driver, t: &str, p: &str| -> f64 {
            let v = d.call("list_parameters", json!({"track": t})).unwrap();
            (v[0]["params"].as_array().unwrap().iter())
                .find(|x| x["param"] == p)
                .unwrap()["value"]
                .as_f64()
                .unwrap()
        };

        // Open from the start, on the first track.
        let i = inspector(&mut d);
        assert_eq!(i["open"], true);
        assert_eq!(i["track"], json!(a));

        // Mute from the header: the assistant and the engine have it.
        d.h.get_by_label("Mute the Guitar track").click();
        d.h.run();
        assert_eq!(param(&mut d, &a, "mute"), 1.0);
        assert!(d.app().session().tracks[0].mix.mute);

        // Solo from the assistant: the header shows it lit.
        d.call(
            "set_parameter",
            json!({"track": b, "param": "solo", "value": 1}),
        )
        .unwrap();
        d.h.run();
        assert!(d.app().heads()[1].solo);

        // The assistant picks the instrument track: the inspector shows
        // it, with its instrument.
        d.call("select_track", json!({"track": b})).unwrap();
        d.h.run();
        assert_eq!(inspector(&mut d)["track"], json!(b));
        d.h.get_by_label("Open the track's instrument").click();
        d.h.run();
        assert_eq!(d.app().instrument_open, Some(1));
        d.app().instrument_open = None;

        // Its volume set by the assistant shows in the inspector, and
        // its mute there reaches the assistant.
        d.call(
            "set_parameter",
            json!({"track": b, "param": "gain", "value": -6.0}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(d.app().tracks[1].mix.gain_db, -6.0);
        d.h.get_by_label("Track mute").click();
        d.h.run();
        assert_eq!(param(&mut d, &b, "mute"), 1.0);

        // Clicking a track's header shows it.
        let header = d.h.get_by_label("Mute the Guitar track").rect();
        let at = egui::pos2(header.left() + 40.0, header.top() - 8.0);
        d.h.input_mut().events.push(egui::Event::PointerMoved(at));
        d.h.step();
        for pressed in [true, false] {
            d.h.input_mut().events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            d.h.step();
        }
        d.h.run();
        assert_eq!(inspector(&mut d)["track"], json!(a));

        // A clip selected shows its track and itself.
        let c = d
            .call("add_note_clip", json!({"track": b, "bar": 1}))
            .unwrap();
        let clip = c["clip"].as_str().unwrap().to_string();
        d.call("select_clips", json!({"clips": [clip]})).unwrap();
        d.h.run();
        let i = inspector(&mut d);
        assert_eq!(i["track"], json!(b));
        assert_eq!(i["clip"], json!(clip));
        d.h.get_by_label("Edit the clip's notes");

        // The command closes it, as from the View menu.
        d.command("inspector");
        d.h.run();
        assert_eq!(inspector(&mut d)["open"], false);
    }
}
