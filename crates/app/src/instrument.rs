// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruments on instrument tracks (roadmaps 3.1 and 3.4): the slot
//! that holds a track's instrument, its window, and how it is saved.
//! The simple synth from 3.1 is still there; the instruments from
//! `daw_instruments` (a synth, a sampler, a drum sampler, a drum machine
//! and a loop slicer) join it. Plugin instruments follow (roadmap 3.2).
//!
//! Moving a control is heard at once: the settings are shared with the
//! audio thread. Changing what an instrument is made of (a sound, a
//! zone, a pad's effects, the slices) makes a new one, under a new id,
//! which the next graph takes in.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use daw_engine::graph::InstrumentSpec;
use daw_engine::midi::Instrument;
use daw_engine::synth::{PARAMS, SimpleSynth, SynthParams, Wave};
use daw_instruments::{
    Builtin, Kind, Loaded, PadFx, Params, Sample, Unit, drum_machine as dm, made, sampler, slicer,
};
use eframe::egui;

use crate::commands::Cmd;

/// The simple synth, as a project names it.
pub const SYNTH: &str = "synth";

/// Instrument ids, unique for the life of the process.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Reads sounds by reference: an asset of the project, else a path.
pub type Reader<'a> = &'a dyn Fn(&str) -> Result<Sample, String>;

/// A reader for sounds in the project whose assets are in `assets`
/// (none before it is saved), or anywhere else by path.
pub fn reader(assets: Option<PathBuf>) -> impl Fn(&str) -> Result<Sample, String> {
    move |r: &str| {
        let in_project = (assets.as_ref())
            .filter(|_| daw_project::is_asset_name(r))
            .map(|a| a.join(r));
        let path = in_project.unwrap_or_else(|| PathBuf::from(r));
        daw_instruments::read_file(&path)
    }
}

enum Inner {
    Simple(Arc<SynthParams>),
    Builtin(Builtin),
}

/// A track's instrument.
pub struct Slot {
    /// Matches the engine's instrument id.
    pub id: u64,
    /// Its id in the project document.
    pub key: String,
    pub name: String,
    /// Which built-in instrument, as the project names it.
    pub builtin: String,
    inner: Inner,
    /// The drum machine's pad and the sampler's zone shown.
    pad: usize,
    zone: usize,
    /// The drum machine's part shown: sounds, effects, or outputs.
    tab: usize,
    /// The slicer's settings for cutting: how keen to find hits, and
    /// how many equal parts.
    sensitivity: f32,
    parts: usize,
}

impl Slot {
    fn with(name: &str, builtin: &str, inner: Inner) -> Slot {
        Slot {
            id: next_id(),
            key: daw_project::new_id(),
            name: name.into(),
            builtin: builtin.into(),
            inner,
            pad: 0,
            zone: 0,
            tab: 0,
            sensitivity: 0.5,
            parts: 8,
        }
    }

    /// A new simple synth with its default settings.
    pub fn synth() -> Slot {
        Slot::with(
            "Simple synth",
            SYNTH,
            Inner::Simple(Arc::new(SynthParams::default())),
        )
    }

    /// A new instrument of `kind` with its default settings and sounds.
    pub fn new(kind: Kind) -> Slot {
        Slot::with(kind.name(), kind.id(), Inner::Builtin(Builtin::new(kind)))
    }

    /// Which of the instruments from `daw_instruments` it is, if one.
    pub fn kind(&self) -> Option<Kind> {
        match &self.inner {
            Inner::Builtin(b) => Some(b.kind()),
            Inner::Simple(_) => None,
        }
    }

    pub fn builtin_ref(&self) -> Option<&Builtin> {
        match &self.inner {
            Inner::Builtin(b) => Some(b),
            Inner::Simple(_) => None,
        }
    }

    pub fn builtin_mut(&mut self) -> Option<&mut Builtin> {
        match &mut self.inner {
            Inner::Builtin(b) => Some(b),
            Inner::Simple(_) => None,
        }
    }

    /// The slot for an instrument as the project has it, reading its
    /// sounds with `read`. An instrument this version doesn't know (a
    /// plugin, say) is kept as the simple synth but keeps its name and
    /// settings for saving. Also returns the sounds that couldn't be
    /// read.
    pub fn restore(e: &daw_project::Effect, read: Reader) -> (Slot, Vec<String>) {
        let mut problems = Vec::new();
        let mut s = match Kind::from_id(&e.builtin) {
            Some(kind) => {
                let fx = (e.inner.iter())
                    .filter_map(|x| {
                        let k = daw_effects::Kind::from_id(&x.builtin)?;
                        let effect = daw_effects::Effect::new(k);
                        effect.load(&x.params);
                        Some(PadFx {
                            id: x.id.clone(),
                            pad: x.branch as usize,
                            effect: Arc::new(effect),
                            bypass: x.bypass,
                        })
                    })
                    .collect();
                let (b, p) = Builtin::restore(kind, &e.params, &e.samples, fx, read);
                problems = p;
                Slot::with(kind.name(), kind.id(), Inner::Builtin(b))
            }
            None => {
                let s = Slot::synth();
                if let Inner::Simple(p) = &s.inner {
                    p.set_all(&e.params);
                }
                s
            }
        };
        s.key = e.id.clone();
        if !e.name.is_empty() {
            s.name = e.name.clone();
        }
        if !e.builtin.is_empty() {
            s.builtin = e.builtin.clone();
        }
        (s, problems)
    }

    /// Whether `e` is this instrument with other settings only, which
    /// [`Slot::load`] can take without a new instrument.
    pub fn same_shape(&self, e: &daw_project::Effect) -> bool {
        let v = self.value();
        fn keys(p: &[(String, f64)]) -> Vec<&str> {
            let mut k: Vec<&str> = p.iter().map(|x| x.0.as_str()).collect();
            k.sort_unstable();
            k
        }
        let fx = |x: &[daw_project::Effect]| {
            x.iter()
                .map(|f| (f.id.clone(), f.builtin.clone(), f.branch, f.bypass))
                .collect::<Vec<_>>()
        };
        let mut a = v.samples.clone();
        let mut b = e.samples.clone();
        a.sort();
        b.sort();
        self.key == e.id
            && v.builtin == e.builtin
            && a == b
            && fx(&v.inner) == fx(&e.inner)
            && (self.kind().is_none() || keys(&v.params) == keys(&e.params))
    }

