// SPDX-License-Identifier: GPL-3.0-or-later
//! Layered instruments (roadmap 3.5): several instruments on one
//! instrument track, sharing its notes. Each layer plays the notes in
//! its key and velocity ranges, moved by its transpose, through its own
//! effects, at its own level and pan; a keyboard split is two layers
//! with ranges side by side, and a doubled sound is two playing every
//! note.
//!
//! A layer's effects are the track's effects placed in it, as effects
//! sit in an effect container's chains: the effects window shows each
//! layer as a chain, before the track's own. A layer's settings are
//! shared with the audio thread and change as they move.
//!
//! Every change here is an [`App`] method, so the instrument window, the
//! command search and anything driving the app from outside make the
//! same change.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use daw_engine::graph::{InsertSpec, Place};
use daw_engine::layers::{LayerControls, LayerSpec, MAX_TRANSPOSE};
use eframe::egui;

use crate::App;
use crate::fx::FxSlot;
use crate::instrument::{Ctx, Reader, Shown, Slot};

/// Layer ids, unique for the life of the process, kept apart from the
/// effects' ids since effects name a layer as their container.
static NEXT_ID: AtomicU64 = AtomicU64::new(1 << 62);

/// One layer of a track's layered instrument.
pub struct Layer {
    /// Its id in the project document; the layer's effects name it.
    pub key: String,
    /// Matches the engine's layer id.
    pub id: u64,
    pub slot: Slot,
    /// Its level, pan, mute, ranges and transpose, shared with the audio
    /// thread.
    pub controls: Arc<LayerControls>,
}

impl Layer {
    /// A layer playing every note through `slot`.
    pub fn new(slot: Slot) -> Layer {
        Layer {
            key: daw_project::new_id(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            slot,
            controls: Arc::new(LayerControls::default()),
        }
    }

    /// The layer as the project has it, with a fresh instrument whose
    /// sounds are read with `read` (a plugin's made by `effects`). Also
    /// returns the sounds and plugins that couldn't be had.
    pub fn restore(
        l: &daw_project::Layer,
        read: Reader,
        effects: &mut crate::fx::Effects,
        rate: u32,
    ) -> (Layer, Vec<String>) {
        let (slot, missing) = Slot::restore_any(&l.instrument, read, effects, rate);
        let mut layer = Layer::new(slot);
        layer.key = l.id.clone();
        layer.set(l);
        (layer, missing)
    }

    /// Takes the project's settings (not its instrument).
    pub fn set(&self, l: &daw_project::Layer) {
        let c = &self.controls;
        c.set_gain_db(l.gain_db as f32);
        c.set_pan(l.pan as f32);
        c.set_mute(l.mute);
        c.set_keys(l.keys.0, l.keys.1);
        c.set_velocities(l.velocities.0, l.velocities.1);
        c.set_transpose(l.transpose);
    }

    /// As the project stores it.
    pub fn value(&self) -> daw_project::Layer {
        let c = &self.controls;
        daw_project::Layer {
            id: self.key.clone(),
            instrument: self.slot.value(),
            gain_db: c.gain_db() as f64,
            pan: c.pan() as f64,
            mute: c.mute(),
            keys: c.keys(),
            velocities: c.velocities(),
            transpose: c.transpose(),
        }
    }

    pub fn spec(&self) -> LayerSpec {
        LayerSpec {
            id: self.id,
            instrument: Some(self.slot.spec()),
            controls: self.controls.clone(),
        }
    }
}

/// Points the inserts of effects placed in a layer at the layer, by the
/// engine's ids. `inserts` are `slots`' specs, in order.
pub fn place(layers: &[Layer], slots: &[FxSlot], inserts: &mut [InsertSpec]) {
    for (slot, spec) in slots.iter().zip(inserts) {
        if let Some((key, _)) = &slot.place
            && let Some(l) = layers.iter().find(|l| &l.key == key)
        {
            spec.place = Some(Place {
                container: l.id,
                branch: 0,
            });
        }
    }
}

/// The layers as the effects window shows them: key and heading.
pub fn chains(layers: &[Layer]) -> Vec<(String, String)> {
    (layers.iter().enumerate())
        .map(|(n, l)| (l.key.clone(), format!("Layer {}: {}", n + 1, l.slot.name)))
        .collect()
}

/// The effects in the layer with this key, and in containers inside
/// them, by index.
fn effects_in(slots: &[FxSlot], key: &str) -> Vec<usize> {
    let mut keys: HashSet<String> = HashSet::from([key.to_string()]);
    let mut out: Vec<usize> = Vec::new();
    loop {
        let more: Vec<usize> = (0..slots.len())
            .filter(|i| !out.contains(i))
            .filter(|&i| {
                slots[i]
                    .place
                    .as_ref()
                    .is_some_and(|(k, _)| keys.contains(k))
            })
            .collect();
        if more.is_empty() {
            break;
        }
        for &i in &more {
            keys.insert(slots[i].key.clone());
        }
        out.extend(more);
    }
    out.sort_unstable();
    out
}

/// Takes away all of `t`'s layers, with their effects.
pub fn drop_layers(t: &mut crate::Track) {
    for l in std::mem::take(&mut t.layers) {
        for i in effects_in(&t.fx, &l.key).into_iter().rev() {
            t.fx.remove(i);
        }
    }
}

/// A change asked for in the instrument window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Another layer with the simple synth (the track's instrument
    /// becoming the first, if it isn't layered yet).
    Add,
    Remove(usize),
    /// Swap this layer with the one before it.
    Up(usize),
    /// Show the track's effects window, where the layers' effects are.
    Effects,
}

