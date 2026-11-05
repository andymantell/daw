// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruments on instrument tracks (roadmaps 3.1 and 3.4): the slot
//! that holds a track's instrument, its window, and how it is saved.
//! The simple synth from 3.1 is still there; the instruments from
//! `daw_instruments` (a synth, a sampler, a drum sampler, a drum machine
//! and a loop slicer) join it, and so do plugin instruments in either
//! plugin format (roadmap 3.2), made and run where the user chose, as
//! plugin effects are. A plugin's extra outputs (a drum plugin with an
//! output per drum, say) go to buses as the drum machine's do; those
//! that go nowhere play through the track.
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
use daw_plugins::scan::Available;
use eframe::egui;

use crate::commands::Cmd;
use crate::fx::{Effects, FxSlot};
use crate::panel_kit::{Header, HeaderAction, Height, RAIL, Rack, RackEntry, header};
use crate::spoken;
use daw_project::RackView;

/// The simple synth, as a project names it.
pub const SYNTH: &str = "synth";

/// Instrument ids, unique for the life of the process.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_id() -> u64 {
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

/// An instrument made for rendering offline, and the plugin to keep
/// until done.
pub type Fresh = (crate::fx::Keep, Box<dyn Instrument>);

enum Inner {
    Simple(Arc<SynthParams>),
    Builtin(Builtin),
    /// A plugin, made and run as plugin effects are; its slot's id is
    /// the instrument's.
    Plugin(Box<FxSlot>),
    /// A plugin that couldn't be made here (not installed, say): kept as
    /// saved, and silent.
    Missing(Box<daw_project::Effect>),
}

/// One of an instrument's settings, as an automation lane sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    /// Its index among the instrument's settings, as the engine's
    /// lanes name it.
    pub index: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    /// Moves evenly in ratio (frequencies, times).
    pub log: bool,
    /// How many values it steps through, when it steps.
    pub steps: Option<usize>,
}