    /// Takes the settings in `e`, which has [`Slot::same_shape`].
    pub fn load(&mut self, e: &daw_project::Effect) {
        self.name = e.name.clone();
        match &mut self.inner {
            Inner::Simple(p) => p.set_all(&e.params),
            Inner::Builtin(b) => {
                b.params().load(&e.params);
                for x in &e.inner {
                    if let Some(f) = b.fx.iter().find(|f| f.id == x.id) {
                        f.effect.load(&x.params);
                    }
                }
            }
        }
    }

    /// As the project stores it.
    pub fn value(&self) -> daw_project::Effect {
        let (params, samples, inner) = match &self.inner {
            Inner::Simple(p) => (p.all(), Vec::new(), Vec::new()),
            Inner::Builtin(b) => (
                b.params().values(),
                b.samples(),
                (b.fx.iter())
                    .map(|f| daw_project::Effect {
                        id: f.id.clone(),
                        name: f.effect.kind().name().into(),
                        builtin: f.effect.kind().id().into(),
                        params: f.effect.values(),
                        bypass: f.bypass,
                        branch: f.pad as u32,
                        ..daw_project::Effect::default()
                    })
                    .collect(),
            ),
        };
        daw_project::Effect {
            id: self.key.clone(),
            name: self.name.clone(),
            builtin: self.builtin.clone(),
            params,
            samples,
            inner,
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
        match &self.inner {
            Inner::Simple(p) => Box::new(SimpleSynth::new(rate, p.clone())),
            Inner::Builtin(b) => b.make(rate),
        }
    }

    /// After a change to what it is made of: a new id, and a new
    /// instrument for the next graph.
    pub fn renew(&mut self, rate: u32, pending: &mut Vec<(u64, Box<dyn Instrument>)>) {
        self.id = next_id();
        pending.push((self.id, self.make(rate)));
    }

    /// The sound files it uses that aren't in the project yet (full
    /// paths), for saving.
    pub fn outside_files(&self) -> Vec<String> {
        let mut v: Vec<String> = (self.value().samples.into_iter())
            .map(|(_, r)| r)
            .filter(|r| !r.starts_with(made::PREFIX) && !daw_project::is_asset_name(r))
            .filter(|r| Path::new(r).is_file())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Gives a sound file its name in the project, once copied in.
    pub fn rename_sample(&mut self, old: &str, new: &str) {
        if let Some(b) = self.builtin_mut() {
            b.rename_sample(old, new);
        }
    }

    /// The drum machine's pads in use (with a sound on any layer).
    pub fn pads_in_use(&self) -> Vec<usize> {
        let Some(b) = self.builtin_ref().filter(|b| b.kind() == Kind::DrumMachine) else {
            return Vec::new();
        };
        (0..dm::PADS)
            .filter(|&p| {
                (0..dm::LAYERS).any(|l| b.params().get(dm::layer_at(p, l, dm::layer::SOURCE)) > 0.0)
            })
            .collect()
    }

    /// What the drum machine's pad is called: the sound on its first
    /// layer.
    pub fn pad_name(&self, p: usize) -> String {
        let Some(b) = self.builtin_ref() else {
            return format!("Pad {}", p + 1);
        };
        let pr = b.params();
        for l in 0..dm::LAYERS {
            let src = pr.get(dm::layer_at(p, l, dm::layer::SOURCE)) as usize;
            if src == dm::SAMPLE as usize {
                if let Some(s) = b.sample(&dm::sample_key(p, l)) {
                    return sound_name(&s.reference);
                }
            } else if src > 0 {
                let name = dm::SOURCES.get(src).copied().unwrap_or("Pad");
                // The kit's own names where the pad still has its drum.
                let kit = dm::kit_name(p);
                return if kit.starts_with(name) || name.contains(kit) {
                    kit.into()
                } else {
                    name.into()
                };
            }
        }
        format!("Pad {}", p + 1)
    }

    /// The drum machine's outputs its pads use, with what each is
    /// called: a pad's own output, or a send.
    pub fn outputs_in_use(&self) -> Vec<(u32, String)> {
        let Some(b) = self.builtin_ref().filter(|b| b.kind() == Kind::DrumMachine) else {
            return Vec::new();
        };
        let pr = b.params();
        let mut v = Vec::new();
        for o in 1..=dm::OUTPUTS {
            let pads: Vec<String> = (0..dm::PADS)
                .filter(|&p| pr.get(dm::pad_at(p, dm::pad::OUTPUT)) as usize == o)
                .map(|p| self.pad_name(p))
                .collect();
            if !pads.is_empty() {
                v.push((o as u32, format!("Output {o} ({})", pads.join(", "))));
            }
        }
        for (k, o, name) in [
            (dm::pad::SEND_A, dm::SEND_A, "Send A"),
            (dm::pad::SEND_B, dm::SEND_B, "Send B"),
        ] {
            if (0..dm::PADS).any(|p| pr.get(dm::pad_at(p, k)) > -60.0) {
                v.push((o as u32, name.into()));
            }
        }
        v
    }

    /// The simple synth's controls. True if a setting changed.
    fn simple_controls(&mut self, ui: &mut egui::Ui) -> bool {
        let Inner::Simple(p) = &self.inner else {
            return false;
        };
        let mut changed = false;
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

/// What a sound reference is called: a file's name without its folder
/// and ending, or the name of a sound made here.
pub fn sound_name(reference: &str) -> String {
    if let Some(n) = made::name(reference) {
        return n.into();
    }
    Path::new(reference)
        .file_stem()
        .map_or_else(|| reference.into(), |s| s.to_string_lossy().into_owned())
}

/// One setting's control, in a two-column grid row. True if it moved.
fn param(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    p: &Params,
    i: usize,
) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    if info.unit == Unit::Hidden {
        return false;
    }
    ui.label(&info.name);
    let mut v = p.get(i);
    let changed = match info.unit {
        Unit::Choice => {
            let mut c = v as usize;
            let mut moved = false;
            egui::ComboBox::from_id_salt((salt, i))
                .selected_text(info.text(v))
                .show_ui(ui, |ui| {
                    for (k, name) in info.choices.iter().enumerate() {
                        moved |= ui.selectable_value(&mut c, k, *name).changed();
                    }
                })
                .response
                .accessible_name(info.name.as_str());
            v = c as f64;
            moved
        }
        Unit::Switch => {
            let mut on = v >= 0.5;
            let r = ui.checkbox(&mut on, "").accessible_name(info.name.as_str());
            v = if on { 1.0 } else { 0.0 };
            r.changed()
        }
        Unit::Note | Unit::Count => {
            let text = info.clone();
            ui.add(
                egui::DragValue::new(&mut v)
                    .range(info.min..=info.max)
                    .speed(0.2)
                    .custom_formatter(move |x, _| text.text(x.round())),
            )
            .accessible_name(info.name.as_str())
            .changed()
        }
        _ => {
            let text = info.clone();
            ui.add(
                egui::Slider::new(&mut v, info.min..=info.max)
                    .logarithmic(info.log)
                    .custom_formatter(move |x, _| text.text(x)),
            )
            .accessible_name(info.name.as_str())
            .changed()
        }
    };
    ui.end_row();
    if changed {
        p.set(i, v);
    }
    changed
}

/// Settings `from..to` in a grid.
fn params_grid(ui: &mut egui::Ui, salt: &str, id: u64, p: &Params, from: usize, to: usize) -> bool {
    let mut changed = false;
    egui::Grid::new((salt, id, from))
        .num_columns(2)
        .show(ui, |ui| {
            for i in from..to.min(p.len()) {
                changed |= param(ui, (salt, id), p, i);
            }
        });
    changed
}

/// Sound files the samplers read.
const SOUND_FILES: &[&str] = &["wav", "flac", "mp3", "ogg", "opus", "aif", "aiff", "m4a"];

fn pick_sounds(title: &str, many: bool) -> Vec<PathBuf> {
    let d = rfd::FileDialog::new()
        .set_title(title)
        .add_filter("Sound file", SOUND_FILES);
    if many {
        d.pick_files().unwrap_or_default()
    } else {
        d.pick_file().into_iter().collect()
    }
}

/// A menu choosing a sound for a slot: a file, or one made here. The
/// reference chosen, if one was.
fn sound_menu(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    current: &str,
) -> Option<String> {
    let mut pick = None;
    let shown = if current.is_empty() {
        "None".to_string()
    } else {
        sound_name(current)
    };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .width(180.0)
        .show_ui(ui, |ui| {
            if ui.button("Choose a file…").clicked() {
                pick = pick_sounds("Choose a sound", false)
                    .first()
                    .map(|p| p.to_string_lossy().into_owned());
                ui.close();
            }
            ui.separator();
            ui.label(egui::RichText::new("Made here").weak());
            for (id, name) in made::SOUNDS {
                let r = made::reference(id);
                if ui.selectable_label(current == r, *name).clicked() {
                    pick = Some(r);
                }
            }
        })
        .response
        .accessible_name("Sound")
        .on_hover_text("A sound file of your own, or one of the sounds made by the app");
    pick
}

/// What the window asks of the app beyond its own track.
pub struct Ctx<'a> {
    /// MIDI output port names.
    pub outputs: &'a [String],
    /// Buses this track's outputs may go to: (key, name).
    pub buses: &'a [(String, String)],
    pub rate: u32,
    pub pending: &'a mut Vec<(u64, Box<dyn Instrument>)>,
    pub read: Reader<'a>,
}

/// What came of showing the window this frame.
#[derive(Default)]
pub struct Shown {
    pub open: bool,
    /// The engine needs a new graph.
    pub changed: bool,
    /// A command to run, as if chosen from a menu.
    pub cmd: Option<Cmd>,
    /// Something to tell the user.
    pub message: Option<String>,
}

/// The instruments to choose from, in the order shown.
fn choices() -> Vec<(Option<Kind>, &'static str)> {
    let mut v = vec![(None, "Simple synth")];
    v.extend(Kind::ALL.iter().map(|k| (Some(*k), k.name())));
    v
}

/// The instrument window for a track: which instrument, its controls,
/// and the MIDI output its notes also go to.
pub fn window(ctx: &egui::Context, t: &mut crate::Track, cx: Ctx) -> Shown {
    let mut open = true;
    let mut shown = Shown::default();
    egui::Window::new(format!("Instrument: {}", t.name))
        .id(egui::Id::unique("track-instrument"))
        .open(&mut open)
        .default_pos(egui::pos2(320.0, 160.0))
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Instrument");
                let current = t.instrument.as_ref().map_or("None", |i| i.name.as_str());
                let now = t.instrument.as_ref().map(|i| i.kind());
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
                            shown.changed = true;
                        }
                        for (kind, name) in choices() {
                            let hint = kind.map_or(
                                "The synth from the first version: one oscillator and a filter",
                                |k| k.about(),
                            );
                            if ui
                                .selectable_label(now == Some(kind), name)
                                .on_hover_text(hint)
                                .clicked()
                                && now != Some(kind)
                            {
                                let s = kind.map_or_else(Slot::synth, Slot::new);
                                cx.pending.push((s.id, s.make(cx.rate)));
                                t.instrument = Some(s);
                                shown.changed = true;
                            }
                        }
                    })
                    .response
                    .accessible_name("Instrument");
            });
            let Some(slot) = &mut t.instrument else {
                return;
            };
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(520.0)
                .show(ui, |ui| {
                    let renew = match slot.kind() {
                        None => {
                            slot.simple_controls(ui);
                            false
                        }
                        Some(Kind::Synth) => synth_controls(ui, slot),
                        Some(Kind::Sampler) => sampler_controls(ui, slot, &cx, &mut shown),
                        Some(Kind::DrumSampler) => drum_sampler_controls(ui, slot, &cx, &mut shown),
                        Some(Kind::DrumMachine) => drum_machine_controls(
                            ui,
                            slot,
                            &mut t.instrument_outputs,
                            &cx,
                            &mut shown,
                        ),
                        Some(Kind::Slicer) => slicer_controls(ui, slot, &cx, &mut shown),
                    };
                    if renew {
                        slot.renew(cx.rate, cx.pending);
                        shown.changed = true;
                    }
                });
            ui.separator();
            midi_out_picker(ui, t, cx.outputs, &mut shown.changed);
        });
    shown.open = open;
    shown
}

