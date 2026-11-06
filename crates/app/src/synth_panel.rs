// SPDX-License-Identifier: GPL-3.0-or-later
//! The poly-synth's front panel (UI polish step 4): the first instrument
//! laid out on the panel kit, as the proving ground for the rest.
//!
//! Its modules run left to right in signal order: oscillators (with the
//! unison voices), the filter with its response drawn live and the big
//! cutoff knob, the two envelopes with their shapes, the LFO, and
//! playing and output. Every control is a setting of the synth, so an
//! assistant reaches each through `set_parameter`.

use daw_instruments::params::Params;
use daw_instruments::synth::at;
use eframe::egui::{RichText, Ui, vec2};

use crate::panel_kit::{
    FilterShape, Size, Switch, envelope_display, filter_display, module, param_knob, param_switch,
};
use crate::theme;

/// The panel's modules, `height` tall, across the width given (in more
/// than one row when the pane is narrower than they are). True if a setting
/// moved.
pub(crate) fn poly_synth(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    crate::panel_kit::modules(ui, 8.0, |ui| {
        changed |= oscillators(ui, p, salt, height);
        changed |= filter(ui, p, salt, height);
        changed |= envelopes(ui, p, salt, height);
        changed |= lfo(ui, p, salt, height);
        changed |= playing(ui, p, salt, height);
    });
    changed
}

fn dim(ui: &Ui, text: &str) -> RichText {
    let look = theme::look(ui.ctx());
    RichText::new(text)
        .size(look.small)
        .color(look.c().text_dim)
}

fn oscillators(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    let waves = Switch::width_of(ui, daw_instruments::dsp::Wave::NAMES) + 40.0;
    module(ui, "Oscillators", vec2(waves.max(290.0), height), |ui| {
        for (n, i) in [("1", at::OSC1), ("2", at::OSC2)] {
            ui.horizontal(|ui| {
                ui.label(dim(ui, n));
                changed |= param_switch(ui, ("poly", salt), p, i);
            });
        }
        ui.horizontal(|ui| {
            for (i, label) in [
                (at::OSC2_SEMIS, "Osc 2 pitch"),
                (at::OSC2_CENTS, "Osc 2 fine"),
                (at::OSC_MIX, "Mix 1 / 2"),
                (at::WIDTH, "Pulse width"),
                (at::SUB, "Sub"),
            ] {
                changed |= param_knob(ui, ("poly", salt), p, i, Some(label), Size::Small);
            }
        });
        ui.horizontal(|ui| {
            changed |= param_knob(ui, ("poly", salt), p, at::NOISE, Some("Noise"), Size::Small);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(dim(ui, "Unison voices"));
                changed |= param_switch(ui, ("poly", salt), p, at::UNISON);
            });
            changed |= param_knob(
                ui,
                ("poly", salt),
                p,
                at::DETUNE,
                Some("Detune"),
                Size::Small,
            );
            changed |= param_knob(
                ui,
                ("poly", salt),
                p,
                at::SPREAD,
                Some("Spread"),
                Size::Small,
            );
        });
    });
    changed
}

/// The response the filter choice draws.
fn shape(choice: f64) -> FilterShape {
    match choice as u32 {
        1 => FilterShape::Band,
        2 => FilterShape::High,
        3..=5 => FilterShape::Low { poles: 4 },
        _ => FilterShape::Low { poles: 2 },
    }
}

fn filter(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Filter", vec2(258.0, height), |ui| {
        changed |= param_switch(ui, ("poly", salt), p, at::FILTER);
        filter_display(
            ui,
            vec2(ui.available_width(), 44.0),
            p.get(at::CUTOFF),
            p.get(at::RESONANCE),
            shape(p.get(at::FILTER)),
        );
        ui.horizontal(|ui| {
            changed |= param_knob(
                ui,
                ("poly", salt),
                p,
                at::CUTOFF,
                Some("Cutoff"),
                Size::Large,
            );
            for (i, label) in [
                (at::RESONANCE, "Resonance"),
                (at::KEY_TRACK, "Keys"),
                (at::FILTER_ENV, "Envelope"),
            ] {
                changed |= param_knob(ui, ("poly", salt), p, i, Some(label), Size::Small);
            }
        });
    });
    changed
}

fn envelopes(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Envelopes", vec2(222.0, height), |ui| {
        for (name, a, d, s, r) in [
            (
                "Filter",
                at::F_ATTACK,
                at::F_DECAY,
                at::F_SUSTAIN,
                at::F_RELEASE,
            ),
            ("Level", at::ATTACK, at::DECAY, at::SUSTAIN, at::RELEASE),
        ] {
            envelope_display(
                ui,
                vec2(ui.available_width(), 26.0),
                Some(name),
                p.get(a),
                p.get(d),
                p.get(s),
                p.get(r),
            );
            ui.horizontal(|ui| {
                for (i, label) in [(a, "Attack"), (d, "Decay"), (s, "Sustain"), (r, "Release")] {
                    changed |= param_knob(ui, ("poly", salt), p, i, Some(label), Size::Small);
                }
            });
        }
    });
    changed
}

fn lfo(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    let shapes = Switch::width_of(ui, &["Sine", "Triangle", "Square", "Random"]) + 20.0;
    module(ui, "LFO", vec2(shapes.max(160.0), height), |ui| {
        changed |= param_switch(ui, ("poly", salt), p, at::LFO_WAVE);
        ui.horizontal(|ui| {
            for (i, label) in [(at::LFO_RATE, "Rate"), (at::LFO_PITCH, "To pitch")] {
                changed |= param_knob(ui, ("poly", salt), p, i, Some(label), Size::Small);
            }
        });
        ui.horizontal(|ui| {
            for (i, label) in [(at::LFO_CUTOFF, "To filter"), (at::LFO_LEVEL, "To level")] {
                changed |= param_knob(ui, ("poly", salt), p, i, Some(label), Size::Small);
            }
        });
    });
    changed
}

fn playing(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    let modes = Switch::width_of(ui, &["Chords", "One note, gliding"]) + 20.0;
    module(ui, "Playing", vec2(modes.max(186.0), height), |ui| {
        changed |= param_switch(ui, ("poly", salt), p, at::PLAY);
        ui.horizontal(|ui| {
            for (i, label) in [
                (at::GLIDE, "Glide"),
                (at::VELOCITY, "Velocity"),
                (at::BEND, "Bend"),
            ] {
                changed |= param_knob(ui, ("poly", salt), p, i, Some(label), Size::Small);
            }
        });
        ui.horizontal(|ui| {
            changed |= param_knob(
                ui,
                ("poly", salt),
                p,
                at::LEVEL,
                Some("Level"),
                Size::Normal,
            );
            for (i, label) in [(at::DRIVE, "Drive"), (at::VINTAGE, "Drift")] {
                changed |= param_knob(ui, ("poly", salt), p, i, Some(label), Size::Small);
            }
        });
    });
    changed
}