/// The simple synth's setting `key` as a person reads it.
fn simple_name(key: &str) -> String {
    match key {
        "wave" => "Wave".into(),
        "level_db" => "Level".into(),
        k => {
            let mut c = k.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
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
    /// The drum machine's pad shown, and the sampler's zone or the
    /// slicer's slice.
    pub(crate) pad: usize,
    pub(crate) zone: usize,
    /// The drum machine's part shown: layers, effects, or outputs.
    pub(crate) tab: usize,
    /// The slicer's settings for cutting: how keen to find hits, and
    /// how many equal parts.
    pub(crate) sensitivity: f32,
    pub(crate) parts: usize,
    /// The Lucky Seven's loaded sound bank, for choosing from; the
    /// settings chosen are the project's, the bank isn't.
    pub bank: Option<Arc<daw_instruments::lucky_seven::bank::Bank>>,
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
            bank: None,
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
            _ => None,
        }
    }

    /// Whether it plays the track's audio input through itself (see
    /// [`Builtin::takes_input`]).
    pub fn takes_input(&self) -> bool {
        matches!(&self.inner, Inner::Builtin(b) if b.takes_input())
    }

    /// Its settings, for assistants: (key, name, value, min, max, as
    /// text). None for a plugin, whose settings it keeps itself.
    pub fn settings(&self) -> Vec<(String, String, f64, f64, f64, String)> {
        match &self.inner {
            Inner::Simple(p) => (PARAMS.iter())
                .map(|&(k, lo, hi, _)| {
                    let v = p.get(k);
                    let (name, text) = simple_setting(k, v);
                    (k.to_string(), name.to_string(), v, lo, hi, text)
                })
                .collect(),
            Inner::Builtin(b) => {
                let p = b.params();
                (p.list().iter().enumerate())
                    .map(|(i, x)| {
                        let v = p.get(i);
                        (x.key.clone(), x.name.clone(), v, x.min, x.max, x.text(v))
                    })
                    .collect()
            }
            Inner::Plugin(_) | Inner::Missing(_) => Vec::new(),
        }
    }

    /// One of its settings an automation lane can be on, by key. None
    /// for a plugin, or a key it hasn't.
    pub fn setting(&self, key: &str) -> Option<Setting> {
        match &self.inner {
            Inner::Simple(_) => {
                let i = PARAMS.iter().position(|p| p.0 == key)?;
                let (_, min, max, _) = PARAMS[i];
                Some(Setting {
                    index: i as u32,
                    name: simple_name(key),
                    min,
                    max,
                    log: matches!(key, "attack" | "decay" | "release" | "cutoff"),
                    steps: (key == "wave").then_some(4),
                })
            }
            Inner::Builtin(b) => {
                let i = b.params().index(key)?;
                let p = &b.params().list()[i];
                (p.unit != Unit::Hidden).then(|| Setting {
                    index: i as u32,
                    name: p.name.clone(),
                    min: p.min,
                    max: p.max,
                    log: p.log,
                    steps: p
                        .stepped()
                        .then(|| (p.max - p.min).round().max(0.0) as usize + 1),
                })
            }
            Inner::Plugin(_) | Inner::Missing(_) => None,
        }
    }

    /// The keys of the settings a lane can be on, in order.
    pub fn setting_keys(&self) -> Vec<String> {
        match &self.inner {
            Inner::Simple(_) => PARAMS.iter().map(|p| p.0.to_string()).collect(),
            Inner::Builtin(b) => (b.params().list().iter())
                .filter(|p| p.unit != Unit::Hidden)
                .map(|p| p.key.clone())
                .collect(),
            Inner::Plugin(_) | Inner::Missing(_) => Vec::new(),
        }
    }

    /// A setting's value now, as its control has it.
    pub fn get_setting(&self, key: &str) -> Option<f64> {
        self.setting(key)?;
        match &self.inner {
            Inner::Simple(p) => Some(p.get(key)),
            Inner::Builtin(b) => b.params().get_key(key),
            Inner::Plugin(_) | Inner::Missing(_) => None,
        }
    }

    /// Sets a setting's control, kept in its range. Heard at once, as
    /// moving the control is; the project takes it on its next sync.
    pub fn set_setting(&self, key: &str, v: f64) {
        match &self.inner {
            Inner::Simple(p) => p.set(key, v),
            Inner::Builtin(b) => b.params().set_key(key, v),
            Inner::Plugin(_) | Inner::Missing(_) => {}
        }
    }

    /// A setting's value for showing, with its unit.
    pub fn setting_text(&self, key: &str, v: f64) -> Option<String> {
        match &self.inner {
            Inner::Simple(_) => {
                self.setting(key)?;
                Some(match key {
                    "wave" => Wave::ALL
                        .get(v.round().max(0.0) as usize)
                        .map_or("?", |w| w.label())
                        .into(),
                    "cutoff" if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
                    "cutoff" => format!("{v:.0} Hz"),
                    "level_db" => format!("{v:+.1} dB"),
                    "sustain" => format!("{:.0}%", v * 100.0),
                    _ if v < 1.0 => format!("{:.0} ms", v * 1000.0),
                    _ => format!("{v:.2} s"),
                })
            }
            Inner::Builtin(b) => {
                let i = b.params().index(key)?;
                Some(b.params().list()[i].text(v))
            }
            Inner::Plugin(_) | Inner::Missing(_) => None,
        }
    }

    /// The names of a setting's values, by value from its lowest, when
    /// it picks one of a list (a waveform, a mode); empty otherwise.
    pub fn choices(&self, key: &str) -> Vec<String> {
        if matches!(self.inner, Inner::Simple(_)) && key == "wave" {
            return Wave::ALL.iter().map(|w| w.label().to_string()).collect();
        }
        let Inner::Builtin(b) = &self.inner else {
            return Vec::new();
        };
        (b.params().list().iter())
            .find(|x| x.key == key && !x.choices.is_empty())
            .map(|x| x.choices.iter().map(|c| c.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn builtin_ref(&self) -> Option<&Builtin> {
        match &self.inner {
            Inner::Builtin(b) => Some(b),
            _ => None,
        }
    }

    pub fn builtin_mut(&mut self) -> Option<&mut Builtin> {
        match &mut self.inner {
            Inner::Builtin(b) => Some(b),
            _ => None,
        }
    }

    /// The slot for a plugin instrument just made.
    pub fn from_plugin(fx: FxSlot) -> Slot {
        let mut s = Slot::with(&fx.name, "", Inner::Simple(Arc::default()));
        s.id = fx.id;
        s.key = fx.key.clone();
        s.inner = Inner::Plugin(Box::new(fx));
        s
    }

    /// A new plugin instrument: its audio side waits in the effects'
    /// `pending_instruments`.
    pub fn new_plugin(effects: &mut Effects, a: &Available, rate: u32) -> Result<Slot, String> {
        effects.create_instrument(a, rate).map(Slot::from_plugin)
    }

    /// As [`restore`](Self::restore), for any instrument: a plugin is
    /// made with its settings, its audio side waiting in the effects'
    /// `pending_instruments`, and one that can't be made here is kept as
    /// saved, with why among the problems. The audio side of any other
    /// is made with [`make`](Self::make).
    pub fn restore_any(
        e: &daw_project::Effect,
        read: Reader,
        effects: &mut Effects,
        rate: u32,
    ) -> (Slot, Vec<String>) {
        if !e.is_plugin() {
            return Slot::restore(e, read);
        }
        match effects.restore_instrument(e, rate) {
            Ok(mut fx) => {
                fx.key = e.id.clone();
                (Slot::from_plugin(fx), Vec::new())
            }
            Err(why) => {
                let mut s = Slot::with(&e.name, "", Inner::Missing(Box::new(e.clone())));
                s.key = e.id.clone();
                (s, vec![format!("{}: {why}", e.name)])
            }
        }
    }

    /// Whether it is a plugin (made here or not).
    pub fn is_plugin(&self) -> bool {
        matches!(self.inner, Inner::Plugin(_) | Inner::Missing(_))
    }

    /// The plugin, if it is one made here.
    pub fn plugin(&self) -> Option<&FxSlot> {
        match &self.inner {
            Inner::Plugin(fx) => Some(fx),
            _ => None,
        }
    }

    pub fn plugin_mut(&mut self) -> Option<&mut FxSlot> {
        match &mut self.inner {
            Inner::Plugin(fx) => Some(fx),
            _ => None,
        }
    }

    /// The plugin's extra outputs, by bus index, with their names.
    pub fn plugin_outputs(&self) -> Vec<(u32, String)> {
        let Some(fx) = self.plugin() else {
            return Vec::new();
        };
        let Some(p) = fx.plugin() else {
            return Vec::new();
        };
        (p.buses().extra_outputs())
            .filter(|(i, _)| fx.outputs.contains(&(*i as u32)))
            .map(|(i, b)| (i as u32, b.name.clone()))
            .collect()
    }

    /// A plugin's own settings, for saving.
    pub fn state(&mut self) -> Option<daw_project::EffectState> {
        match &mut self.inner {
            Inner::Plugin(fx) => fx.state(),
            Inner::Missing(e) => e.state.clone(),
            _ => None,
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
        if self.is_plugin() || e.is_plugin() {
            // The same plugin: its own settings stay as they are.
            return self.key == e.id && v.is_plugin() && e.is_plugin() && v.class == e.class;
        }
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
            Inner::Plugin(_) | Inner::Missing(_) => {}
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

    /// As the project stores it, without a plugin's own settings (read
    /// only when saving, with [`state`](Self::state)).
    pub fn value(&self) -> daw_project::Effect {
        let (params, samples, inner) = match &self.inner {
            Inner::Missing(e) => return (**e).clone(),
            Inner::Plugin(fx) => {
                return daw_project::Effect {
                    id: self.key.clone(),
                    ..fx.value()
                };
            }
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
            latency: self.plugin().map_or(0, |p| p.latency),
        }
    }

    /// A fresh instrument for the engine, sharing this slot's settings.
    /// None for a plugin, whose audio side the effects make (see
    /// [`remake`](Self::remake)), and nothing plays a missing one.
    pub fn make(&self, rate: u32) -> Option<Box<dyn Instrument>> {
        match &self.inner {
            Inner::Simple(p) => Some(Box::new(SimpleSynth::new(rate, p.clone()))),
            Inner::Builtin(b) => Some(b.make(rate)),
            Inner::Plugin(_) | Inner::Missing(_) => None,
        }
    }

    /// Its instrument for the next graph, if [`make`](Self::make) makes
    /// one, into `pending`.
    pub fn make_into(&self, rate: u32, pending: &mut Vec<(u64, Box<dyn Instrument>)>) {
        pending.extend(self.make(rate).map(|m| (self.id, m)));
    }

    /// After a change to what it is made of: a new id, and a new
    /// instrument for the next graph.
    pub fn renew(&mut self, rate: u32, pending: &mut Vec<(u64, Box<dyn Instrument>)>) {
        self.id = next_id();
        self.make_into(rate, pending);
    }

    /// Makes its audio side again for a new engine or rate, or after a
    /// crash, keeping its settings: into `pending`, or the effects'
    /// `pending_instruments` for a plugin.
    pub fn remake(
        &mut self,
        effects: &mut Effects,
        rate: u32,
        pending: &mut Vec<(u64, Box<dyn Instrument>)>,
    ) -> Result<(), String> {
        match &mut self.inner {
            Inner::Plugin(fx) => effects.reload(fx, rate),
            _ => {
                self.make_into(rate, pending);
                Ok(())
            }
        }
    }

    /// A second instrument with the same settings, for rendering
    /// offline, with the plugin to keep until done.
    pub fn fresh(&self, effects: &mut Effects, rate: u32) -> Result<Option<Fresh>, String> {
        match &self.inner {
            Inner::Plugin(fx) => effects.fresh_instrument(fx, rate).map(Some),
            Inner::Builtin(b) => Ok(Some((None, b.make_offline(rate)))),
            _ => Ok(self.make(rate).map(|m| (None, m))),
        }
    }

    /// The sound files it uses that aren't in the project yet (full
    /// paths), for saving.
    pub fn outside_files(&self) -> Vec<String> {
        // An SFZ library stays where it is.
        let mut v: Vec<String> = (self.value().samples.into_iter())
            .filter(|(k, _)| k != daw_instruments::SFZ_SLOT)
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

    /// Shows what `old`'s panel showed: the pad, zone or slice, and the
    /// drum machine's part.
    pub(crate) fn keep_view(&mut self, old: &Slot) {
        self.pad = old.pad;
        self.zone = old.zone;
        self.tab = old.tab;
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
                // The kit's own name where the pad still has its drum.
                if l == 0 && src as f64 == dm::kit_source(p) {
                    return dm::kit_name(p).into();
                }
                return dm::SOURCES.get(src).copied().unwrap_or("Pad").into();
            }
        }
        format!("Pad {}", p + 1)
    }

    /// The drum machine's outputs its pads use, with what each is
    /// called: a pad's own output, or a send.
    pub fn outputs_in_use(&self) -> Vec<(u32, String)> {
        if self.plugin().is_some() {
            return self.plugin_outputs();
        }
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
                        // ai: set_parameter
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
                    // ai: set_parameter
                    let slider = egui::Slider::new(&mut v, lo..=hi).suffix(suffix);
                    let slider = if name == "cutoff" || name == "attack" || name == "release" {
                        slider.logarithmic(true)
                    } else {
                        slider
                    };
                    let first = spoken::start(ui);
                    if ui.add(slider).accessible_name(label).changed() {
                        p.set(name, v);
                        changed = true;
                    }
                    let text = match name {
                        "sustain" => format!("{:.0}%", v * 100.0),
                        "level_db" => spoken::db_text(v),
                        _ => format!("{v:.3}{suffix}"),
                    };
                    spoken::hearable(ui, first, text, spoken::steady(v, lo, hi));
                    ui.end_row();
                }
            });
        changed
    }
}

/// A simple synth setting's name and its value as text, with its unit:
/// the wave by name ("Saw"), times in ms or s, the level in dB.
fn simple_setting(key: &str, v: f64) -> (&'static str, String) {
    match key {
        "wave" => (
            "Wave",
            Wave::ALL
                .get(v.max(0.0) as usize)
                .map_or("?", |w| w.label())
                .to_string(),
        ),
        "attack" | "decay" | "release" => {
            let name = match key {
                "attack" => "Attack",
                "decay" => "Decay",
                _ => "Release",
            };
            let text = if v < 1.0 {
                format!("{:.0} ms", v * 1000.0)
            } else {
                format!("{v:.2} s")
            };
            (name, text)
        }
        "sustain" => ("Sustain", format!("{:.0}%", v * 100.0)),
        "cutoff" if v >= 1000.0 => ("Cutoff", format!("{:.2} kHz", v / 1000.0)),
        "cutoff" => ("Cutoff", format!("{v:.0} Hz")),
        "level_db" => ("Level", format!("{v:+.1} dB")),
        _ => ("?", format!("{v:.3}")),
    }
}

/// What a sound reference is called: a file's name without its folder
/// and ending, or the name of a sound made here.
/// What a library is, in a line.
pub fn library_summary(l: &daw_instruments::Library) -> String {
    let mut m = format!(
        "Loaded {}: {} regions, {} sounds, {} MB in memory{}.",
        l.name(),
        l.sfz.regions.len(),
        l.sources.len(),
        l.bytes >> 20,
        if l.streamed {
            ", the rest played from disk"
        } else {
            ""
        }
    );
    if !l.problems.is_empty() {
        m.push_str(&format!(
            " {} couldn't be read, such as {}.",
            l.problems.len(),
            l.problems[0]
        ));
    }
    m
}

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
    let first = spoken::start(ui);
    let changed = match info.unit {
        Unit::Choice => {
            let mut c = v as usize;
            let mut moved = false;
            egui::ComboBox::from_id_salt((salt, i))
                .selected_text(info.text(v))
                .show_ui(ui, |ui| {
                    for (k, name) in info.choices.iter().enumerate() {
                        // ai: set_parameter
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
            // ai: set_parameter
            let r = ui.checkbox(&mut on, "").accessible_name(info.name.as_str());
            v = if on { 1.0 } else { 0.0 };
            r.changed()
        }
        Unit::Note | Unit::Count => {
            let text = info.clone();
            ui.add(
                // ai: set_parameter
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
                // ai: set_parameter
                egui::Slider::new(&mut v, info.min..=info.max)
                    .logarithmic(info.log)
                    .custom_formatter(move |x, _| text.text(x)),
            )
            .accessible_name(info.name.as_str())
            .changed()
        }
    };
    // A screen reader hears the setting with its unit, and Hear the
    // value plays it.
    let hear = match info.unit {
        Unit::Choice | Unit::Switch => None,
        Unit::Db => Some(spoken::Hear::Level(v)),
        Unit::Pan => Some(spoken::Hear::Pan(v)),
        _ => Some(spoken::steady(v, info.min, info.max)),
    };
    if let Some(hear) = hear {
        spoken::hearable(ui, first, info.text(v), hear);
    }
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

pub(crate) fn pick_sounds(title: &str, many: bool) -> Vec<PathBuf> {
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
pub(crate) fn sound_menu(
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
            // ai: set_sound.sound
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
                // ai: set_sound.sound
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
    /// Makes and runs plugin instruments.
    pub effects: &'a mut Effects,
    /// The song's key, which some note effects follow.
    pub scale: Option<daw_project::Scale>,
}

/// What came of showing the window this frame.
#[derive(Default)]
pub struct Shown {
    /// The engine needs a new graph.
    pub changed: bool,
    /// A command to run, as if chosen from a menu.
    pub cmd: Option<Cmd>,
    /// Something to tell the user.
    pub message: Option<String>,
    /// A change to the track's layered instruments to make.
    pub layer: Option<crate::layers::Edit>,
}

/// The instruments to choose from, in the order shown.
pub(crate) fn choices() -> Vec<(Option<Kind>, &'static str)> {
    let mut v = vec![(None, "Simple synth")];
    v.extend(Kind::ALL.iter().map(|k| (Some(*k), k.name())));
    v
}

/// The plugin instruments to choose from, in an instrument menu, after
/// the built-in ones; `slot` is the one chosen now. A new slot for the
/// plugin picked, if one was, its audio side waiting in the effects'
/// `pending_instruments`.
pub(crate) fn plugin_choices(
    ui: &mut egui::Ui,
    slot: Option<&Slot>,
    cx: &mut Ctx,
    shown: &mut Shown,
) -> Option<Slot> {
    let plugins: Vec<Available> = (cx.effects.list.plugins().into_iter())
        .filter(|p| p.enabled && p.class.is_instrument())
        .collect();
    ui.separator();
    if plugins.is_empty() {
        ui.label(egui::RichText::new("No plugin instruments found").weak())
            .on_hover_text("Plugins are found in Settings → Plugins");
    }
    let now = slot.map(Slot::value).filter(|v| v.is_plugin());
    let mut chosen = None;
    for p in &plugins {
        let cid: Vec<u8> = p.class.cid.iter().map(|&b| b as u8).collect();
        let on = now.as_ref().is_some_and(|v| v.class == cid);
        if ui
            // ai: set_instrument.instrument
            .selectable_label(on, &p.class.name)
            .on_hover_text(format!("{} plugin", p.format()))
            .clicked()
            && !on
        {
            chosen = Some(p.clone());
        }
    }
    let a = chosen?;
    match Slot::new_plugin(cx.effects, &a, cx.rate) {
        Ok(s) => Some(s),
        Err(e) => {
            shown.message = Some(format!("{}: {e}", a.class.name));
            None
        }
    }
}

/// The instrument pane's insides for a track: which instrument (or
/// layered instruments, roadmap 3.5), its controls, and the MIDI output
/// its notes also go to.
pub fn panel(ui: &mut egui::Ui, t: &mut crate::Track, mut cx: Ctx) -> Shown {
    use crate::layers::Edit;
    let mut shown = Shown::default();
    let layered = !t.layers.is_empty();
    egui::ScrollArea::vertical()
        .id_salt("instrument-pane")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&t.name).strong());
                ui.label("Instrument");
                instrument_chooser(ui, t, &mut cx, &mut shown, layered);
            });
            if layered {
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(560.0)
                    .show(ui, |ui| {
                        let edit = crate::layers::controls(ui, &mut t.layers, &mut cx, &mut shown);
                        shown.layer = edit.or(shown.layer);
                    });
                ui.separator();
                let edited = crate::notefx::controls(ui, &t.key, &mut t.note_fx, cx.scale);
                shown.changed |= edited.changed;
                shown.cmd = edited.cmd.or(shown.cmd);
                ui.separator();
                note_repeat_picker(ui, &t.key, &mut t.note_repeat, &mut shown.changed);
                midi_out_picker(ui, &mut t.midi_out, cx.outputs, &mut shown.changed);
                return;
            }
            if t.instrument.is_none() {
                return;
            }
            ui.add_space(2.0);
            rack_panel(ui, t, &mut cx, &mut shown);
        });
    let _ = Edit::Add;
    shown
}