fn midi_out_picker(
    ui: &mut egui::Ui,
    t: &mut crate::Track,
    outputs: &[String],
    changed: &mut bool,
) {
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
                    *changed = true;
                }
                for (p, name) in outputs.iter().enumerate() {
                    if ui.selectable_label(t.midi_out == Some(p), name).clicked()
                        && t.midi_out != Some(p)
                    {
                        t.midi_out = Some(p);
                        *changed = true;
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
}

fn synth_controls(ui: &mut egui::Ui, slot: &mut Slot) -> bool {
    let Some(b) = slot.builtin_ref() else {
        return false;
    };
    let p = b.params().clone();
    for &(heading, from, to) in daw_instruments::synth::GROUPS {
        egui::CollapsingHeader::new(heading)
            .id_salt(("synth-group", heading))
            .default_open(true)
            .show(ui, |ui| params_grid(ui, heading, slot.id, &p, from, to + 1));
    }
    false
}

/// Reads `reference` into a sound, telling the user if it couldn't be.
fn read_sound(reference: &str, cx: &Ctx, shown: &mut Shown) -> Option<Loaded> {
    match daw_instruments::load(reference, cx.read) {
        Ok(l) => Some(l),
        Err(e) => {
            shown.message = Some(format!("Couldn't read the sound: {e}"));
            None
        }
    }
}

fn sampler_controls(ui: &mut egui::Ui, slot: &mut Slot, cx: &Ctx, shown: &mut Shown) -> bool {
    let id = slot.id;
    let mut renew = false;
    let mut chosen = slot.zone;
    let Some(b) = slot.builtin_mut() else {
        return false;
    };
    let p = b.params().clone();
    egui::CollapsingHeader::new("Sound")
        .default_open(true)
        .show(ui, |ui| {
            params_grid(ui, "sampler", id, &p, 0, sampler::at::ZONES)
        });
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong("Zones");
        if ui
            .button("Add sound files…")
            .on_hover_text(
                "Each file becomes a zone, at the note its name gives (\"Piano C4\") or on \
                 the next key, spread over the keyboard",
            )
            .clicked()
        {
            let files = pick_sounds("Add sound files", true);
            let mut add = Vec::new();
            for f in files {
                let r = f.to_string_lossy().into_owned();
                if let Some(l) = read_sound(&r, cx, shown) {
                    add.push((sound_name(&r), l));
                }
            }
            if !add.is_empty() {
                b.add_zones(add);
                renew = true;
            }
        }
        if ui
            .button("Spread over the keys")
            .on_hover_text("Each zone from halfway to the one below to halfway to the one above")
            .clicked()
        {
            b.spread_zones();
        }
    });
    let zones = b.zones();
    let mut remove = None;
    if zones > 0 {
        chosen = chosen.min(zones - 1);
        ui.horizontal_wrapped(|ui| {
            for z in 0..zones {
                let name = b
                    .sample(&format!("zone{z}"))
                    .map_or_else(|| format!("Zone {}", z + 1), |s| sound_name(&s.reference));
                let root = p.list()[daw_instruments::zone_at(z, sampler::zone::ROOT)]
                    .text(p.get(daw_instruments::zone_at(z, sampler::zone::ROOT)));
                if ui
                    .selectable_label(chosen == z, format!("{name} ({root})"))
                    .clicked()
                {
                    chosen = z;
                }
            }
        });
        ui.horizontal(|ui| {
            let current = b
                .sample(&format!("zone{chosen}"))
                .map(|s| s.reference.clone())
                .unwrap_or_default();
            ui.label("Sound");
            if let Some(r) = sound_menu(ui, ("zone-sound", id, chosen), &current)
                && let Some(l) = read_sound(&r, cx, shown)
            {
                b.set_sample(&format!("zone{chosen}"), Some(l));
                renew = true;
            }
            if ui.button("Remove zone").clicked() {
                remove = Some(chosen);
            }
        });
        let from = daw_instruments::zone_at(chosen, 0);
        params_grid(ui, "zone", id, &p, from, from + sampler::ZONE);
    } else {
        ui.label(egui::RichText::new("No zones yet: add sound files.").weak());
    }
    if let Some(z) = remove {
        b.remove_zone(z);
        renew = true;
    }
    slot.zone = chosen;
    renew
}