impl App {
    /// The instrument track layer commands act on: the one whose
    /// instrument window is open, else the one with the selected note
    /// clip, else the first.
    pub(crate) fn layer_track(&self) -> Option<usize> {
        let plays = |i: &usize| self.tracks.get(*i).is_some_and(|t| t.kind.plays_notes());
        self.instrument_open
            .filter(plays)
            .or_else(|| {
                let key = self.timeline.note_sel.as_deref()?;
                (self.tracks.iter()).position(|t| t.note_clips.iter().any(|c| c.id == key))
            })
            .filter(plays)
            .or_else(|| self.tracks.iter().position(|t| t.kind.plays_notes()))
    }

    /// Adds a layer with the simple synth to track `ti`. A track not yet
    /// layered keeps its instrument as the first layer, playing on. The
    /// new layer's index, if `ti` is an instrument track.
    pub(crate) fn add_layer(&mut self, ti: usize) -> Option<usize> {
        let t = self.tracks.get_mut(ti).filter(|t| t.kind.plays_notes())?;
        if t.layers.is_empty()
            && let Some(slot) = t.instrument.take()
        {
            t.layers.push(Layer::new(slot));
        }
        let layer = Layer::new(Slot::synth());
        layer
            .slot
            .make_into(self.rate, &mut self.pending_instruments);
        t.layers.push(layer);
        let n = t.layers.len() - 1;
        self.rebuild();
        Some(n)
    }

    /// Removes layer `n` of track `ti`, with its effects. True if there
    /// was one.
    pub(crate) fn remove_layer(&mut self, ti: usize, n: usize) -> bool {
        let Some(t) = self.tracks.get_mut(ti).filter(|t| n < t.layers.len()) else {
            return false;
        };
        let gone = t.layers.remove(n);
        for i in effects_in(&t.fx, &gone.key).into_iter().rev() {
            t.fx.remove(i);
        }
        self.rebuild();
        true
    }

    /// Moves layer `n` of track `ti` one place earlier. True if it moved.
    pub(crate) fn move_layer_up(&mut self, ti: usize, n: usize) -> bool {
        let Some(t) = self
            .tracks
            .get_mut(ti)
            .filter(|t| n > 0 && n < t.layers.len())
        else {
            return false;
        };
        t.layers.swap(n - 1, n);
        // The sound is the same; only the order in the project changes.
        true
    }