/// The drop-down choosing the track's instrument.
fn instrument_chooser(
    ui: &mut egui::Ui,
    t: &mut crate::Track,
    cx: &mut Ctx,
    shown: &mut Shown,
    layered: bool,
) {
    use crate::layers::Edit;
    let current = match &t.instrument {
        _ if layered => "Layered instruments",
        Some(i) => i.name.as_str(),
        None => "None",
    };
    let now = (t.instrument.as_ref())
        .filter(|i| !layered && !i.is_plugin())
        .map(|i| i.kind());
    egui::ComboBox::from_id_salt("instrument-choice")
        .selected_text(current)
        .show_ui(ui, |ui| {
            let none = t.instrument.is_none() && !layered;
            if ui
                // ai: set_instrument.instrument
                .selectable_label(none, "None")
                .on_hover_text("Notes go only to the MIDI output, if one is chosen")
                .clicked()
                && !none
            {
                crate::layers::drop_layers(t);
                t.instrument = None;
                shown.changed = true;
            }
            for (kind, name) in choices() {
                let hint = kind.map_or(
                    "The synth from the first version: one oscillator and a filter",
                    |k| k.about(),
                );
                if ui
                    // ai: set_instrument.instrument
                    .selectable_label(now == Some(kind), name)
                    .on_hover_text(hint)
                    .clicked()
                    && now != Some(kind)
                {
                    let s = kind.map_or_else(Slot::synth, Slot::new);
                    s.make_into(cx.rate, cx.pending);
                    crate::layers::drop_layers(t);
                    t.instrument = Some(s);
                    shown.changed = true;
                }
            }
            let slot = t.instrument.as_ref().filter(|_| !layered);
            if let Some(s) = plugin_choices(ui, slot, cx, shown) {
                crate::layers::drop_layers(t);
                // Where another instrument's outputs went
                // means nothing to this one.
                t.instrument_outputs.clear();
                t.instrument = Some(s);
                shown.changed = true;
            }
            ui.separator();
            if ui
                // ai: add-layer
                .selectable_label(layered, "Layered instruments")
                .on_hover_text(
                    "Several instruments playing the same notes, each with its own \
                     range of notes, level and effects: for a keyboard split or a \
                     doubled sound. The instrument now becomes the first layer",
                )
                .clicked()
                && !layered
            {
                shown.layer = Some(Edit::Add);
            }
        })
        .response
        .accessible_name("Instrument");
}

/// The unit id of a track's instrument in its rack.
pub(crate) const INSTRUMENT_UNIT: &str = "instrument";

/// Shows `unit` of `rack` as `view`; `all` shows every unit.
pub(crate) fn set_rack_view(rack: &mut Vec<daw_project::RackUnit>, unit: &str, view: RackView) {
    if unit == "all" {
        rack.clear();
        return;
    }
    rack.retain(|u| u.unit != unit);
    if view != RackView::Shown {
        rack.push(daw_project::RackUnit {
            unit: unit.to_string(),
            view,
        });
        rack.sort_by(|a, b| a.unit.cmp(&b.unit));
    }
}

/// How `unit` shows in `rack`.
pub(crate) fn rack_view(rack: &[daw_project::RackUnit], unit: &str) -> RackView {
    (rack.iter())
        .find(|u| u.unit == unit)
        .map_or(RackView::Shown, |u| u.view)
}

/// The units a track's rack lists: its instrument, then its effects.
pub(crate) fn rack_entries(t: &crate::Track) -> Vec<RackEntry> {
    let mut v = Vec::new();
    if let Some(i) = &t.instrument {
        v.push(RackEntry {
            id: INSTRUMENT_UNIT.into(),
            name: i.name.clone(),
            view: rack_view(&t.rack, INSTRUMENT_UNIT),
        });
    }
    for f in &t.fx {
        v.push(RackEntry {
            id: f.key.clone(),
            name: f.name.clone(),
            view: rack_view(&t.rack, &f.key),
        });
    }
    v
}

/// The settings kept for Compare: the sound as it was when the panel
/// first showed it or a sound was chosen, and, while comparing, the
/// edits put aside.
#[derive(Clone, Default)]
struct Compare {
    kept: Vec<(String, f64)>,
    edited: Option<Vec<(String, f64)>>,
}

fn compare_id(slot: u64) -> egui::Id {
    egui::Id::unique(("compare-sound", slot))
}

/// What Compare holds for `slot`, started from its settings now.
fn compare_state(ctx: &egui::Context, slot: &Slot) -> Compare {
    let id = compare_id(slot.id);
    ctx.data_mut(|d| {
        d.get_temp::<Compare>(id).unwrap_or_else(|| {
            let kept = slot
                .builtin_ref()
                .map(|b| b.params().values())
                .unwrap_or_default();
            let c = Compare { kept, edited: None };
            d.insert_temp(id, c.clone());
            c
        })
    })
}

/// Keeps the settings now as the sound to compare against.
fn keep_for_compare(ctx: &egui::Context, slot: &Slot) {
    let kept = slot
        .builtin_ref()
        .map(|b| b.params().values())
        .unwrap_or_default();
    ctx.data_mut(|d| d.insert_temp(compare_id(slot.id), Compare { kept, edited: None }));
}

/// Compare: swaps the settings with those kept, and back.
pub(crate) fn compare_sound(ctx: &egui::Context, slot: &Slot) {
    let Some(b) = slot.builtin_ref() else {
        return;
    };
    let mut c = compare_state(ctx, slot);
    match c.edited.take() {
        Some(edited) => b.params().load(&edited),
        None => {
            c.edited = Some(b.params().values());
            b.params().load(&c.kept);
        }
    }
    ctx.data_mut(|d| d.insert_temp(compare_id(slot.id), c));
}

/// Whether Compare is showing the kept sound.
pub(crate) fn comparing(ctx: &egui::Context, slot: &Slot) -> bool {
    ctx.data(|d| d.get_temp::<Compare>(compare_id(slot.id)))
        .is_some_and(|c| c.edited.is_some())
}

/// Steps to the next (`by` 1) or previous (-1) of the instrument's
/// sounds, from the one playing, or the first. The sound chosen.
pub(crate) fn step_sound(ctx: &egui::Context, slot: &Slot, by: i32) -> Option<&'static str> {
    let b = slot.builtin_ref()?;
    let sounds = b.kind().sounds();
    if sounds.is_empty() {
        return None;
    }
    let n = sounds.len() as i32;
    let now = b
        .current_sound()
        .and_then(|s| sounds.iter().position(|x| x.name == s));
    let next = match now {
        Some(i) => (i as i32 + by).rem_euclid(n),
        None if by < 0 => n - 1,
        None => 0,
    };
    let name = sounds[next as usize].name;
    b.set_sound(name);
    keep_for_compare(ctx, slot);
    Some(name)
}

/// The height, in rack units, of a built-in instrument's panel on the
/// kit; `Fit` for one still on its settings grid.
fn unit_height(kind: Option<Kind>) -> Height {
    match kind {
        Some(
            Kind::Synth
            | Kind::Sampler
            | Kind::Slicer
            | Kind::PatchTwenty
            | Kind::BlueWave
            | Kind::LuckySeven
            | Kind::June
            | Kind::Oracle
            | Kind::Eighty,
        ) => Height::Units(3),
        Some(Kind::DrumMachine) => Height::Units(crate::drum_panel::UNITS),
        Some(Kind::ModelMono) => Height::Units(crate::mono_panels::MODEL_MONO_UNITS),
        Some(Kind::AcidThree) => Height::Units(crate::mono_panels::ACID_THREE_UNITS),
        Some(Kind::OhOne) => Height::Units(crate::mono_panels::OH_ONE_UNITS),
        _ => Height::Fit,
    }
}

/// The track's instrument as a unit in the rack: the rails, the "Show in
/// rack" menu, and the unit in full, as a strip, or hidden.
fn rack_panel(ui: &mut egui::Ui, t: &mut crate::Track, cx: &mut Ctx, shown: &mut Shown) {
    let entries = rack_entries(t);
    let (mut rack, change) = Rack::new(ui, &entries);
    if let Some((unit, view)) = change {
        set_rack_view(&mut t.rack, &unit, view);
    }
    let view = rack_view(&t.rack, INSTRUMENT_UNIT);
    let crate::Track {
        key,
        instrument,
        instrument_outputs,
        note_fx,
        note_repeat,
        midi_out,
        meter,
        ..
    } = t;
    let Some(slot) = instrument else {
        return;
    };
    let id = egui::Id::unique(("instrument-unit", slot.id));
    let name = slot.name.clone();
    let kind = slot.kind();
    let about = match (slot.is_plugin(), kind) {
        (true, _) => "Plugin instrument".to_string(),
        (false, None) => "The synth from the first version: one oscillator and a filter".into(),
        (false, Some(k)) => k.about().to_string(),
    };
    let ctx = ui.ctx().clone();
    // The unit's own character, for the kit to draw with.
    crate::character::set(
        &ctx,
        crate::character::Character::of(if slot.is_plugin() { None } else { kind }),
    );
    match view {
        RackView::Hidden => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(RAIL + 8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{name} is hidden from the rack; it still plays. Show in rack is the menu at the top of the rail."
                    ))
                    .weak(),
                );
            });
        }
        RackView::Collapsed => {
            let sound = slot.builtin_ref().and_then(|b| b.current_sound());
            let level = meter.0.max(meter.1);
            rack.strip(ui, id, &name, |ui| {
                if let Some(s) = sound {
                    ui.label(egui::RichText::new(s).weak());
                }
                strip_meter(ui, level);
            });
        }
        RackView::Shown => {
            let height = unit_height(if slot.is_plugin() { None } else { kind }).max_fit(kind);
            rack.unit(ui, id, height, |ui| {
                let sound = slot
                    .builtin_ref()
                    .and_then(|b| b.current_sound())
                    .map(String::from);
                let has_sounds = kind.is_some_and(|k| !k.sounds().is_empty()) && !slot.is_plugin();
                let comparing = comparing(&ctx, slot);
                let mut cmd = None;
                let action = header(
                    ui,
                    Header {
                        name: &name,
                        about: &about,
                        sound,
                        has_sounds,
                        comparing,
                    },
                    |ui| {
                        notes_in(ui, key, note_fx, note_repeat, midi_out, cx, shown, &mut cmd);
                    },
                );
                shown.cmd = cmd.or(shown.cmd);
                match action {
                    Some(HeaderAction::Previous) => {
                        step_sound(&ctx, slot, -1);
                    }
                    Some(HeaderAction::Next) => {
                        step_sound(&ctx, slot, 1);
                    }
                    Some(HeaderAction::Save) => shown.cmd = Some(Cmd::SaveTrackPreset),
                    Some(HeaderAction::Compare) => compare_sound(&ctx, slot),
                    None => {}
                }
                ui.add_space(2.0);
                let body = ui.available_height();
                match kind {
                    Some(Kind::Synth) if !slot.is_plugin() => {
                        if let Some(b) = slot.builtin_ref() {
                            let p = b.params().clone();
                            crate::synth_panel::poly_synth(ui, &p, slot.id, body);
                        }
                    }
                    Some(Kind::Sampler) if !slot.is_plugin() => {
                        if crate::sampler_panel::sampler(ui, slot, cx, shown, body) {
                            slot.renew(cx.rate, cx.pending);
                            shown.changed = true;
                        }
                    }
                    Some(Kind::PatchTwenty) if !slot.is_plugin() => {
                        if let Some(b) = slot.builtin_ref() {
                            let p = b.params().clone();
                            crate::patch_twenty_panel::patch_twenty(ui, &p, slot.id, body);
                        }
                    }
                    Some(Kind::BlueWave) if !slot.is_plugin() => {
                        if let Some(b) = slot.builtin_ref() {
                            let p = b.params().clone();
                            crate::blue_wave_panel::blue_wave(ui, &p, slot.id, body);
                        }
                    }
                    Some(Kind::LuckySeven) if !slot.is_plugin() => {
                        lucky_seven_panel(ui, slot, body, shown);
                    }
                    Some(Kind::Slicer) if !slot.is_plugin() => {
                        if crate::slicer_panel::panel(ui, slot, cx, shown, body) {
                            slot.renew(cx.rate, cx.pending);
                            shown.changed = true;
                        }
                    }
                    Some(k @ (Kind::June | Kind::Oracle | Kind::Eighty)) if !slot.is_plugin() => {
                        if let Some(b) = slot.builtin_ref() {
                            let p = b.params().clone();
                            let panel = match k {
                                Kind::June => crate::poly_panels::june,
                                Kind::Oracle => crate::poly_panels::oracle,
                                _ => crate::poly_panels::eighty,
                            };
                            panel(ui, &p, slot.id, body);
                        }
                    }
                    Some(k @ (Kind::ModelMono | Kind::AcidThree | Kind::OhOne))
                        if !slot.is_plugin() =>
                    {
                        if let Some(b) = slot.builtin_ref() {
                            let p = b.params().clone();
                            let draw = match k {
                                Kind::ModelMono => crate::mono_panels::model_mono,
                                Kind::AcidThree => crate::mono_panels::acid_three,
                                _ => crate::mono_panels::oh_one,
                            };
                            draw(ui, &p, slot.id, body);
                        }
                    }
                    _ => {
                        slot_controls(ui, slot, instrument_outputs, cx, shown);
                    }
                }
            });
        }
    }
    crate::character::clear(&ctx);
}