fn drum_sampler_controls(ui: &mut egui::Ui, slot: &mut Slot, cx: &Ctx, shown: &mut Shown) -> bool {
    let id = slot.id;
    let Some(b) = slot.builtin_mut() else {
        return false;
    };
    let mut renew = false;
    ui.horizontal(|ui| {
        ui.label("Sound");
        let current = b
            .sample("sample")
            .map(|s| s.reference.clone())
            .unwrap_or_default();
        if let Some(r) = sound_menu(ui, ("drum-sampler-sound", id), &current)
            && let Some(l) = read_sound(&r, cx, shown)
        {
            b.set_sample("sample", Some(l));
            renew = true;
        }
    });
    let p = b.params().clone();
    params_grid(ui, "drum-sampler", id, &p, 0, p.len());
    renew
}

/// The built-in effects a pad can have: not containers, and none that
/// read a file of their own.
fn pad_effects() -> impl Iterator<Item = daw_effects::Kind> {
    daw_effects::Kind::ALL
        .into_iter()
        .filter(|k| !k.is_container() && k.uses() == daw_effects::Uses::Nothing)
}

fn drum_machine_controls(
    ui: &mut egui::Ui,
    slot: &mut Slot,
    routes: &mut Vec<daw_project::InstrumentOutput>,
    cx: &Ctx,
    shown: &mut Shown,
) -> bool {
    let id = slot.id;
    let names: Vec<String> = (0..dm::PADS).map(|p| slot.pad_name(p)).collect();
    let in_use = slot.outputs_in_use();
    let mut pad = slot.pad.min(dm::PADS - 1);
    let mut tab = slot.tab;
    let mut renew = false;
    let Some(b) = slot.builtin_mut() else {
        return false;
    };
    let p = b.params().clone();
    params_grid(ui, "drum-machine", id, &p, 0, dm::at::PADS);
    ui.separator();
    // The pads, four by four, the lowest at the bottom left.
    egui::Grid::new(("pads", id))
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            for row in (0..4).rev() {
                for col in 0..4 {
                    let k = row * 4 + col;
                    let note = daw_instruments::params::note_name(dm::FIRST_NOTE as i32 + k as i32);
                    let text = format!("{}\n{note}", names[k]);
                    let r = ui
                        .add_sized([100.0, 40.0], egui::Button::selectable(pad == k, text))
                        .on_hover_text("Choose the pad to change. It plays on the key shown");
                    if r.clicked() {
                        pad = k;
                    }
                }
                ui.end_row();
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        for (k, name) in ["Sounds", "Effects", "Outputs and sends"]
            .into_iter()
            .enumerate()
        {
            if ui.selectable_label(tab == k, name).clicked() {
                tab = k;
            }
        }
    });
    ui.separator();
    if tab == 0 {
        ui.strong(format!("Pad {}: {}", pad + 1, names[pad]));
        // The pad's own settings, with its output and sends.
        egui::Grid::new(("pad", id, pad))
            .num_columns(2)
            .show(ui, |ui| {
                for k in 0..dm::pad::LAYERS {
                    let i = dm::pad_at(pad, k);
                    if k == dm::pad::OUTPUT {
                        ui.label("Output");
                        let mut o = p.get(i) as usize;
                        let text = |o: usize| {
                            if o == 0 {
                                "The track".to_string()
                            } else {
                                format!("Output {o}")
                            }
                        };
                        egui::ComboBox::from_id_salt(("pad-output", id, pad))
                            .selected_text(text(o))
                            .show_ui(ui, |ui| {
                                for n in 0..=dm::OUTPUTS {
                                    ui.selectable_value(&mut o, n, text(n));
                                }
                            })
                            .response
                            .accessible_name("Output")
                            .on_hover_text(
                                "An output of its own can go to a bus in the mixer, below. One \
                             that goes nowhere plays through the track",
                            );
                        if o != p.get(i) as usize {
                            p.set(i, o as f64);
                        }
                        ui.end_row();
                        continue;
                    }
                    param(ui, ("pad", id, pad), &p, i);
                }
            });
        // Its layers.
        for l in 0..dm::LAYERS {
            let src = dm::layer_at(pad, l, dm::layer::SOURCE);
            let on = p.get(src) > 0.0;
            let title = if on {
                format!("Layer {}: {}", l + 1, dm::SOURCES[p.get(src) as usize])
            } else {
                format!("Layer {}: off", l + 1)
            };
            egui::CollapsingHeader::new(title)
                .id_salt(("layer", id, pad, l))
                .default_open(l == 0)
                .show(ui, |ui| {
                    let sample = p.get(src) as usize == dm::SAMPLE as usize;
                    egui::Grid::new(("layer-grid", id, pad, l))
                        .num_columns(2)
                        .show(ui, |ui| {
                            param(ui, ("layer", id, pad, l), &p, src);
                            if sample {
                                ui.label("Sound");
                                let key = dm::sample_key(pad, l);
                                let current = b
                                    .sample(&key)
                                    .map(|s| s.reference.clone())
                                    .unwrap_or_default();
                                if let Some(r) =
                                    sound_menu(ui, ("layer-sound", id, pad, l), &current)
                                    && let Some(s) = read_sound(&r, cx, shown)
                                {
                                    b.set_sample(&key, Some(s));
                                    renew = true;
                                }
                                ui.end_row();
                            }
                            for k in 1..dm::LAYER {
                                // Tone means nothing to a sample, start nothing
                                // to a drum made here.
                                if (sample && k == dm::layer::TONE)
                                    || (!sample && k == dm::layer::START)
                                {
                                    continue;
                                }
                                param(ui, ("layer", id, pad, l), &p, dm::layer_at(pad, l, k));
                            }
                        });
                });
        }
    }
    if tab == 1 {
        // Its effects.
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Effects on this pad");
            egui::ComboBox::from_id_salt(("pad-fx-add", id, pad))
                .selected_text("Add…")
                .show_ui(ui, |ui| {
                    for k in pad_effects() {
                        if ui
                            .selectable_label(false, k.name())
                            .on_hover_text(k.about())
                            .clicked()
                        {
                            b.fx.push(PadFx {
                                id: daw_project::new_id(),
                                pad,
                                effect: Arc::new(daw_effects::Effect::new(k)),
                                bypass: false,
                            });
                            renew = true;
                        }
                    }
                })
                .response
                .accessible_name("Add an effect to the pad");
        });
        let mut remove = None;
        for (fi, f) in b.fx.iter_mut().enumerate().filter(|(_, f)| f.pad == pad) {
            egui::CollapsingHeader::new(f.effect.kind().name())
                .id_salt(("pad-fx", &f.id))
                .default_open(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut on = !f.bypass;
                        if ui.checkbox(&mut on, "On").changed() {
                            f.bypass = !on;
                            renew = true;
                        }
                        if ui.button("Remove").clicked() {
                            remove = Some(fi);
                        }
                    });
                    crate::fx::builtin_params(ui, id ^ (fi as u64) << 32, &f.effect);
                });
        }
        if let Some(fi) = remove {
            b.fx.remove(fi);
            renew = true;
        }
        if !b.fx.iter().any(|f| f.pad == pad) {
            ui.label(egui::RichText::new("None.").weak());
        }
    }
    if tab == 2 {
        // Where the outputs go.
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button("Put each pad on a bus")
                .on_hover_text("Gives every pad with a sound an output of its own and a bus in the mixer for it")
                .clicked()
            {
                shown.cmd = Some(Cmd::PadBuses);
            }
        });
        if in_use.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Every pad plays through the track. Give a pad an output, or turn up a send, to route it.",
                )
                .weak(),
            );
        }
        egui::Grid::new(("routes", id))
            .num_columns(2)
            .show(ui, |ui| {
                for (o, name) in &in_use {
                    ui.label(name);
                    let now = routes.iter().find(|r| r.output == *o).map(|r| r.to.clone());
                    let shown_name = now
                        .as_ref()
                        .and_then(|k| cx.buses.iter().find(|b| &b.0 == k))
                        .map_or("Nowhere: plays through the track", |b| b.1.as_str());
                    let mut pick: Option<Option<String>> = None;
                    egui::ComboBox::from_id_salt(("route", id, *o))
                        .selected_text(shown_name)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(now.is_none(), "Nowhere").clicked() {
                                pick = Some(None);
                            }
                            for (k, n) in cx.buses {
                                if ui.selectable_label(now.as_ref() == Some(k), n).clicked() {
                                    pick = Some(Some(k.clone()));
                                }
                            }
                        })
                        .response
                        .accessible_name(name.as_str());
                    if let Some(to) = pick
                        && to != now
                    {
                        routes.retain(|r| r.output != *o);
                        if let Some(to) = to {
                            routes.push(daw_project::InstrumentOutput { output: *o, to });
                            routes.sort_by_key(|r| r.output);
                        }
                        shown.changed = true;
                    }
                    ui.end_row();
                }
            });
        if cx.buses.is_empty() && !in_use.is_empty() {
            ui.label(egui::RichText::new("Add a bus in the mixer to route outputs to.").weak());
        }
    }
    slot.tab = tab;
    slot.pad = pad;
    renew
}