    /// Makes track `ti` play one instrument again: its first layer's,
    /// whose effects move to the start of the track's own chain. The
    /// other layers go, with their effects. True if it was layered.
    pub(crate) fn single_instrument(&mut self, ti: usize) -> bool {
        let Some(t) = self.tracks.get_mut(ti).filter(|t| !t.layers.is_empty()) else {
            return false;
        };
        let mut layers = std::mem::take(&mut t.layers).into_iter();
        let first = layers.next();
        for l in layers {
            for i in effects_in(&t.fx, &l.key).into_iter().rev() {
                t.fx.remove(i);
            }
        }
        if let Some(first) = first {
            let mut kept = Vec::new();
            let mut rest = Vec::new();
            for mut s in t.fx.drain(..) {
                if s.place.as_ref().is_some_and(|(k, _)| *k == first.key) {
                    s.place = None;
                    kept.push(s);
                } else {
                    rest.push(s);
                }
            }
            kept.extend(rest);
            t.fx = kept;
            t.instrument = Some(first.slot);
        }
        self.rebuild();
        true
    }

    /// Does what the instrument window asked for, on track `ti`.
    pub(crate) fn layer_edit(&mut self, ti: usize, edit: Edit) {
        match edit {
            Edit::Add => {
                self.add_layer(ti);
            }
            Edit::Remove(n) => {
                self.remove_layer(ti, n);
            }
            Edit::Up(n) => {
                self.move_layer_up(ti, n);
            }
            Edit::Effects => self.effects.open_track = Some(ti),
        }
    }
}

/// A note's name for a range, such as `C4` (60).
fn key_name(v: f64) -> String {
    daw_project::Note::pitch_name(v.clamp(0.0, 127.0) as u8)
}

