// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruments on instrument tracks (roadmap 3.1): the slot that holds a
//! track's instrument, its controls, and how it is saved. For now the
//! one instrument is the simple synth that comes with the app; plugin
//! instruments and the built-in instruments follow (roadmap 3.2, 3.4).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use daw_engine::graph::InstrumentSpec;
use daw_engine::midi::Instrument;
use daw_engine::synth::{PARAMS, SimpleSynth, SynthParams, Wave};
use eframe::egui;

/// The built-in instruments, as a project names them.
pub const SYNTH: &str = "synth";

/// Instrument ids, unique for the life of the process.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A track's instrument.
pub struct Slot {
    /// Matches the engine's instrument id.
    pub id: u64,
    /// Its id in the project document.
    pub key: String,
    pub name: String,
    /// Which built-in instrument.
    pub builtin: String,
    /// The synth's settings, shared with the audio thread.
    pub params: Arc<SynthParams>,
}

impl Slot {
    /// A new simple synth with its default settings.
    pub fn synth() -> Slot {
        Slot {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            key: daw_project::new_id(),
            name: "Simple synth".into(),
            builtin: SYNTH.into(),
            params: Arc::new(SynthParams::default()),
        }
    }

    /// The slot for an instrument as the project has it. An instrument
    /// this version doesn't know (a plugin, say) is kept as the synth
    /// but keeps its name and settings for saving.
    pub fn restore(e: &daw_project::Effect) -> Slot {
        let mut s = Slot::synth();
        s.key = e.id.clone();
        if !e.name.is_empty() {
            s.name = e.name.clone();
        }
        if !e.builtin.is_empty() {
            s.builtin = e.builtin.clone();
        }
        s.params.set_all(&e.params);
        s
    }

    /// As the project stores it.
    pub fn value(&self) -> daw_project::Effect {
        daw_project::Effect {
            id: self.key.clone(),
            name: self.name.clone(),
            builtin: self.builtin.clone(),
            params: self.params.all(),
            ..daw_project::Effect::default()
        }
    }

    pub fn spec(&self) -> InstrumentSpec {
        InstrumentSpec {
            id: self.id,
            latency: 0,
        }
    }

    /// A fresh instrument for the engine, sharing this slot's settings.
    pub fn make(&self, rate: u32) -> Box<dyn Instrument> {
        Box::new(SimpleSynth::new(rate, self.params.clone()))
    }

    /// The instrument's controls. True if a setting changed.
    pub fn controls(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let p = &self.params;
        ui.horizontal(|ui| {
            ui.label("Wave");
            let mut wave = p.get("wave") as usize;
            let current = Wave::ALL.get(wave).map_or("", |w| w.label());
            egui::ComboBox::from_id_salt(("wave", self.id))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, w) in Wave::ALL.iter().enumerate() {
                        if ui.selectable_value(&mut wave, i, w.label()).changed() {
                            p.set("wave", i as f64);
                            changed = true;
                        }
                    }
                })
                .response
                .accessible_name("Wave");
        });
        egui::Grid::new(("synth", self.id))
            .num_columns(2)
            .show(ui, |ui| {
                for (name, lo, hi, _) in PARAMS {
                    if name == "wave" {
                        continue;
                    }
                    let (label, suffix) = match name {
                        "attack" => ("Attack", " s"),
                        "decay" => ("Decay", " s"),
                        "sustain" => ("Sustain", ""),
                        "release" => ("Release", " s"),
                        "cutoff" => ("Cutoff", " Hz"),
                        _ => ("Level", " dB"),
                    };
                    ui.label(label);
                    let mut v = p.get(name);
                    let slider = egui::Slider::new(&mut v, lo..=hi).suffix(suffix);
                    let slider = if name == "cutoff" || name == "attack" || name == "release" {
                        slider.logarithmic(true)
                    } else {
                        slider
                    };
                    if ui.add(slider).accessible_name(label).changed() {
                        p.set(name, v);
                        changed = true;
                    }
                    ui.end_row();
                }
            });
        changed
    }
}

/// The instrument window for a track: which instrument, its controls,
/// and the MIDI output its notes also go to. Returns whether the window
/// stays open, and whether the engine needs a new graph.
pub fn window(ctx: &egui::Context, t: &mut crate::Track, outputs: &[String]) -> (bool, bool) {
    let mut open = true;
    let mut changed = false;
    egui::Window::new(format!("Instrument: {}", t.name))
        .id(egui::Id::unique("track-instrument"))
        .open(&mut open)
        .default_pos(egui::pos2(320.0, 240.0))
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Instrument");
                let current = t.instrument.as_ref().map_or("None", |i| i.name.as_str());
                egui::ComboBox::from_id_salt("instrument-choice")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(t.instrument.is_none(), "None")
                            .on_hover_text("Notes go only to the MIDI output, if one is chosen")
                            .clicked()
                            && t.instrument.is_some()
                        {
                            t.instrument = None;
                            changed = true;
                        }
                        if ui
                            .selectable_label(t.instrument.is_some(), "Simple synth")
                            .clicked()
                            && t.instrument.is_none()
                        {
                            t.instrument = Some(Slot::synth());
                            changed = true;
                        }
                    })
                    .response
                    .accessible_name("Instrument");
            });
            if let Some(i) = &mut t.instrument {
                ui.separator();
                // Settings change on the audio thread as they move: no
                // new graph is needed.
                i.controls(ui);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("MIDI output");
                let current = t
                    .midi_out
                    .and_then(|p| outputs.get(p))
                    .map_or("None", |s| s.as_str());
                egui::ComboBox::from_id_salt("midi-out")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(t.midi_out.is_none(), "None").clicked()
                            && t.midi_out.is_some()
                        {
                            t.midi_out = None;
                            changed = true;
                        }
                        for (p, name) in outputs.iter().enumerate() {
                            if ui.selectable_label(t.midi_out == Some(p), name).clicked()
                                && t.midi_out != Some(p)
                            {
                                t.midi_out = Some(p);
                                changed = true;
                            }
                        }
                    })
                    .response
                    .accessible_name("MIDI output")
                    .on_hover_text(
                        "A MIDI device or program this track's notes also go to. Which one \
                         is this computer's choice, not the project's",
                    );
            });
            if outputs.is_empty() {
                ui.label(egui::RichText::new("No MIDI outputs were found.").weak());
            }
        });
    (open, changed)
}