fn slicer_controls(ui: &mut egui::Ui, slot: &mut Slot, cx: &Ctx, shown: &mut Shown) -> bool {
    let id = slot.id;
    let (mut sensitivity, mut parts) = (slot.sensitivity, slot.parts);
    let Some(b) = slot.builtin_mut() else {
        return false;
    };
    let mut renew = false;
    ui.horizontal(|ui| {
        ui.label("Loop");
        let current = b
            .sample("loop")
            .map(|s| s.reference.clone())
            .unwrap_or_default();
        if let Some(r) = sound_menu(ui, ("slicer-loop", id), &current)
            && let Some(l) = read_sound(&r, cx, shown)
        {
            b.set_loop(Arc::new(l), sensitivity);
            renew = true;
        }
    });
    // The loop's shape, with a line where each slice starts.
    if let Some(s) = b.loop_sample() {
        let slices = b.slices();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 72.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
        let w = rect.width().max(1.0) as usize;
        let n = s.len().max(1);
        for x in 0..w {
            let a = x * n / w;
            let z = ((x + 1) * n / w).max(a + 1).min(n);
            let peak = (a..z).fold(0.0f32, |m, i| m.max(s.left[i].abs()).max(s.right[i].abs()));
            let h = peak.min(1.0) * rect.height() * 0.48;
            let cx_ = rect.left() + x as f32;
            painter.line_segment(
                [
                    egui::pos2(cx_, rect.center().y - h),
                    egui::pos2(cx_, rect.center().y + h),
                ],
                egui::Stroke::new(1.0, visuals.text_color().gamma_multiply(0.6)),
            );
        }
        for (k, &f) in slices.iter().enumerate() {
            let x = rect.left() + f as f32 / n as f32 * rect.width();
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.5, visuals.selection.bg_fill),
            );
            painter.text(
                egui::pos2(x + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                daw_instruments::params::note_name(slicer::FIRST_NOTE as i32 + k as i32),
                egui::FontId::proportional(10.0),
                visuals.text_color(),
            );
        }
        ui.label(format!(
            "{} slices, on the keys from {} up.",
            slices.len(),
            daw_instruments::params::note_name(slicer::FIRST_NOTE as i32)
        ));
    }
    ui.horizontal(|ui| {
        if ui
            .button("Cut at the hits")
            .on_hover_text("A slice where each hit starts")
            .clicked()
        {
            b.slice_at_hits(sensitivity);
            renew = true;
        }
        ui.add(egui::Slider::new(&mut sensitivity, 0.0..=1.0).text("Sensitivity"))
            .accessible_name("Sensitivity")
            .on_hover_text("Higher finds quieter hits too");
    });
    ui.horizontal(|ui| {
        if ui.button("Cut into equal parts").clicked() {
            b.slice_equally(parts);
            renew = true;
        }
        ui.add(
            egui::DragValue::new(&mut parts)
                .range(1..=slicer::MAX_SLICES)
                .suffix(" parts"),
        )
        .accessible_name("Parts");
    });
    let p = b.params().clone();
    params_grid(ui, "slicer", id, &p, 0, slicer::at::SLICES);
    if ui
        .button("Make notes that play the loop")
        .on_hover_text(
            "A note clip playing the slices in order on their beats, so the loop follows \
             the song's tempo",
        )
        .clicked()
    {
        shown.cmd = Some(Cmd::SliceNotes);
    }
    slot.sensitivity = sensitivity;
    slot.parts = parts;
    renew
}