/// Lucky Seven on its panel, with the bank it has loaded.
fn lucky_seven_panel(ui: &mut egui::Ui, slot: &mut Slot, body: f32, shown: &mut Shown) {
    let bank = slot.bank.clone();
    let Some(b) = slot.builtin_ref() else {
        return;
    };
    let p = b.params().clone();
    let mut choose = None;
    let asked =
        crate::lucky_seven_panel::lucky_seven(ui, &p, slot.id, body, bank.as_deref(), &mut choose);
    if let (Some(k), Some(bank)) = (choose, &bank)
        && let Some(v) = bank.voices.get(k)
    {
        b.set_bank_voice(v);
    }
    if asked.load_bank {
        shown.cmd = Some(Cmd::LoadBank);
    }
}

impl Height {
    /// A unit on the kit keeps its height; one on its settings grid fits
    /// its contents, whatever its kind.
    fn max_fit(self, _kind: Option<Kind>) -> Height {
        self
    }
}

/// A small level meter on a collapsed unit's strip.
fn strip_meter(ui: &mut egui::Ui, level: f32) {
    let look = crate::theme::look(ui.ctx());
    let c = look.c();
    let (_, rect) = ui.allocate_space(egui::vec2(60.0, 8.0));
    let p = ui.painter();
    p.rect_filled(rect, 2.0, c.meter_bg);
    let db = 20.0 * level.max(1e-5).log10();
    let k = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
    if k > 0.0 {
        let lit = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * k, rect.height()));
        let colour = if db > 0.0 {
            c.meter_high
        } else if db > -12.0 {
            c.meter_mid
        } else {
            c.meter_low
        };
        p.rect_filled(lit, 2.0, colour);
    }
}

/// The "Notes in" buttons at the right of the header: the note effects,
/// note repeat and MIDI output, each opening its controls.
#[allow(clippy::too_many_arguments)]
fn notes_in(
    ui: &mut egui::Ui,
    key: &str,
    note_fx: &mut Vec<daw_project::NoteFx>,
    note_repeat: &mut Option<daw_project::StepRate>,
    midi_out: &mut Option<usize>,
    cx: &Ctx,
    shown: &mut Shown,
    cmd: &mut Option<Cmd>,
) {
    use egui::{Popup, PopupCloseBehavior};
    let look = crate::theme::look(ui.ctx());
    // Right to left: the last first.
    let out = (midi_out.and_then(|p| cx.outputs.get(p))).map_or("none", |s| s.as_str());
    // ai: set_track_input.midi_out
    let r = ui
        .small_button(format!("MIDI out: {out}"))
        .on_hover_text("A MIDI device or program this track's notes also go to");
    Popup::menu(&r)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(220.0);
            midi_out_picker(ui, midi_out, cx.outputs, &mut shown.changed);
        });
    let rep = note_repeat.map_or("off", |r| r.label());
    // ai: edit_instrument.note_repeat
    let r = ui
        .small_button(format!("Repeat: {rep}"))
        .on_hover_text("Keys and pads held down play again at this rate");
    Popup::menu(&r)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(200.0);
            note_repeat_picker(ui, key, note_repeat, &mut shown.changed);
        });
    // ai: run_command.id
    let r = ui
        .small_button(format!("Note effects ({})", note_fx.len()))
        .on_hover_text("Effects on the notes before the instrument: an arpeggiator, chords…");
    Popup::menu(&r)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(360.0);
            let edited = crate::notefx::controls(ui, key, note_fx, cx.scale);
            shown.changed |= edited.changed;
            *cmd = edited.cmd.or(cmd.take());
        });
    ui.label(
        egui::RichText::new("Notes in")
            .size(look.small)
            .color(look.c().text_dim),
    );
}

/// The controls of the instrument in `slot`, with where its extra
/// outputs go. Changing what it is made of gives it a new instrument.
pub(crate) fn slot_controls(
    ui: &mut egui::Ui,
    slot: &mut Slot,
    outputs: &mut Vec<daw_project::InstrumentOutput>,
    cx: &mut Ctx,
    shown: &mut Shown,
) {
    if slot.is_plugin() {
        plugin_controls(ui, slot, outputs, cx, shown);
        return;
    }
    let renew = match slot.kind() {
        None => {
            slot.simple_controls(ui);
            false
        }
        Some(Kind::Synth) => synth_controls(ui, slot),
        Some(Kind::OhOne) => grouped_controls(ui, slot, "oh-one", daw_instruments::oh_one::GROUPS),
        Some(Kind::Sampler) => sampler_controls(ui, slot, cx, shown),
        Some(Kind::DrumSampler) => drum_sampler_controls(ui, slot, cx, shown),
        Some(Kind::DrumMachine) => drum_machine_controls(ui, slot, outputs, cx, shown),
        Some(Kind::Slicer) => {
            crate::slicer_panel::panel(ui, slot, cx, shown, crate::slicer_panel::BODY)
        }
        Some(Kind::PatchTwenty) => patch_twenty_controls(ui, slot),
        Some(Kind::AcidThree) => acid_controls(ui, slot),
        Some(Kind::June) => june_controls(ui, slot),
        Some(Kind::ModelMono) => {
            sound_controls(ui, slot, "model-mono", daw_instruments::model_mono::GROUPS)
        }
        Some(Kind::Eighty) => {
            ui.label(
                egui::RichText::new(
                    "How hard a key is struck and then pressed sets the level and brightness: draw a note's pressure line in the editor, or play a keyboard with aftertouch.",
                )
                .weak(),
            );
            sound_controls(ui, slot, "eighty", daw_instruments::eighty::GROUPS)
        }
        Some(Kind::Oracle) => sound_controls(ui, slot, "oracle", daw_instruments::oracle::GROUPS),
        Some(Kind::BlueWave) => {
            ui.label(
                egui::RichText::new(
                    "Wave is the place in the table the oscillators read; the wave envelope, LFO, keyboard and mod wheel move it. Eight-bit keeps the rounding, the steps and the aliasing.",
                )
                .weak(),
            );
            sound_controls(ui, slot, "blue-wave", daw_instruments::blue_wave::GROUPS)
        }
        Some(Kind::LuckySeven) => lucky_seven_controls(ui, slot, shown),
    };
    if renew {
        slot.renew(cx.rate, cx.pending);
        shown.changed = true;
    }
}

/// Note-repeat: off, or how often held keys and pads play again.
fn note_repeat_picker(
    ui: &mut egui::Ui,
    key: &str,
    note_repeat: &mut Option<daw_project::StepRate>,
    changed: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label("Note repeat");
        let text = |r: Option<daw_project::StepRate>| r.map_or("Off", |r| r.label());
        let mut now = *note_repeat;
        egui::ComboBox::from_id_salt(("note-repeat", key))
            .selected_text(text(now))
            .show_ui(ui, |ui| {
                // ai: edit_instrument.note_repeat
                ui.selectable_value(&mut now, None, "Off");
                for r in daw_project::StepRate::ALL {
                    // ai: edit_instrument.note_repeat
                    ui.selectable_value(&mut now, Some(r), r.label());
                }
            })
            .response
            .accessible_name("Note repeat")
            .on_hover_text(
                "Keys and pads held down play again at this rate, for rolls and hi-hat runs",
            );
        if now != *note_repeat {
            *note_repeat = now;
            *changed = true;
        }
    });
}