/// Each layer's settings in the instrument window. Settings change on
/// the audio thread as they move; other edits come back as an [`Edit`].
/// A layer's instrument can be changed for another here, as a track's
/// can.
pub fn controls(
    ui: &mut egui::Ui,
    layers: &mut [Layer],
    cx: &mut Ctx,
    shown: &mut Shown,
) -> Option<Edit> {
    let mut edit = None;
    let count = layers.len();
    for (n, l) in layers.iter_mut().enumerate() {
        let c = l.controls.clone();
        let title = format!("Layer {}: {}", n + 1, l.slot.name);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&title).strong());
            let mut on = !c.mute();
            if ui
                .toggle_value(&mut on, "On")
                .on_hover_text("Switch off to silence this layer")
                .changed()
            {
                c.set_mute(!on);
            }
            if ui
                .add_enabled(n > 0, egui::Button::new("▲"))
                .on_hover_text("Move earlier in the list")
                .clicked()
            {
                edit = Some(Edit::Up(n));
            }
            if ui
                .add_enabled(n + 1 < count, egui::Button::new("▼"))
                .on_hover_text("Move later in the list")
                .clicked()
            {
                edit = Some(Edit::Up(n + 1));
            }
            if ui
                .button("✖")
                .on_hover_text("Remove this layer and its effects")
                .clicked()
            {
                edit = Some(Edit::Remove(n));
            }
        });
        egui::Grid::new(("layer", l.id))
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Instrument");
                let now = (!l.slot.is_plugin()).then_some(l.slot.kind());
                egui::ComboBox::from_id_salt(("layer-instrument", l.id))
                    .selected_text(l.slot.name.clone())
                    .show_ui(ui, |ui| {
                        for (kind, name) in crate::instrument::choices() {
                            if ui.selectable_label(now == Some(kind), name).clicked()
                                && now != Some(kind)
                            {
                                let s = kind.map_or_else(Slot::synth, Slot::new);
                                s.make_into(cx.rate, cx.pending);
                                l.slot = s;
                                shown.changed = true;
                            }
                        }
                        let slot = Some(&l.slot);
                        if let Some(s) = crate::instrument::plugin_choices(ui, slot, cx, shown) {
                            l.slot = s;
                            shown.changed = true;
                        }
                    })
                    .response
                    .accessible_name(format!("{title} instrument"));
                ui.end_row();
                ui.label("Level");
                let mut db = c.gain_db();
                if ui
                    .add(egui::Slider::new(&mut db, -60.0..=12.0).suffix(" dB"))
                    .accessible_name(format!("{title} level"))
                    .changed()
                {
                    c.set_gain_db(db);
                }
                ui.end_row();
                ui.label("Pan");
                let mut pan = c.pan();
                if ui
                    .add(egui::Slider::new(&mut pan, -1.0..=1.0))
                    .accessible_name(format!("{title} pan"))
                    .changed()
                {
                    c.set_pan(pan);
                }
                ui.end_row();
                ui.label("Notes");
                let (mut lo, mut hi) = c.keys();
                ui.horizontal(|ui| {
                    let a = ui
                        .add(
                            egui::DragValue::new(&mut lo)
                                .range(0..=127)
                                .custom_formatter(|v, _| key_name(v)),
                        )
                        .accessible_name(format!("{title} lowest note"));
                    ui.label("to");
                    let b = ui
                        .add(
                            egui::DragValue::new(&mut hi)
                                .range(0..=127)
                                .custom_formatter(|v, _| key_name(v)),
                        )
                        .accessible_name(format!("{title} highest note"));
                    if a.changed() || b.changed() {
                        c.set_keys(lo, hi);
                    }
                })
                .response
                .on_hover_text("The notes this layer plays, as written or played");
                ui.end_row();
                ui.label("Velocities");
                let (mut soft, mut loud) = c.velocities();
                ui.horizontal(|ui| {
                    let a = ui
                        .add(egui::DragValue::new(&mut soft).range(1..=127))
                        .accessible_name(format!("{title} softest"));
                    ui.label("to");
                    let b = ui
                        .add(egui::DragValue::new(&mut loud).range(1..=127))
                        .accessible_name(format!("{title} loudest"));
                    if a.changed() || b.changed() {
                        c.set_velocities(soft, loud);
                    }
                })
                .response
                .on_hover_text("How hard a note is played for this layer to play it");
                ui.end_row();
                ui.label("Transpose");
                let mut t = c.transpose();
                if ui
                    .add(
                        egui::DragValue::new(&mut t)
                            .range(-MAX_TRANSPOSE..=MAX_TRANSPOSE)
                            .suffix(" st"),
                    )
                    .accessible_name(format!("{title} transpose"))
                    .on_hover_text("Semitones this layer's notes move by: 12 is an octave up")
                    .changed()
                {
                    c.set_transpose(t);
                }
                ui.end_row();
            });
        egui::CollapsingHeader::new("Sound")
            .id_salt(("layer-sound", l.id))
            .show(ui, |ui| {
                // A layer's extra outputs (a drum machine's) all play
                // through the layer.
                crate::instrument::slot_controls(ui, &mut l.slot, &mut Vec::new(), cx, shown);
            });
        ui.separator();
    }
    ui.horizontal(|ui| {
        if ui
            .button("+ Add layer")
            .on_hover_text("Another instrument playing the same notes")
            .clicked()
        {
            edit = Some(Edit::Add);
        }
        if ui
            .button("Effects…")
            .on_hover_text("Each layer's own effects, before the track's")
            .clicked()
        {
            edit = Some(Edit::Effects);
        }
    });
    edit
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use daw_engine::engine::render_with;
    use daw_engine::time::Frames;
    use daw_project::{Note, NoteClip, PPQ, ProjectDoc, ProjectFolder};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::App;
    use crate::commands::Cmd;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-layers-{name}-{}", std::process::id()));
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

    /// The left side of the first `frames`, with fresh instruments and
    /// effects, as an export makes them.
    fn render(app: &mut App, frames: usize) -> Vec<f32> {
        let s = app.session();
        let mut keep = Vec::new();
        let mut fx = Vec::new();
        for slot in app.tracks.iter().flat_map(|t| &t.fx) {
            let (p, proc) = app.effects.fresh(slot, app.rate).unwrap();
            keep.extend(p);
            fx.push((slot.id, proc));
        }
        let mut instruments: Vec<_> = (app.tracks.iter().flat_map(|t| t.instruments()))
            .filter_map(|i| Some((i.id, i.make(app.rate)?)))
            .collect();
        let g = s
            .compile_all(daw_audio_io::MAX_BLOCK, &mut fx, &mut instruments)
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

    /// An instrument track with a low note, then a high one, each half
    /// a beat long, a beat apart.
    fn keys(h: &mut Harness<'_, App>) {
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddInstrumentTrack);
        let app = h.state_mut();
        let mut clip = NoteClip::new("c".into(), "Riff".into(), 0, PPQ * 4);
        clip.notes.push(Note::new(0, PPQ / 2, 48, 100));
        clip.notes.push(Note::new(PPQ, PPQ / 2, 72, 100));
        app.tracks[0].note_clips.push(clip);
        app.rebuild();
        h.run();
    }

    #[test]
    fn layers_are_added_by_command_split_the_keyboard_and_come_back_with_the_project() {
        let dir = scratch("split");
        let mut h = harness();
        keys(&mut h);
        let ctx = h.ctx.clone();
        let rate = h.state().rate as usize;
        let beat = rate / 2;
        let first = h.state().tracks[0].instrument.as_ref().unwrap().id;
        let both = render(h.state_mut(), 2 * beat);
        assert!(peak(&both[..beat]) > 0.01 && peak(&both[beat..]) > 0.01);

        // By command: the synth becomes the first layer, still playing,
        // and a second layer is added. The instrument window opens.
        h.state_mut().run(&ctx, Cmd::AddLayer);
        h.run();
        {
            let t = &h.state().tracks[0];
            assert!(t.instrument.is_none());
            assert_eq!(t.layers.len(), 2);
            assert_eq!(t.layers[0].slot.id, first);
            assert_eq!(h.state().instrument_open, Some(0));
        }
        assert!(h.query_by_label("Layer 2: Simple synth level").is_some());

        // A split at middle C: the low note in the first layer, the high
        // one in the second, which is silenced.
        {
            let t = &h.state().tracks[0];
            t.layers[0].controls.set_keys(0, 59);
            t.layers[1].controls.set_keys(60, 127);
            t.layers[1].controls.set_mute(true);
        }
        h.run();
        let split = render(h.state_mut(), 2 * beat);
        assert!(peak(&split[..beat]) > 0.01, "the low note plays");
        assert!(
            peak(&split[beat + rate / 10..]) < 1e-4,
            "the high one doesn't"
        );

        // An EQ in the second layer, from its own list in the effects
        // window.
        h.state_mut().effects.open_track = Some(0);
        h.run();
        h.get_by_label("Add an effect to Layer 2: Simple synth")
            .click();
        h.run();
        h.get_by_label("EQ").click();
        h.run();
        let layer = h.state().tracks[0].layers[1].key.clone();
        let id = h.state().tracks[0].layers[1].id;
        assert_eq!(h.state().tracks[0].fx[0].place, Some((layer.clone(), 0)));
        let s = h.state().session();
        assert_eq!(s.tracks[0].layers.len(), 2);
        assert_eq!(s.tracks[0].inserts[0].place.map(|p| p.container), Some(id));

        // Saved and reopened, as it was.
        let song = dir.join("Song");
        h.state_mut().save_in(&ctx, &song).unwrap();
        h.run();
        let (_, bytes) = ProjectFolder::open(&song).unwrap();
        let saved = ProjectDoc::from_bytes(&bytes, 1).unwrap().get();
        let t = &saved.tracks[0];
        assert_eq!(t.layers.len(), 2);
        assert_eq!((t.layers[0].keys, t.layers[1].keys), ((0, 59), (60, 127)));
        assert!(t.layers[1].mute);
        assert_eq!(t.effects[0].container.as_deref(), Some(layer.as_str()));
        drop(h);
        let mut h = harness();
        h.state_mut().open_project(song.clone());
        until(&mut h, |a| !a.proj.loading());
        h.run();
        let t = &h.state().tracks[0];
        assert_eq!(t.layers.len(), 2);
        assert_eq!(t.layers[1].controls.keys(), (60, 127));
        assert_eq!(t.fx[0].place, Some((layer, 0)));
        let again = render(h.state_mut(), 2 * beat);
        assert!((peak(&again) - peak(&split)).abs() < 1e-4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_layer_can_play_a_built_in_instrument_and_it_is_saved() {
        let mut h = harness();
        keys(&mut h);
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddLayer);
        h.run();
        let before = h.state().tracks[0].layers[1].slot.id;
        h.get_by_label("Layer 2: Simple synth instrument").click();
        h.run();
        h.get_by_label("Synth").click();
        h.run();
        let t = &h.state().tracks[0];
        assert_eq!(t.layers[1].slot.kind(), Some(daw_instruments::Kind::Synth));
        assert_ne!(t.layers[1].slot.id, before);
        let rate = h.state().rate as usize;
        let out = render(h.state_mut(), rate / 2);
        assert!(peak(&out) > 0.01);
        let saved = h.state().project();
        assert_eq!(saved.tracks[0].layers[1].instrument.builtin, "poly-synth");
    }

    #[test]
    fn a_change_to_a_layer_is_one_undo_step() {
        let mut h = harness();
        keys(&mut h);
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::AddLayer);
        h.run();
        h.state().tracks[0].layers[1].controls.set_transpose(12);
        h.run();
        h.state_mut().undo(&ctx);
        h.run();
        assert_eq!(h.state().tracks[0].layers[1].controls.transpose(), 0);
        h.state_mut().undo(&ctx);
        h.run();
        let t = &h.state().tracks[0];
        assert!(t.layers.is_empty() && t.instrument.is_some());
    }

    #[test]
    fn going_back_to_one_instrument_keeps_the_first_layer_and_its_effects() {
        let mut h = harness();
        keys(&mut h);
        let ctx = h.ctx.clone();
        h.state_mut().add_layer(0);
        h.state_mut().add_layer(0);
        let rate = h.state().rate;
        let (k0, k1, k2) = {
            let t = &h.state().tracks[0];
            (
                t.layers[0].key.clone(),
                t.layers[1].key.clone(),
                t.layers[2].key.clone(),
            )
        };
        // An effect in each layer, and one in the track's own chain.
        for place in [Some(k0.clone()), Some(k1), None, Some(k2.clone())] {
            let app = h.state_mut();
            let mut slot = app.effects.create_builtin(daw_effects::Kind::Eq, rate);
            slot.place = place.map(|k| (k, 0));
            app.tracks[0].fx.push(slot);
        }
        // Removing a layer takes its effects.
        assert!(h.state_mut().remove_layer(0, 2));
        assert_eq!(h.state().tracks[0].fx.len(), 3);
        let first = h.state().tracks[0].layers[0].slot.id;
        h.state_mut().run(&ctx, Cmd::SingleInstrument);
        h.run();
        let t = &h.state().tracks[0];
        assert!(t.layers.is_empty());
        assert_eq!(t.instrument.as_ref().map(|i| i.id), Some(first));
        // The first layer's effect leads the track's chain; the second
        // layer's is gone.
        assert_eq!(t.fx.len(), 2);
        assert!(t.fx.iter().all(|s| s.place.is_none()));
        let rendered = render(h.state_mut(), rate as usize / 2);
        assert!(peak(&rendered) > 0.01);
    }
}