/// The name of instrument kind `k`'s track command, as numbered in
/// [`Cmd::AddInstrument`].
pub fn kind_at(k: u8) -> Option<Kind> {
    Kind::ALL.get(k as usize).copied()
}

impl crate::App {
    /// Reads sounds for instruments: the project's assets, or files.
    pub(crate) fn sound_reader(&self) -> impl Fn(&str) -> Result<Sample, String> + use<> {
        reader((self.proj.folder.as_ref()).map(|f| f.dir.join(daw_project::ASSETS)))
    }

    /// The instrument track commands act on: the one whose instrument
    /// window is open, else the first that has an instrument of `kind`
    /// (any instrument, for `None`).
    pub(crate) fn instrument_track(&self, kind: Option<Kind>) -> Option<usize> {
        let fits = |t: &crate::Track| {
            t.kind.plays_notes()
                && t.instrument
                    .as_ref()
                    .is_some_and(|i| kind.is_none() || i.kind() == kind)
        };
        self.instrument_open
            .filter(|&i| self.tracks.get(i).is_some_and(fits))
            .or_else(|| self.tracks.iter().position(fits))
    }

    /// Adds an instrument track playing a new instrument of `kind`, and
    /// opens its window.
    pub(crate) fn add_instrument_track(&mut self, kind: Kind) {
        let n = (self.tracks.iter())
            .filter(|t| {
                t.instrument
                    .as_ref()
                    .is_some_and(|i| i.kind() == Some(kind))
            })
            .count()
            + 1;
        let mut t = crate::Track::instrument(format!("{} {n}", kind.name()));
        let slot = Slot::new(kind);
        self.pending_instruments
            .push((slot.id, slot.make(self.rate)));
        t.instrument = Some(slot);
        self.tracks.push(t);
        self.instrument_open = Some(self.tracks.len() - 1);
        self.rebuild();
    }