fn midi_out_picker(
    ui: &mut egui::Ui,
    midi_out: &mut Option<usize>,
    outputs: &[String],
    changed: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label("MIDI output");
        let current = midi_out
            .and_then(|p| outputs.get(p))
            .map_or("None", |s| s.as_str());
        egui::ComboBox::from_id_salt("midi-out")
            .selected_text(current)
            .show_ui(ui, |ui| {
                // ai: set_track_input.midi_out
                if ui.selectable_label(midi_out.is_none(), "None").clicked() && midi_out.is_some() {
                    *midi_out = None;
                    *changed = true;
                }
                for (p, name) in outputs.iter().enumerate() {
                    // ai: set_track_input.midi_out
                    if ui.selectable_label(*midi_out == Some(p), name).clicked()
                        && *midi_out != Some(p)
                    {
                        *midi_out = Some(p);
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

/// The semi-modular monosynth's settings in their groups, with a word
/// on how its external input is fed (from the mixer strip).
fn patch_twenty_controls(ui: &mut egui::Ui, slot: &mut Slot) -> bool {
    let Some(b) = slot.builtin_ref() else {
        return false;
    };
    let p = b.params().clone();
    for &(heading, from, to) in daw_instruments::patch_twenty::GROUPS {
        egui::CollapsingHeader::new(heading)
            .id_salt(("patch-twenty-group", heading))
            .default_open(from == 0)
            .show(ui, |ui| {
                if from == daw_instruments::patch_twenty::at::EXT_IN {
                    ui.label(
                        "Feeds the track's audio input through the filters and amplifier: \
                         pick the input on the mixer strip and turn on Monitor. Triggering \
                         starts the envelopes when the input gets loud.",
                    );
                }
                params_grid(ui, heading, slot.id, &p, from, to + 1)
            });
    }
    false
}

/// The controls of an instrument whose settings are only its `params`,
/// shown in `groups` of (heading, first, last).
fn grouped_controls(
    ui: &mut egui::Ui,
    slot: &mut Slot,
    salt: &str,
    groups: &[(&str, usize, usize)],
) -> bool {
    let Some(b) = slot.builtin_ref() else {
        return false;
    };
    let p = b.params().clone();
    for &(heading, from, to) in groups {
        egui::CollapsingHeader::new(heading)
            .id_salt((salt, "group", heading))
            .default_open(true)
            .show(ui, |ui| params_grid(ui, heading, slot.id, &p, from, to + 1));
    }
    false
}

/// The Acid Three bass line synth's controls: a word on how accent and
/// slide come from the notes, then its settings in groups.
fn acid_controls(ui: &mut egui::Ui, slot: &mut Slot) -> bool {
    ui.label(
        egui::RichText::new(
            "Notes at or above the accent velocity are accented; a note that starts while the one before is still held slides into it.",
        )
        .weak(),
    );
    grouped_controls(ui, slot, "acid", daw_instruments::acid::GROUPS)
}

/// A synth's controls where it has sounds made in code: the sounds,
/// then its settings in groups.
fn sound_controls(
    ui: &mut egui::Ui,
    slot: &mut Slot,
    salt: &str,
    groups: &[(&str, usize, usize)],
) -> bool {
    if let Some(b) = slot.builtin_ref() {
        ui.horizontal(|ui| {
            ui.label("Sound");
            // ai: set_instrument
            egui::ComboBox::from_id_salt((salt, "sound", slot.id))
                .selected_text("Choose a sound…")
                .show_ui(ui, |ui| {
                    for s in b.kind().sounds() {
                        // ai: set_instrument
                        if ui.selectable_label(false, s.name).clicked() {
                            b.set_sound(s.name);
                        }
                    }
                })
                .response
                .accessible_name("Sound")
                .on_hover_text("Sounds made from the settings: choosing one sets them all");
        });
    }
    grouped_controls(ui, slot, salt, groups)
}

/// The Lucky Seven's controls: its sounds made in code, a sound bank
/// to load and choose from, then its settings in groups, the operators'
/// closed to start with so the window isn't a wall of numbers.
fn lucky_seven_controls(ui: &mut egui::Ui, slot: &mut Slot, shown: &mut Shown) -> bool {
    let id = slot.id;
    let bank = slot.bank.clone();
    if let Some(b) = slot.builtin_ref() {
        ui.horizontal(|ui| {
            ui.label("Sound");
            // ai: set_instrument
            egui::ComboBox::from_id_salt(("lucky-seven", "sound", id))
                .selected_text("Choose a sound…")
                .show_ui(ui, |ui| {
                    for s in b.kind().sounds() {
                        // ai: set_instrument
                        if ui.selectable_label(false, s.name).clicked() {
                            b.set_sound(s.name);
                        }
                    }
                })
                .response
                .accessible_name("Sound")
                .on_hover_text("Sounds made from the settings: choosing one sets them all");
            // ai: load-synth-bank
            if ui
                .button("Load a bank…")
                .on_hover_text("Reads your own sounds from a file in the public six-operator FM bank format (.syx)")
                .clicked()
            {
                shown.cmd = Some(Cmd::LoadBank);
            }
        });
        if let Some(bank) = bank {
            ui.horizontal(|ui| {
                ui.label("From the bank");
                // ai: set_instrument
                egui::ComboBox::from_id_salt(("lucky-seven", "bank", id))
                    .selected_text("Choose a sound…")
                    .show_ui(ui, |ui| {
                        for v in &bank.voices {
                            // ai: set_instrument
                            if ui.selectable_label(false, &v.name).clicked() {
                                b.set_bank_voice(v);
                            }
                        }
                    })
                    .response
                    .accessible_name("Sound from the bank")
                    .on_hover_text(format!(
                        "{} sounds read from {}",
                        bank.voices.len(),
                        bank.file
                    ));
            });
        }
    }
    let Some(b) = slot.builtin_ref() else {
        return false;
    };
    let p = b.params().clone();
    for &(heading, from, to) in daw_instruments::lucky_seven::GROUPS {
        egui::CollapsingHeader::new(heading)
            .id_salt(("lucky-seven", "group", heading))
            .default_open(!heading.starts_with("Operator"))
            .show(ui, |ui| params_grid(ui, heading, id, &p, from, to + 1));
    }
    false
}

/// The June's controls: its sounds made in code, then its settings in
/// groups.
fn june_controls(ui: &mut egui::Ui, slot: &mut Slot) -> bool {
    if let Some(b) = slot.builtin_ref() {
        ui.horizontal(|ui| {
            ui.label("Sound");
            // ai: set_instrument
            egui::ComboBox::from_id_salt(("june-sound", slot.id))
                .selected_text("Choose a sound…")
                .show_ui(ui, |ui| {
                    for s in b.kind().sounds() {
                        // ai: set_instrument
                        if ui.selectable_label(false, s.name).clicked() {
                            b.set_sound(s.name);
                        }
                    }
                })
                .response
                .accessible_name("Sound")
                .on_hover_text("Sounds made from the settings: choosing one sets them all");
        });
    }
    grouped_controls(ui, slot, "june", daw_instruments::june::GROUPS)
}

/// Reads `reference` into a sound, telling the user if it couldn't be.
pub(crate) fn read_sound(reference: &str, cx: &Ctx, shown: &mut Shown) -> Option<Loaded> {
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
    ui.horizontal(|ui| {
        // ai: load-sfz
        if ui
            .button("Load an SFZ library…")
            .on_hover_text("Plays a sample library in the SFZ format, with its layers, round robins, mics and articulations")
            .clicked()
        {
            shown.cmd = Some(Cmd::LoadSfz);
        }
        if b.library().is_some() {
            // ai: unload-sfz
            if ui.button("Back to zones").clicked() {
                shown.cmd = Some(Cmd::UnloadSfz);
            }
        }
    });
    if let Some(lib) = b.library().cloned() {
        library_controls(ui, id, &p, &lib);
        return false;
    }
    if b.sfz_path().is_some() {
        ui.label(
            egui::RichText::new(format!(
                "Its SFZ library couldn't be read: {}",
                b.sfz_path().unwrap_or_default()
            ))
            .weak(),
        );
    }
    egui::CollapsingHeader::new("Sound")
        .default_open(true)
        .show(ui, |ui| {
            params_grid(ui, "sampler", id, &p, 0, sampler::at::ZONES)
        });
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong("Zones");
        if ui
            // ai: set_sound.slot
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
        // ai: edit_instrument.spread_zones
        if ui
            .button("Spread over the keys")
            .on_hover_text("Each zone from halfway to the one below to halfway to the one above")
            .clicked()
        {
            b.spread_zones();
        }
        // ai: normalise-zones
        if ui
            .button("Normalise levels")
            .on_hover_text("Each zone's level so its loudest moment peaks at -6 dB")
            .clicked()
        {
            shown.cmd = Some(Cmd::NormaliseZones);
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
                    // ai: view
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
            // ai: edit_instrument.remove_zone
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

/// A loaded SFZ library's settings: its level, the articulation, and a
/// fader for each controller it names (mic levels, say). Each is a
/// setting assistants set with set_parameter.
fn library_controls(ui: &mut egui::Ui, id: u64, p: &Params, lib: &daw_instruments::Library) {
    ui.label(format!(
        "{}: {} regions, {} sounds, {} MB in memory{}",
        lib.name(),
        lib.sfz.regions.len(),
        lib.sources.len(),
        lib.bytes >> 20,
        if lib.streamed {
            ", the rest from disk"
        } else {
            ""
        }
    ))
    .on_hover_text(lib.path.display().to_string());
    if !lib.problems.is_empty() {
        egui::CollapsingHeader::new(format!("{} problems", lib.problems.len()))
            .id_salt(("sfz-problems", id))
            .show(ui, |ui| {
                for pr in lib.problems.iter().take(20) {
                    ui.label(egui::RichText::new(pr).weak());
                }
            });
    }
    if !lib.sfz.unsupported.is_empty() {
        let list: Vec<String> = (lib.sfz.unsupported.iter())
            .map(|(k, n)| format!("{k} ({n})"))
            .collect();
        ui.label(egui::RichText::new("Some of its settings aren't played yet").weak())
            .on_hover_text(list.join(", "));
    }
    let row = |key: &str| p.index(key);
    egui::Grid::new(("sfz-main", id))
        .num_columns(2)
        .show(ui, |ui| {
            // ai: set_parameter
            param(ui, ("sfz", id), p, sampler::at::LEVEL);
            if let Some(i) = row("normalise") {
                // ai: set_parameter
                param(ui, ("sfz", id), p, i);
            }
            if let Some(i) = row("keyswitch") {
                ui.label("Articulation");
                let now = p.get(i) as u8;
                let switches = lib.sfz.key_switches();
                let shown = (switches.iter()).find(|(k, _)| *k == now).map_or_else(
                    || daw_instruments::params::note_name(now as i32),
                    |(_, n)| n.clone(),
                );
                // ai: set_parameter
                egui::ComboBox::from_id_salt(("sfz-switch", id))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (k, name) in &switches {
                            let text = format!(
                                "{name} ({})",
                                daw_instruments::params::note_name(*k as i32)
                            );
                            // ai: set_parameter
                            if ui.selectable_label(*k == now, text).clicked() {
                                p.set(i, *k as f64);
                            }
                        }
                    })
                    .response
                    .accessible_name("Articulation");
                ui.end_row();
            }
        });
    let ccs: Vec<usize> = (p.list().iter().enumerate())
        .filter(|(_, x)| x.key.starts_with("cc"))
        .map(|(i, _)| i)
        .collect();
    if !ccs.is_empty() {
        egui::CollapsingHeader::new(format!("Controls ({})", ccs.len()))
            .id_salt(("sfz-ccs", id))
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        egui::Grid::new(("sfz-cc", id))
                            .num_columns(2)
                            .show(ui, |ui| {
                                for i in ccs {
                                    // ai: set_parameter
                                    param(ui, ("sfz-cc", id), p, i);
                                }
                            });
                    });
            });
    }
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
pub(crate) fn pad_effects() -> impl Iterator<Item = daw_effects::Kind> {
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
    crate::drum_panel::drum_machine(ui, slot, routes, cx, shown)
}

/// Where each of an instrument's outputs in use goes: a bus, or
/// nowhere, so it plays through the track.
pub(crate) fn route_rows(
    ui: &mut egui::Ui,
    id: u64,
    in_use: &[(u32, String)],
    routes: &mut Vec<daw_project::InstrumentOutput>,
    cx: &Ctx,
    shown: &mut Shown,
) {
    egui::Grid::new(("routes", id))
        .num_columns(2)
        .show(ui, |ui| {
            for (o, name) in in_use {
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
                        // ai: set_instrument_output.to
                        if ui.selectable_label(now.is_none(), "Nowhere").clicked() {
                            pick = Some(None);
                        }
                        for (k, n) in cx.buses {
                            // ai: set_instrument_output.to
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

/// A plugin instrument's controls: where it runs, its own window,
/// presets, a crash, its settings, and where its extra outputs go.
fn plugin_controls(
    ui: &mut egui::Ui,
    slot: &mut Slot,
    routes: &mut Vec<daw_project::InstrumentOutput>,
    cx: &mut Ctx,
    shown: &mut Shown,
) {
    let id = slot.id;
    let in_use = slot.plugin_outputs();
    if let Inner::Missing(e) = &slot.inner {
        ui.label(
            egui::RichText::new(format!(
                "{} isn't available on this computer. It is kept as saved, and plays \
                 nothing here.",
                e.name
            ))
            .color(ui.visuals().warn_fg_color),
        );
        return;
    }
    let Some(fx) = slot.plugin_mut() else {
        return;
    };
    let did = crate::fx::plugin_instrument_controls(ui, fx, &cx.effects.presets_dir);
    if did.restart {
        match cx.effects.reload(fx, cx.rate) {
            Ok(()) => shown.changed = true,
            Err(e) => shown.message = Some(format!("{}: {e}", fx.name)),
        }
    }
    if did.message.is_some() {
        shown.message = did.message;
    }
    if in_use.is_empty() {
        return;
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong("Outputs").on_hover_text(
            "Each of the plugin's extra outputs can go to a bus in the mixer, with its own \
             fader and effects. One that goes nowhere plays through the track",
        );
        if ui
            // ai: instrument-output-buses
            .button("Put each output on a bus")
            .on_hover_text("Gives every extra output a bus in the mixer of its own")
            .clicked()
        {
            shown.cmd = Some(Cmd::InstrumentOutputBuses);
        }
    });
    route_rows(ui, id, &in_use, routes, cx, shown);
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
        slot.make_into(self.rate, &mut self.pending_instruments);
        t.instrument = Some(slot);
        self.tracks.push(t);
        self.instrument_open = Some(self.tracks.len() - 1);
        self.rebuild();
    }

    /// Plays the SFZ library at `path` on the sampler track commands act
    /// on, or on a new one. Reading a big library takes a few seconds.
    /// What was loaded, to tell the user.
    pub(crate) fn load_sfz(&mut self, path: &Path) -> Result<String, String> {
        let lib = daw_instruments::library::open(path)?;
        if lib.sfz.regions.is_empty() {
            return Err(format!("{}: no sounds in it", path.display()));
        }
        let i = match self.instrument_track(Some(Kind::Sampler)) {
            Some(i) => i,
            None => {
                self.add_instrument_track(Kind::Sampler);
                self.tracks.len() - 1
            }
        };
        let rate = self.rate;
        let mut pending = std::mem::take(&mut self.pending_instruments);
        let t = &mut self.tracks[i];
        let renamed = t.name.starts_with(Kind::Sampler.name());
        let slot = t
            .instrument
            .as_mut()
            .ok_or("The track has no instrument.")?;
        let b = slot
            .builtin_mut()
            .ok_or("The track's instrument isn't the sampler.")?;
        b.set_library(&path.to_string_lossy(), lib.clone());
        slot.renew(rate, &mut pending);
        if renamed {
            t.name = lib.name();
        }
        self.pending_instruments.extend(pending);
        self.instrument_open = Some(i);
        self.rebuild();
        Ok(library_summary(&lib))
    }

    /// Reads a bank of sounds in the public FM bank format onto the
    /// Lucky Seven on track `track` (else the first Lucky Seven track,
    /// else a new one), sets its first sound, and keeps the bank with
    /// the track's instrument for choosing from. What it read, or why
    /// it couldn't.
    pub(crate) fn load_bank(
        &mut self,
        path: &Path,
        track: Option<usize>,
    ) -> Result<String, String> {
        let bank = daw_instruments::lucky_seven::bank::open(path)?;
        let i = match track.or_else(|| self.instrument_track(Some(Kind::LuckySeven))) {
            Some(i) => i,
            None => {
                self.add_instrument_track(Kind::LuckySeven);
                self.tracks.len() - 1
            }
        };
        let t = self.tracks.get_mut(i).ok_or("There is no such track.")?;
        let slot = t
            .instrument
            .as_mut()
            .ok_or("The track has no instrument.")?;
        let b = (slot.builtin_ref())
            .filter(|b| b.kind() == Kind::LuckySeven)
            .ok_or("The track's instrument isn't the Lucky Seven.")?;
        let first = &bank.voices[0];
        b.set_bank_voice(first);
        let summary = format!(
            "Read {} sound{} from {}; playing {}.",
            bank.voices.len(),
            if bank.voices.len() == 1 { "" } else { "s" },
            path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned()
            ),
            first.name
        );
        slot.bank = Some(Arc::new(bank));
        self.instrument_open = Some(i);
        self.rebuild();
        Ok(summary)
    }

    /// The sampler's track stops playing its library.
    pub(crate) fn unload_sfz(&mut self) -> Result<(), String> {
        let i = self
            .instrument_track(Some(Kind::Sampler))
            .ok_or("Add a sampler track first.")?;
        let rate = self.rate;
        let mut pending = std::mem::take(&mut self.pending_instruments);
        if let Some(slot) = self.tracks[i].instrument.as_mut()
            && let Some(b) = slot.builtin_mut()
            && b.library().is_some()
        {
            b.clear_library();
            slot.renew(rate, &mut pending);
        }
        self.pending_instruments.extend(pending);
        self.rebuild();
        Ok(())
    }

    /// Sets the sampler's zones' levels so each peaks at −6 dBFS.
    pub(crate) fn normalise_zones(&mut self) -> Result<(), String> {
        let i = self
            .instrument_track(Some(Kind::Sampler))
            .ok_or("Add a sampler track first.")?;
        if let Some(b) = self.tracks[i]
            .instrument
            .as_mut()
            .and_then(Slot::builtin_mut)
        {
            b.normalise_zones();
        }
        self.rebuild();
        Ok(())
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

    /// The track with a plugin instrument with extra outputs that
    /// commands act on: the one whose instrument window is open, else
    /// the first.
    pub(crate) fn plugin_instrument_track(&self) -> Option<usize> {
        let fits = |t: &crate::Track| {
            t.kind.plays_notes()
                && (t.instrument.as_ref()).is_some_and(|i| !i.plugin_outputs().is_empty())
        };
        self.instrument_open
            .filter(|&i| self.tracks.get(i).is_some_and(fits))
            .or_else(|| self.tracks.iter().position(fits))
    }

    /// Gives each extra output of a plugin instrument a bus of its own,
    /// after its track, keeping the buses outputs already go to.
    pub(crate) fn plugin_output_buses(&mut self) -> Result<(), String> {
        let i = self
            .plugin_instrument_track()
            .ok_or("Choose a plugin instrument with several outputs first.")?;
        let outputs = (self.tracks[i].instrument.as_ref())
            .map(Slot::plugin_outputs)
            .unwrap_or_default();
        let name = self.tracks[i].name.clone();
        let mut at = i + 1;
        while at < self.tracks.len()
            && (self.tracks[i].instrument_outputs.iter()).any(|o| o.to == self.tracks[at].key)
        {
            at += 1;
        }
        let mut routes = Vec::new();
        for (output, out_name) in outputs {
            let routed = (self.tracks[i].instrument_outputs.iter())
                .any(|o| o.output == output && self.bus_index(&o.to).is_some());
            if routed {
                continue;
            }
            let mut bus = crate::Track::new(format!("{name}: {out_name}"));
            bus.kind = daw_project::TrackKind::Bus;
            bus.parent = self.tracks[i].parent.clone();
            routes.push(daw_project::InstrumentOutput {
                output,
                to: bus.key.clone(),
            });
            self.tracks.insert(at, bus);
            at += 1;
        }
        let t = &mut self.tracks[i];
        let taken: Vec<u32> = routes.iter().map(|r| r.output).collect();
        t.instrument_outputs.retain(|o| !taken.contains(&o.output));
        t.instrument_outputs.extend(routes);
        t.instrument_outputs.sort_by_key(|o| o.output);
        self.rebuild();
        Ok(())
    }

    /// The instrument pane, showing track `i`.
    pub(crate) fn instrument_panel(&mut self, ui: &mut egui::Ui, i: usize) {
        let outputs = self.midi.ports.outputs.clone();
        let Some(key) = (self.tracks.get(i))
            .filter(|t| t.kind.plays_notes())
            .map(|t| t.key.clone())
        else {
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
        let shown = panel(
            ui,
            &mut self.tracks[i],
            Ctx {
                outputs: &outputs,
                buses: &buses,
                rate,
                pending: &mut pending,
                read: &read,
                effects: &mut self.effects,
                scale: self.scale,
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
            let ctx = ui.ctx().clone();
            self.run(&ctx, cmd);
        }
        if let Some(edit) = shown.layer {
            self.layer_edit(i, edit);
        }
    }
}

// ---- For assistants ----

pub(crate) const TOOLS: &[crate::agent::Tool] = &[crate::agent::Tool {
    name: "edit_instrument",
    description: "What a track's instrument window does beyond its settings (which are \
                  set_parameter's): note repeat, how often held keys and pads play again; \
                  for a sampler, spreading its zones over the keys by their root notes, and \
                  removing a zone (the zones after it move down one); for the loop \
                  slicer, its loop (a sound file, cut where its hits start), and cutting it \
                  again at the hits or into equal parts. Answers with the zones or slices \
                  there are now. restart starts a plugin instrument that stopped again, \
                  with its last saved settings.",
    access: crate::agent::Access::Edit,
    schema: || {
        let rates: Vec<&str> = std::iter::once("off")
            .chain(daw_project::StepRate::ALL.iter().map(|r| r.label()))
            .collect();
        serde_json::json!({"type": "object", "properties": {
            "track": {"type": "string"},
            "restart": {"type": "boolean", "description": "Start the stopped plugin instrument again (get_instrument says why as stopped)."},
            "layer": {"type": "string", "description": "With restart: the layer whose plugin to start again."},
            "note_repeat": {"type": "string", "enum": rates},
            "spread_zones": {"type": "boolean"},
            "remove_zone": {"type": "integer", "description": "The zone's number, from 0 (zone0)."},
            "loop": {"type": "string", "description": "The slicer's loop: a sound file's path."},
            "slice": {"type": "object", "description": "Cut the slicer's loop again: at_hits or parts.", "properties": {
                "at_hits": {"type": "number", "description": "Sensitivity, 0 to 1: higher finds quieter hits too."},
                "parts": {"type": "integer", "description": "Into this many equal parts."}
            }}
        }, "required": ["track"]})
    },
}];

impl crate::App {
    pub(crate) fn instrument_tool(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<Result<(serde_json::Value, Option<String>), String>> {
        (name == "edit_instrument").then(|| self.edit_instrument(args))
    }

    fn edit_instrument(
        &mut self,
        args: &serde_json::Value,
    ) -> Result<(serde_json::Value, Option<String>), String> {
        use serde_json::json;
        let key = args["track"].as_str().ok_or("Give the track's id.")?;
        let i = (self.tracks.iter())
            .position(|t| t.key == key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        if !self.tracks[i].kind.plays_notes() {
            return Err(format!("{} plays no notes.", self.tracks[i].name));
        }
        // Check everything before changing anything.
        let repeat = match args["note_repeat"].as_str() {
            None => None,
            Some("off") => Some(None),
            Some(r) => Some(Some(
                (daw_project::StepRate::ALL.into_iter())
                    .find(|x| x.label() == r)
                    .ok_or_else(|| format!("Note repeat is off or one of the rates, not {r}."))?,
            )),
        };
        let kind = self.tracks[i].instrument.as_ref().and_then(Slot::kind);
        let zones = args["spread_zones"].as_bool() == Some(true) || !args["remove_zone"].is_null();
        if zones && kind != Some(Kind::Sampler) {
            return Err("Zones are a sampler's.".into());
        }
        if let Some(z) = args["remove_zone"].as_u64() {
            let n = (self.tracks[i].instrument.as_ref())
                .and_then(Slot::builtin_ref)
                .map_or(0, |b| b.zones());
            if z as usize >= n {
                return Err(format!("The sampler has {n} zones, numbered from 0."));
            }
        }
        let slicing = !args["loop"].is_null() || !args["slice"].is_null();
        if slicing && kind != Some(Kind::Slicer) {
            return Err("The loop and its slices are the loop slicer's.".into());
        }
        let sound = match args["loop"].as_str() {
            None => None,
            Some(path) => {
                let read = self.sound_reader();
                Some(Arc::new(
                    daw_instruments::load(path, &read)
                        .map_err(|e| format!("Couldn't read the sound: {e}"))?,
                ))
            }
        };
        let slice = &args["slice"];
        let at_hits = match slice["at_hits"].as_f64() {
            Some(x) if !(0.0..=1.0).contains(&x) => {
                return Err("The sensitivity goes from 0 to 1.".into());
            }
            x => x.map(|x| x as f32),
        };
        let parts = match slice["parts"].as_u64() {
            Some(n) if n == 0 || n as usize > slicer::MAX_SLICES => {
                return Err(format!("From 1 to {} parts.", slicer::MAX_SLICES));
            }
            n => n.map(|n| n as usize),
        };

        if args["restart"].as_bool() == Some(true) {
            let rate = self.rate;
            let t = &mut self.tracks[i];
            let slot = match args["layer"].as_str() {
                None => t.instrument.as_mut(),
                Some(l) => Some(
                    &mut (t.layers.iter_mut().find(|x| x.key == l))
                        .ok_or_else(|| format!("The track has no layer {l}."))?
                        .slot,
                ),
            };
            let fx = (slot.and_then(Slot::plugin_mut)).ok_or("Only a plugin restarts.")?;
            if fx.crashed.is_none() {
                return Err(format!(
                    "{} is running; only a stopped plugin restarts.",
                    fx.name
                ));
            }
            self.effects.reload(fx, rate)?;
        }
        let t = &mut self.tracks[i];
        if let Some(r) = repeat {
            t.note_repeat = r;
        }
        let slot = t.instrument.as_mut();
        let mut answer = json!({"note_repeat": t.note_repeat.map_or("off", |r| r.label())});
        if let Some(slot) = slot {
            let sensitivity = at_hits.unwrap_or(slot.sensitivity);
            if let Some(n) = parts {
                slot.parts = n;
            }
            slot.sensitivity = sensitivity;
            if let Some(b) = slot.builtin_mut() {
                if let Some(z) = args["remove_zone"].as_u64() {
                    b.remove_zone(z as usize);
                }
                if args["spread_zones"].as_bool() == Some(true) {
                    b.spread_zones();
                }
                if let Some(l) = sound {
                    b.set_loop(l, sensitivity);
                }
                if at_hits.is_some() {
                    b.slice_at_hits(sensitivity);
                } else if let Some(n) = parts {
                    b.slice_equally(n);
                }
                match kind {
                    Some(Kind::Sampler) => answer["zones"] = json!(b.zones()),
                    Some(Kind::Slicer) => answer["slices"] = json!(b.slices().len()),
                    _ => {}
                }
            }
        }
        self.rebuild();
        Ok((answer, Some("Change an instrument".into())))
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
            .filter_map(|i| Some((i.id, i.make(app.rate)?)))
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
        h.state_mut().workspace.full = Some(crate::workspace::Pane::Instrument);
        h.run();
        h.get_by_label("Show: Outputs").click();
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
        h.state_mut().workspace.full = Some(crate::workspace::Pane::Instrument);
        h.run();
        assert!(h.query_by_label("Make notes that play the loop").is_some());
        // Each slice has its own settings.
        assert!(h.query_all_by_label_contains("Backwards").next().is_some());
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

#[cfg(test)]
mod plugin_tests {
    use std::path::Path;

    use daw_engine::engine::render_with;
    use daw_engine::time::Frames;
    use daw_plugins::Sandbox;
    use daw_plugins::scan::{Available, BundleEntry, scan_here};
    use daw_project::{Note, NoteClip, PPQ};
    use daw_test_plugin::{CLAP_SYNTH_NAME, SYNTH_NAME};
    use egui_kittest::Harness;

    use super::Slot;
    use crate::commands::Cmd;
    use crate::freeze::test_plugin;
    use crate::{App, Track};

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    /// Lists the plugins in `file` as installed, and returns the one
    /// called `name`.
    fn install(app: &mut App, file: &Path, name: &str) -> Available {
        app.effects.helper = Some(test_plugin::helper());
        let classes = scan_here(file).unwrap();
        app.effects.list.bundles = vec![BundleEntry {
            path: file.to_owned(),
            modified: 0,
            result: Ok(classes),
        }];
        (app.effects.list.plugins().into_iter())
            .find(|a| a.class.name == name)
            .unwrap()
    }

    /// A second of the mix, rendered as an export would: the frames of
    /// the left channel that sound.
    fn sounding(app: &mut App) -> Vec<usize> {
        let mut s = app.session();
        let mut keep = Vec::new();
        let mut instruments = Vec::new();
        for (k, t) in app.tracks.iter().enumerate() {
            let Some(i) = &t.instrument else { continue };
            if let Some((p, inst)) = i.fresh(&mut app.effects, app.rate).unwrap() {
                // Made for rendering: no helper's block of delay.
                if let (Some(p), Some(spec)) = (&p, &mut s.tracks[k].instrument) {
                    spec.latency = p.latency();
                }
                keep.extend(p);
                instruments.push((i.id, inst));
            }
        }
        let g = s
            .compile_all(daw_audio_io::MAX_BLOCK, &mut Vec::new(), &mut instruments)
            .unwrap();
        let mut left = Vec::new();
        render_with(g, Frames(0), app.rate as usize, 256, |l, _| {
            left.extend_from_slice(l);
            true
        });
        (left.iter().enumerate())
            .filter(|(_, x)| x.abs() > 1e-4)
            .map(|(i, _)| i)
            .collect()
    }

    /// A drum track: a note on channel 1 at the start, and one on
    /// channel 10 (the test instruments' "Drums" output) on beat 2.
    fn drum_track(app: &mut App, a: &Available) -> Track {
        let mut t = Track::instrument("Drums".into());
        t.instrument = Some(Slot::new_plugin(&mut app.effects, a, app.rate).unwrap());
        let mut clip = NoteClip::new("nc".into(), "Beat".into(), 0, 4 * PPQ);
        let mut drum = Note::new(PPQ, PPQ / 4, 36, 127);
        drum.channel = 9;
        clip.notes = vec![Note::new(0, PPQ / 4, 60, 127), drum];
        t.note_clips.push(clip);
        t
    }

    #[test]
    fn a_plugin_instrument_plays_and_its_extra_output_goes_to_a_bus() {
        for (file, name) in [
            (test_plugin::bundle(), SYNTH_NAME),
            (test_plugin::clap(), CLAP_SYNTH_NAME),
        ] {
            for sandbox in [Sandbox::InApp, Sandbox::Shared] {
                let what = format!("{name}, {sandbox:?}");
                let mut h = harness();
                let ctx = h.ctx.clone();
                let app = h.state_mut();
                let a = install(app, &file, name);
                app.effects.list.set_sandbox(&a.class.cid, sandbox);
                let t = drum_track(app, &a);
                assert!(t.instrument.as_ref().unwrap().is_plugin());
                app.tracks.push(t);
                app.rebuild();
                let both = sounding(app);
                assert_eq!(both.len(), 2, "{what}: {both:?}");

                // The drums go to a bus of their own: muting it leaves
                // the other note, and muting the instrument's track
                // leaves the drums.
                assert!(app.can_run(Cmd::InstrumentOutputBuses), "{what}");
                app.run(&ctx, Cmd::InstrumentOutputBuses);
                assert_eq!(app.tracks.len(), 2, "{what}: {:?}", app.message);
                assert!(app.tracks[1].kind.is_bus());
                assert_eq!(app.tracks[1].name, "Drums: Drums");
                assert_eq!(app.tracks[0].instrument_outputs.len(), 1);
                assert_eq!(sounding(app), both, "{what}");
                app.tracks[1].mix.mute = true;
                assert_eq!(sounding(app), vec![both[0]], "{what}");
                app.tracks[1].mix.mute = false;
                app.tracks[0].mix.mute = true;
                assert_eq!(sounding(app), vec![both[1]], "{what}");
                app.tracks[0].mix.mute = false;
                // Run again, it makes no second bus.
                app.run(&ctx, Cmd::InstrumentOutputBuses);
                assert_eq!(app.tracks.len(), 2, "{what}");

                // Saved and opened again, it is all there.
                app.write_plugin_states();
                let p = app.proj.doc.get();
                let saved = p.tracks[0].instrument.clone().unwrap();
                assert!(saved.is_plugin(), "{what}");
                assert!(saved.state.is_some(), "{what}");
                let mut h2 = harness();
                let app2 = h2.state_mut();
                install(app2, &file, name);
                app2.effects.list.set_sandbox(&a.class.cid, sandbox);
                app2.apply(p);
                assert_eq!(app2.tracks.len(), 2, "{what}");
                assert!(
                    (app2.tracks[0].instrument.as_ref()).is_some_and(|i| i.plugin().is_some()),
                    "{what}: {:?}",
                    app2.message
                );
                assert_eq!(app2.tracks[0].instrument_outputs.len(), 1);
                assert_eq!(sounding(app2), both, "{what}");

                // Sent nowhere, the drums play through the track again.
                let app = h.state_mut();
                app.tracks[0].instrument_outputs.clear();
                app.tracks[0].mix.mute = false;
                app.tracks[1].mix.mute = true;
                assert_eq!(sounding(app), both, "{what}");
            }
        }
    }

    #[test]
    fn a_plugin_that_is_not_here_is_kept_as_saved_with_its_outputs() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        let app = h.state_mut();
        let a = install(app, &test_plugin::bundle(), SYNTH_NAME);
        let t = drum_track(app, &a);
        app.tracks.push(t);
        app.run(&ctx, Cmd::InstrumentOutputBuses);
        app.write_plugin_states();
        let mut p = app.proj.doc.get();
        let inst = p.tracks[0].instrument.as_mut().unwrap();
        inst.class = vec![0xee; 16];
        inst.bundle = "/nowhere/gone.vst3".into();
        let saved = p.clone();

        let mut h2 = harness();
        let app2 = h2.state_mut();
        app2.apply(p);
        let slot = app2.tracks[0].instrument.as_ref().unwrap();
        assert!(slot.is_plugin() && slot.plugin().is_none());
        assert!(sounding(app2).is_empty());
        // Saved again unchanged, settings and outputs and all.
        app2.write_plugin_states();
        let back = app2.proj.doc.get();
        assert_eq!(back.tracks[0].instrument, saved.tracks[0].instrument);
        assert_eq!(
            back.tracks[0].instrument_outputs,
            saved.tracks[0].instrument_outputs
        );
    }

    #[test]
    fn a_layer_can_be_a_plugin_instrument() {
        let mut h = harness();
        let app = h.state_mut();
        let a = install(app, &test_plugin::clap(), CLAP_SYNTH_NAME);
        let t = drum_track(app, &a);
        app.tracks.push(t);
        app.rebuild();
        let alone = sounding(app);
        // The plugin becomes the first layer, and plays on.
        app.add_layer(0);
        assert_eq!(app.tracks[0].layers.len(), 2);
        assert!(app.tracks[0].layers[0].slot.plugin().is_some());
        app.write_plugin_states();
        let p = app.proj.doc.get();
        assert!(p.tracks[0].layers[0].instrument.state.is_some());
        assert!(!alone.is_empty());
    }

    #[test]
    fn an_assistant_spreads_and_removes_zones_slices_loops_and_repeats_notes() {
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        d.command("add-sampler-track");
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        for (z, sound, root) in [
            (0, "made:keys", 48.0),
            (1, "made:pluck", 60.0),
            (2, "made:keys", 72.0),
        ] {
            d.call(
                "set_sound",
                json!({"track": t, "slot": format!("zone{z}"), "sound": sound}),
            )
            .unwrap();
            d.call(
                "set_parameter",
                json!({"track": t, "param": format!("instrument.zone{z}.root"), "value": root}),
            )
            .unwrap();
        }
        let r = d
            .call(
                "edit_instrument",
                json!({"track": t, "spread_zones": true, "note_repeat": "1/8"}),
            )
            .unwrap();
        assert_eq!(r["zones"], 3);
        assert_eq!(r["note_repeat"], "1/8");
        assert_eq!(d.project()["tracks"][0]["note_repeat"], "1/8");
        let zone = |d: &mut crate::agent::driver::Driver, z: usize, k: usize| {
            let a = d.app();
            let b = a.tracks[0]
                .instrument
                .as_ref()
                .unwrap()
                .builtin_ref()
                .unwrap();
            b.params().get(daw_instruments::zone_at(z, k))
        };
        let (lo, hi) = (
            daw_instruments::sampler::zone::LO,
            daw_instruments::sampler::zone::HI,
        );
        assert!(
            zone(&mut d, 0, hi) < zone(&mut d, 1, lo),
            "spread out over the keys"
        );
        let r = d
            .call("edit_instrument", json!({"track": t, "remove_zone": 1}))
            .unwrap();
        assert_eq!(r["zones"], 2);
        assert_eq!(
            zone(&mut d, 1, daw_instruments::sampler::zone::ROOT),
            72.0,
            "the one after moved down"
        );
        assert!(
            d.call("edit_instrument", json!({"track": t, "remove_zone": 5}))
                .is_err()
        );
        assert!(
            d.call(
                "edit_instrument",
                json!({"track": t, "slice": {"parts": 4}})
            )
            .is_err()
        );
        d.call("edit_instrument", json!({"track": t, "note_repeat": "off"}))
            .unwrap();
        assert!(d.project()["tracks"][0]["note_repeat"].is_null());

        d.command("add-slicer-track");
        let s = d.project()["tracks"][1]["id"].as_str().unwrap().to_string();
        let r = d
            .call("edit_instrument", json!({"track": s, "loop": "made:loop"}))
            .unwrap();
        assert!(r["slices"].as_u64().unwrap() > 1, "{r}");
        let r = d
            .call(
                "edit_instrument",
                json!({"track": s, "slice": {"parts": 8}}),
            )
            .unwrap();
        assert_eq!(r["slices"], 8);
        let r = d
            .call(
                "edit_instrument",
                json!({"track": s, "slice": {"at_hits": 0.9}}),
            )
            .unwrap();
        assert!(r["slices"].as_u64().unwrap() > 1, "{r}");
        assert!(
            d.call("edit_instrument", json!({"track": s, "spread_zones": true}))
                .is_err()
        );
        assert!(
            d.call(
                "edit_instrument",
                json!({"track": s, "slice": {"at_hits": 2}})
            )
            .is_err()
        );
    }
}

/// The panel kit on the poly-synth, driven through the assistant
/// interface and the controls themselves.
#[cfg(test)]
mod kit_tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    fn cutoff(d: &mut Driver, t: &str) -> f64 {
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == "instrument.cutoff")
            .map(|p| p["value"].as_f64().unwrap())
            .unwrap()
    }

    /// Each classic synth's own character is drawn on its unit and
    /// reported to an assistant; the house synth has none.
    #[test]
    fn a_synths_character_draws_on_its_unit_and_an_assistant_reads_it() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "blue-wave", "name": "Keys"}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(
            info["instrument"]["character"],
            json!({"knob": "flat", "switch": "lit", "module": "boxed", "screen": "lcd",
                   "lettering": "readout", "band": "stripe", "plate": "#2b4c8e",
                   "accent": "#7fe0ff", "cheeks": false}),
            "{info}"
        );
        // The name plate is still read by its name, whatever its
        // lettering, and the character is cleared once the unit is drawn.
        d.h.get_by_label("BLUE WAVE");
        assert_eq!(
            crate::character::current(&d.h.ctx),
            crate::character::Character::PLAIN
        );
        let s = d
            .call("add_track", json!({"kind": "poly-synth", "name": "House"}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        let info = d.call("get_instrument", json!({"track": s})).unwrap();
        assert_eq!(info["instrument"]["character"], json!(null), "{info}");
    }

    #[test]
    fn the_poly_synth_panel_is_a_rack_unit_an_assistant_sees_and_steers() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "poly-synth", "name": "Keys"}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();

        // The header, the modules' knobs and switches, and the rack.
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["sound"], json!(null), "{info}");
        assert_eq!(
            info["rack"],
            json!([{"unit": "instrument", "name": "Synth", "view": "shown"}])
        );
        d.h.get_by_label("Cutoff");
        d.h.get_by_label("Oscillator 1: Saw");
        d.h.get_by_label("Show in rack");

        // A switch sets the setting an assistant reads.
        d.h.get_by_label("Oscillator 1: Sine").click();
        d.h.run();
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        let osc1 = (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == "instrument.osc1")
            .unwrap();
        assert_eq!(osc1["value"], json!(0.0), "{osc1}");

        // The sound buttons step through the synth's sounds, by click
        // and by command, and get_instrument says which is playing.
        d.h.get_by_label("Next sound").click();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["sound"], "Warm pad", "{info}");
        d.command("next-sound");
        d.h.run();
        assert_eq!(
            d.call("get_instrument", json!({"track": t})).unwrap()["sound"],
            "Bright lead"
        );
        d.command("previous-sound");
        d.h.run();
        assert_eq!(
            d.call("get_instrument", json!({"track": t})).unwrap()["sound"],
            "Warm pad"
        );
        let kept = cutoff(&mut d, &t);

        // Compare: an edit, then the sound as it was, then the edit.
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.cutoff", "value": 500.0}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(
            d.call("get_instrument", json!({"track": t})).unwrap()["sound"],
            json!(null),
            "edited, so no longer the sound"
        );
        d.h.get_by_label("Compare").click();
        d.h.run();
        assert_eq!(cutoff(&mut d, &t), kept);
        d.command("compare-sound");
        d.h.run();
        assert_eq!(cutoff(&mut d, &t), 500.0);

        // The rack view: collapsed to a strip, hidden, and shown again,
        // by tool and from the rail's menu; kept in the project.
        d.call(
            "set_rack",
            json!({"track": t, "unit": "instrument", "view": "collapsed"}),
        )
        .unwrap();
        d.h.run();
        assert!(
            d.h.query_by_label("Cutoff").is_none(),
            "a strip has no knobs"
        );
        assert_eq!(
            d.call("get_instrument", json!({"track": t})).unwrap()["rack"][0]["view"],
            "collapsed"
        );
        assert_eq!(
            d.app().project().tracks[0].rack,
            vec![daw_project::RackUnit {
                unit: "instrument".into(),
                view: daw_project::RackView::Collapsed
            }],
            "kept in the project"
        );
        d.call(
            "set_rack",
            json!({"track": t, "unit": "all", "view": "shown"}),
        )
        .unwrap();
        d.h.run();
        d.h.get_by_label("Cutoff");
        d.h.get_by_label("Show in rack").click();
        d.h.run();
        d.h.get_by_label("Synth").click();
        d.h.run();
        assert_eq!(
            d.call("get_instrument", json!({"track": t})).unwrap()["rack"][0]["view"],
            "hidden"
        );
        assert!(d.h.query_by_label("Cutoff").is_none());
        let e = d
            .call(
                "set_rack",
                json!({"track": t, "unit": "nothing", "view": "shown"}),
            )
            .unwrap_err();
        assert!(e.contains("no unit"), "{e}");
    }

    fn param(d: &mut Driver, t: &str, name: &str) -> f64 {
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == name)
            .map(|p| p["value"].as_f64().unwrap())
            .unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn the_loop_slicer_panel_is_a_rack_unit_an_assistant_sees_and_steers() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "slicer", "name": "Beat"}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();

        // The loop, the slice and playing modules, in the rack.
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["rack"][0]["view"], "shown", "{info}");
        for label in [
            "Sensitivity",
            "Parts",
            "Cut at the hits",
            "Cut into equal parts",
            "Slice",
            "Next slice",
            "Low-pass",
            "Backwards: Off",
            "Notes: Play the slice",
            "Attack",
            "Loop length in beats",
            "Make notes that play the loop",
        ] {
            assert!(
                d.h.query_all_by_label(label).next().is_some(),
                "no {label} on the panel"
            );
        }

        // An assistant cuts the loop again; the panel says so.
        d.call(
            "edit_instrument",
            json!({"track": t, "slice": {"parts": 4}}),
        )
        .unwrap();
        d.h.run();
        assert!(d.h.query_by_label_contains("4 slices").is_some());

        // The slice module shows the slice picked, whose settings an
        // assistant reads: next slice, then its Backwards switch.
        d.h.get_by_label("Next slice").click();
        d.h.run();
        d.h.get_by_label("Backwards: On").click();
        d.h.run();
        assert_eq!(param(&mut d, &t, "instrument.slice01.reverse"), 1.0);
        assert_eq!(param(&mut d, &t, "instrument.slice00.reverse"), 0.0);

        // A playing switch sets what an assistant reads, and the other
        // way about.
        d.h.get_by_label("Notes: Stop with the note").click();
        d.h.run();
        assert_eq!(param(&mut d, &t, "instrument.mode"), 1.0);
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.mode", "value": 0.0}),
        )
        .unwrap();
        d.h.run();
        d.h.get_by_label("Notes: Play the slice");

        // Cutting from the panel: at the loop's eight hits, then into
        // the four parts the assistant asked for, which the panel keeps.
        d.h.get_by_label("Cut at the hits").click();
        d.h.run();
        assert!(d.h.query_by_label_contains("8 slices").is_some());
        d.h.get_by_label("Cut into equal parts").click();
        d.h.run();
        assert!(d.h.query_by_label_contains("4 slices").is_some());

        // The notes that play the loop, by button and by command.
        d.h.get_by_label("Make notes that play the loop").click();
        d.h.run();
        d.command("slice-notes");
        d.h.run();
        assert_eq!(d.app().tracks[0].note_clips.len(), 2);

        // Collapsed to a strip by the assistant: no knobs.
        d.call(
            "set_rack",
            json!({"track": t, "unit": "instrument", "view": "collapsed"}),
        )
        .unwrap();
        d.h.run();
        assert!(d.h.query_by_label("Sensitivity").is_none());
    }
}