    /// Puts a note clip on the slicer's track that plays its slices in
    /// order, after the track's last note clip.
    pub(crate) fn slice_notes(&mut self) -> Result<(), String> {
        let i = self
            .instrument_track(Some(Kind::Slicer))
            .ok_or("Add a loop slicer track first.")?;
        let t = &mut self.tracks[i];
        let Some(b) = t.instrument.as_ref().and_then(Slot::builtin_ref) else {
            return Err("Add a loop slicer track first.".into());
        };
        let notes = b.loop_notes(daw_project::PPQ);
        if notes.is_empty() {
            return Err("The slicer has no loop to play.".into());
        }
        let beats = b.params().get(slicer::at::BEATS).max(1.0);
        let len = (beats * daw_project::PPQ as f64).round() as i64;
        let start = (t.note_clips.iter())
            .map(|c| c.start + c.len)
            .max()
            .unwrap_or(0);
        let name = b
            .sample("loop")
            .map_or_else(|| "Loop".to_string(), |s| sound_name(&s.reference));
        let mut clip = daw_project::NoteClip::new(daw_project::new_id(), name, start, len);
        clip.notes = (notes.into_iter())
            .map(|(at, l, key)| daw_project::Note::new(at, l, key, 100))
            .collect();
        t.note_clips.push(clip);
        t.note_clips.sort_by_key(|c| c.start);
        self.rebuild();
        Ok(())
    }

    /// Gives each of the drum machine's pads that has a sound an output
    /// of its own and a bus for it, after the drum track.
    pub(crate) fn pad_buses(&mut self) -> Result<(), String> {
        let i = self
            .instrument_track(Some(Kind::DrumMachine))
            .ok_or("Add a drum machine track first.")?;
        let Some(slot) = self.tracks[i].instrument.as_ref() else {
            return Err("Add a drum machine track first.".into());
        };
        let pads = slot.pads_in_use();
        let names: Vec<String> = pads.iter().map(|&p| slot.pad_name(p)).collect();
        if let Some(b) = slot.builtin_ref() {
            for &p in &pads {
                b.params()
                    .set(dm::pad_at(p, dm::pad::OUTPUT), (p + 1) as f64);
            }
        }
        let drums = self.tracks[i].name.clone();
        let mut at = i + 1;
        let mut routes = Vec::new();
        for (&p, name) in pads.iter().zip(&names) {
            let output = (p + 1) as u32;
            // A bus already there for this output is used again.
            let existing = (self.tracks[i].instrument_outputs.iter())
                .find(|o| o.output == output)
                .and_then(|o| self.bus_index(&o.to));
            let key = match existing {
                Some(b) => self.tracks[b].key.clone(),
                None => {
                    let mut bus = crate::Track::new(format!("{drums}: {name}"));
                    bus.kind = daw_project::TrackKind::Bus;
                    bus.parent = self.tracks[i].parent.clone();
                    let key = bus.key.clone();
                    self.tracks.insert(at, bus);
                    at += 1;
                    key
                }
            };
            routes.push(daw_project::InstrumentOutput { output, to: key });
        }
        let t = &mut self.tracks[i];
        t.instrument_outputs
            .retain(|o| o.output as usize > dm::OUTPUTS);
        t.instrument_outputs.extend(routes);
        t.instrument_outputs.sort_by_key(|o| o.output);
        self.rebuild();
        Ok(())
    }

    /// The instrument window for track `i`.
    pub(crate) fn instrument_window(&mut self, ctx: &egui::Context, i: usize) {
        let outputs = self.midi.ports.outputs.clone();
        let Some(key) = (self.tracks.get(i))
            .filter(|t| t.kind.plays_notes())
            .map(|t| t.key.clone())
        else {
            self.instrument_open = None;
            return;
        };
        // Buses its outputs can go to: those that don't lead back to it.
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|b| b.kind.is_bus() && b.key != key && !self.feeds(&b.key, &key))
            .map(|b| (b.key.clone(), b.name.clone()))
            .collect();
        let read = self.sound_reader();
        let rate = self.rate;
        let mut pending = std::mem::take(&mut self.pending_instruments);
        let shown = window(
            ctx,
            &mut self.tracks[i],
            Ctx {
                outputs: &outputs,
                buses: &buses,
                rate,
                pending: &mut pending,
                read: &read,
            },
        );
        self.pending_instruments.extend(pending);
        if let Some(m) = shown.message {
            self.message = Some(m);
        }
        if shown.changed {
            self.rebuild();
        }
        if let Some(cmd) = shown.cmd {
            self.run(ctx, cmd);
        }
        if !shown.open {
            self.instrument_open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use daw_engine::engine::render_with;
    use daw_engine::time::Frames;
    use daw_project::{ProjectDoc, ProjectFolder};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::App;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-instr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 1000.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
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

    /// The mix, and each track's audio after its fader, for `frames`.
    fn render(app: &App, frames: usize) -> Vec<f32> {
        let s = app.session();
        let mut inst: Vec<(u64, Box<dyn Instrument>)> = (app.tracks.iter())
            .filter_map(|t| t.instrument.as_ref())
            .map(|i| (i.id, i.make(app.rate)))
            .collect();
        let g = s
            .compile_all(daw_audio_io::MAX_BLOCK, &mut Vec::new(), &mut inst)
            .unwrap();
        let mut out = Vec::new();
        render_with(g, Frames(0), frames, 256, |l, _| {
            out.extend_from_slice(l);
            true
        });
        out
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, v| m.max(v.abs()))
    }

    /// A hit on each of the first beats, on `keys`.
    fn beat(t: &mut crate::Track, keys: &[u8]) {
        let mut c = daw_project::NoteClip::new(
            daw_project::new_id(),
            "Beat".into(),
            0,
            daw_project::PPQ * 4,
        );
        for (i, &k) in keys.iter().enumerate() {
            c.notes.push(daw_project::Note::new(
                i as i64 * daw_project::PPQ,
                daw_project::PPQ / 2,
                k,
                110,
            ));
        }
        t.note_clips.push(c);
    }