/// The one-voice synths' panels on the kit (UI polish step 5), driven
/// through the assistant interface and the controls themselves.
#[cfg(test)]
mod mono_panel_tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    fn value(d: &mut Driver, t: &str, param: &str) -> f64 {
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == param)
            .and_then(|p| p["value"].as_f64())
            .unwrap_or_else(|| panic!("no {param} in {p}"))
    }

    fn info(d: &mut Driver, t: &str) -> Value {
        d.call("get_instrument", json!({"track": t})).unwrap()
    }

    /// Adds a track of `kind` and shows its instrument full size.
    fn open(d: &mut Driver, kind: &str, index: usize) -> String {
        let t = d
            .call("add_track", json!({"kind": kind, "name": kind}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(index);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();
        t
    }

    #[test]
    fn the_mono_synths_panels_are_rack_units_an_assistant_sees_and_steers() {
        let mut d = Driver::new();

        // Model Mono: rotary switches for shape and footage, segments
        // for the priority, and its sounds from the header.
        let mono = open(&mut d, "model-mono", 0);
        assert_eq!(info(&mut d, &mono)["rack"][0]["view"], "shown");
        d.h.get_by_label("Osc 1 shape");
        d.h.get_by_label("Osc 3 range");
        d.h.get_by_label("Emphasis");
        d.h.get_by_label("Note priority: Low").click();
        d.h.run();
        assert_eq!(value(&mut d, &mono, "instrument.priority"), 0.0);
        d.h.get_by_label("Release takes the decay time: Short")
            .click();
        d.h.run();
        assert_eq!(value(&mut d, &mono, "instrument.decay_release"), 0.0);
        d.h.get_by_label("Next sound").click();
        d.h.run();
        assert_eq!(info(&mut d, &mono)["sound"], "Fat bass");
        d.call(
            "set_parameter",
            json!({"track": mono, "param": "instrument.osc1_range", "value": 1.0}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(info(&mut d, &mono)["sound"], json!(null), "edited");

        // Acid Three: two units, its wave switch, its new sounds.
        let acid = open(&mut d, "acid-three", 1);
        d.h.get_by_label("Accent from velocity");
        d.h.get_by_label("Waveform: Square").click();
        d.h.run();
        assert_eq!(value(&mut d, &acid, "instrument.wave"), 1.0);
        d.command("next-sound");
        d.h.run();
        assert_eq!(info(&mut d, &acid)["sound"], "Squelch line");
        assert!((value(&mut d, &acid, "instrument.resonance") - 0.8).abs() < 1e-6);
        let sounds = info(&mut d, &acid)["instrument"]["sounds"].clone();
        assert!(
            sounds.to_string().contains("Squelch line"),
            "the assistant sees the sounds: {sounds}"
        );

        // Oh-One: the footage and sub shape on short segments that a
        // screen reader still hears by the setting's name.
        let oh = open(&mut d, "oh-one", 2);
        d.h.get_by_label("Range: 16'").click();
        d.h.run();
        assert_eq!(value(&mut d, &oh, "instrument.range"), 0.0);
        d.h.get_by_label("Sub shape: 2 narrow").click();
        d.h.run();
        assert_eq!(value(&mut d, &oh, "instrument.sub_shape"), 2.0);
        d.h.get_by_label("Amplifier follows: Gate").click();
        d.h.run();
        assert_eq!(value(&mut d, &oh, "instrument.amp"), 1.0);
        d.h.get_by_label("LFO shape: Random");
        d.h.get_by_label("Width moved by: Env");
        d.command("previous-sound");
        d.h.run();
        assert_eq!(info(&mut d, &oh)["sound"], "Resonant sweep");

        // Collapsed to a strip by tool, it has no knobs.
        d.call(
            "set_rack",
            json!({"track": oh, "unit": "instrument", "view": "collapsed"}),
        )
        .unwrap();
        d.h.run();
        assert!(d.h.query_by_label("Range: 16'").is_none());
    }
}