    /// A short decaying burst, as a sound file.
    fn hit_file(dir: &Path, rate: u32) -> PathBuf {
        let path = dir.join("my hit.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate / 4 {
            let x = (i as f32 * 0.3).sin() * (-(i as f32) / 2000.0).exp();
            w.write_sample((x * 20_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    fn drums(app: &App) -> &Builtin {
        app.tracks[0]
            .instrument
            .as_ref()
            .unwrap()
            .builtin_ref()
            .unwrap()
    }

    #[test]
    fn a_drum_machine_plays_a_file_on_a_pad_saves_it_and_routes_pads_to_buses() {
        let dir = scratch("drums");
        let mut h = harness();
        let ctx = h.ctx.clone();
        let rate = h.state().rate;
        h.state_mut().run(&ctx, Cmd::AddInstrument(3));
        h.run();
        assert_eq!(h.state().tracks.len(), 1);
        assert_eq!(
            h.state().tracks[0].instrument.as_ref().unwrap().kind(),
            Some(Kind::DrumMachine)
        );
        // Its window opens, showing the pads.
        assert_eq!(h.state().instrument_open, Some(0));
        assert!(h.query_all_by_label_contains("Kick").next().is_some());
        beat(&mut h.state_mut().tracks[0], &[36, 38, 42, 46]);
        h.state_mut().rebuild();
        let made_here = render(h.state(), rate as usize * 2);
        assert!(peak(&made_here) > 0.05);

        // A sound file of the user's on the kick's second layer.
        let file = hit_file(&dir, rate);
        {
            let app = h.state_mut();
            let read = app.sound_reader();
            let slot = app.tracks[0].instrument.as_mut().unwrap();
            let b = slot.builtin_mut().unwrap();
            b.params()
                .set(dm::layer_at(0, 1, dm::layer::SOURCE), dm::SAMPLE);
            let l = daw_instruments::load(&file.to_string_lossy(), &read).unwrap();
            b.set_sample(&dm::sample_key(0, 1), Some(l));
            let mut pending = Vec::new();
            slot.renew(rate, &mut pending);
            app.pending_instruments.extend(pending);
            app.rebuild();
        }
        h.run();
        let with_file = render(h.state(), rate as usize * 2);
        assert_ne!(with_file, made_here);

        // Saved: the file comes into the project, and back with it.
        let song = dir.join("Song");
        h.state_mut().save_in(&ctx, &song).unwrap();
        h.run();
        let (_, bytes) = ProjectFolder::open(&song).unwrap();
        let saved = ProjectDoc::from_bytes(&bytes, 1).unwrap().get();
        let inst = saved.tracks[0].instrument.as_ref().unwrap();
        assert_eq!(inst.builtin, "drum-machine");
        let asset = &inst
            .samples
            .iter()
            .find(|s| s.0 == "pad0.layer1")
            .unwrap()
            .1;
        assert!(daw_project::is_asset_name(asset), "{asset}");
        drop(h);
        let mut h = harness();
        h.state_mut().open_project(song.clone());
        until(&mut h, |a| !a.proj.loading());
        h.run();
        assert!(h.state().message.is_none(), "{:?}", h.state().message);
        assert_eq!(drums(h.state()).samples(), drums(h.state()).samples());
        let again = render(h.state(), rate as usize * 2);
        let diff = again
            .iter()
            .zip(&with_file)
            .fold(0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(diff < 1e-4, "{diff}");

        // Each pad on a bus: the buses come after the drums, and with
        // the drum track muted, the pads still sound through them.
        let ctx = h.ctx.clone();
        h.state_mut().instrument_open = Some(0);
        h.run();
        h.get_by_label("Outputs and sends").click();
        h.run();
        h.get_by_label("Put each pad on a bus").click();
        h.run();
        let app = h.state();
        let pads = app.tracks[0]
            .instrument
            .as_ref()
            .unwrap()
            .pads_in_use()
            .len();
        assert_eq!(pads, 16);
        assert_eq!(app.tracks.len(), 17);
        assert!(app.tracks[1..].iter().all(|t| t.kind.is_bus()));
        assert_eq!(app.tracks[1].name, "Drum machine 1: Kick");
        assert_eq!(app.tracks[0].instrument_outputs.len(), 16);
        h.state_mut().tracks[0].mix.mute = true;
        h.state_mut().rebuild();
        assert!(peak(&render(h.state(), rate as usize)) > 0.05);
        // Once more uses the same buses.
        h.state_mut().run(&ctx, Cmd::PadBuses);
        assert_eq!(h.state().tracks.len(), 17);
        // A bus taken away leaves its pad playing through the track.
        h.state_mut().tracks[0].mix.mute = false;
        let gone = h.state().tracks[1].key.clone();
        h.state_mut().tracks.retain(|t| t.key != gone);
        h.state_mut().rebuild();
        assert!(h.state().message.is_none());
    }

    #[test]
    fn a_loop_slicer_makes_notes_that_play_its_loop() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        let rate = h.state().rate;
        h.state_mut().run(&ctx, Cmd::AddInstrument(4));
        h.run();
        assert!(h.query_by_label("Make notes that play the loop").is_some());
        h.get_by_label("Make notes that play the loop").click();
        h.run();
        let t = &h.state().tracks[0];
        assert_eq!(t.note_clips.len(), 1);
        let c = &t.note_clips[0];
        assert_eq!(c.len, daw_project::PPQ * 4);
        assert_eq!(c.notes.len(), 8);
        assert!(peak(&render(h.state(), rate as usize * 2)) > 0.05);
        // From the command too: a second clip after the first.
        h.state_mut().run(&ctx, Cmd::SliceNotes);
        assert_eq!(
            h.state().tracks[0].note_clips[1].start,
            daw_project::PPQ * 4
        );
    }

    #[test]
    fn every_built_in_instrument_comes_back_from_the_project() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        for k in 0..Kind::ALL.len() as u8 {
            h.state_mut().run(&ctx, Cmd::AddInstrument(k));
        }
        h.run();
        let before: Vec<daw_project::Effect> = (h.state().tracks.iter())
            .map(|t| t.instrument.as_ref().unwrap().value())
            .collect();
        // Undo and redo go through the project document.
        h.state_mut().undo(&ctx);
        h.run();
        h.state_mut().redo(&ctx);
        h.run();
        let after: Vec<daw_project::Effect> = (h.state().tracks.iter())
            .map(|t| t.instrument.as_ref().unwrap().value())
            .collect();
        assert_eq!(after, before);
    }
}
