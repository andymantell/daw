// SPDX-License-Identifier: GPL-3.0-or-later
//! The front panels of the one-voice synths (UI polish step 5): Model
//! Mono, Acid Three and Oh-One, laid out on the panel kit like the
//! poly-synth's.
//!
//! Each runs left to right in signal order: what makes the sound, how
//! it is mixed, the filter with its response drawn live and the big
//! cutoff knob, the envelopes with their shapes, then playing and
//! output. Every control is a setting of the synth, so an assistant
//! reaches each through `set_parameter`.

use daw_instruments::params::{Params, Unit};
use daw_instruments::{acid, model_mono, oh_one};
use eframe::egui::{self, RichText, Ui, vec2};

use crate::panel_kit::{
    FilterShape, Knob, Size, Switch, envelope_display, filter_display, module, param_knob,
    param_switch,
};
use crate::{spoken, theme};

/// Model Mono's height in the rack.
pub(crate) const MODEL_MONO_UNITS: u8 = 3;
/// Acid Three's: a small unit, like the box it is modelled on.
pub(crate) const ACID_THREE_UNITS: u8 = 2;
/// Oh-One's.
pub(crate) const OH_ONE_UNITS: u8 = 3;

/// Modules side by side, `height` tall, scrolling sideways when the pane
/// is narrower than they are.
fn row(ui: &mut Ui, id: (&str, u64), add: impl FnOnce(&mut Ui) -> bool) -> bool {
    let mut changed = false;
    egui::ScrollArea::horizontal()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                changed = add(ui);
            });
        });
    changed
}

fn dim(ui: &Ui, text: &str) -> RichText {
    let look = theme::look(ui.ctx());
    RichText::new(text)
        .size(look.small)
        .color(look.c().text_dim)
}

/// A switch for choice `i` of `p` with shorter names on its segments
/// than the setting's own (a screen reader still hears the setting's
/// name), with `caption` over it when given.
fn segments(
    ui: &mut Ui,
    salt: (&str, u64),
    p: &Params,
    i: usize,
    caption: Option<&str>,
    names: &[&str],
) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    let chosen = (p.get(i) - info.min).round().max(0.0) as usize;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        if let Some(c) = caption {
            ui.label(dim(ui, c));
        }
        // ai: set_parameter
        Switch::new(info.name.as_str(), names, chosen)
            .small()
            .show(ui, (salt, i))
            .map(|k| p.set(i, info.min + k as f64))
            .is_some()
    })
    .inner
}

/// A stepped knob for choice `i` of `p`, the way a rotary switch on the
/// hardware picks a shape or a footage, reading out `short`'s name for
/// the choice so it fits under the knob.
fn choice_knob(
    ui: &mut Ui,
    salt: (&str, u64),
    p: &Params,
    i: usize,
    label: &str,
    short: &[&str],
) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    debug_assert_eq!(info.unit, Unit::Choice);
    let mut v = p.get(i);
    let first = spoken::start(ui);
    let name = |v: f64| {
        let k = (v - info.min).round().max(0.0) as usize;
        short.get(k).map_or_else(|| info.text(v), |s| s.to_string())
    };
    // ai: set_parameter
    let r = Knob::new(&mut v, info.min, info.max, info.name.as_str())
        .stepped(true)
        .default(info.default)
        .label(label)
        .readout(name(p.get(i)))
        .size(Size::Small)
        .show(ui, (salt, i));
    spoken::hearable(
        ui,
        first,
        info.text(v),
        spoken::steady(v, info.min, info.max),
    );
    if r.changed() {
        p.set(i, v);
    }
    r.changed()
}

/// Small knobs in a row, each `(setting, label)`.
fn knobs(ui: &mut Ui, salt: (&str, u64), p: &Params, row: &[(usize, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for &(i, label) in row {
            changed |= param_knob(ui, salt, p, i, Some(label), Size::Small);
        }
    });
    changed
}

// ----------------------------------------------------------------------
// Model Mono

/// Model Mono's panel: three oscillators, the mixer, the ladder filter,
/// the two contours, modulation, and playing.
pub(crate) fn model_mono(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    use model_mono::at;
    let s = ("model-mono", salt);
    row(ui, s, |ui| {
        let mut changed = false;
        // The oscillators: shape and footage on rotary switches, as the
        // originals had them; oscillators 2 and 3 tuned against 1.
        let pitch = Switch::width_of(ui, &["Keys", "LFO"]);
        let third = (Size::Small.cell() + 8.0 + pitch).max(104.0);
        module(
            ui,
            "Oscillators",
            vec2(226.0 + third + 16.0, height),
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (n, wave, range, tune) in [
                        ("1", at::OSC1_WAVE, at::OSC1_RANGE, None),
                        ("2", at::OSC2_WAVE, at::OSC2_RANGE, Some(at::OSC2_TUNE)),
                        ("3", at::OSC3_WAVE, at::OSC3_RANGE, Some(at::OSC3_TUNE)),
                    ] {
                        ui.vertical(|ui| {
                            ui.set_width(if n == "3" { third } else { 104.0 });
                            ui.label(dim(ui, &format!("Oscillator {n}")));
                            ui.horizontal(|ui| {
                                changed |= choice_knob(ui, s, p, wave, "Shape", MONO_WAVES);
                                changed |= choice_knob(ui, s, p, range, "Range", MONO_RANGES);
                            });
                            ui.horizontal(|ui| {
                                if let Some(t) = tune {
                                    changed |= param_knob(ui, s, p, t, Some("Tune"), Size::Small);
                                }
                                if n == "3" {
                                    changed |= segments(
                                        ui,
                                        s,
                                        p,
                                        at::OSC3_KEYS,
                                        Some("Pitch"),
                                        &["Keys", "LFO"],
                                    );
                                }
                            });
                        });
                    }
                });
            },
        );
        module(ui, "Mixer", vec2(178.0, height), |ui| {
            changed |= knobs(
                ui,
                s,
                p,
                &[
                    (at::OSC1_LEVEL, "Osc 1"),
                    (at::OSC2_LEVEL, "Osc 2"),
                    (at::OSC3_LEVEL, "Osc 3"),
                ],
            );
            changed |= knobs(
                ui,
                s,
                p,
                &[(at::NOISE_LEVEL, "Noise"), (at::OVERDRIVE, "Overdrive")],
            );
            changed |= segments(
                ui,
                s,
                p,
                at::NOISE_COLOUR,
                Some("Noise"),
                &["White", "Pink"],
            );
        });
        module(ui, "Filter", vec2(250.0, height), |ui| {
            ui.horizontal(|ui| {
                ui.label(dim(ui, "Keys"));
                changed |= param_switch(ui, s, p, at::KEY_TRACK);
            });
            filter_display(
                ui,
                vec2(ui.available_width(), 44.0),
                p.get(at::CUTOFF),
                p.get(at::EMPHASIS),
                FilterShape::Low { poles: 4 },
            );
            ui.horizontal(|ui| {
                changed |= param_knob(ui, s, p, at::CUTOFF, Some("Cutoff"), Size::Large);
                changed |= knobs(
                    ui,
                    s,
                    p,
                    &[(at::EMPHASIS, "Emphasis"), (at::CONTOUR, "Contour")],
                );
            });
        });
        // The contours: attack, decay and sustain; the release switch
        // lets go at the decay time, or at once.
        let switch = Switch::width_of(ui, &["Short", "Decay"]);
        let contours = 3.0 * Size::Small.cell() + 3.0 * 6.0 + switch + 16.0;
        module(ui, "Contours", vec2(contours.max(244.0), height), |ui| {
            let release = |decay: usize| {
                if p.get(at::DECAY_RELEASE) >= 0.5 {
                    p.get(decay)
                } else {
                    0.004
                }
            };
            for (name, a, d, sus) in [
                ("Filter", at::F_ATTACK, at::F_DECAY, at::F_SUSTAIN),
                ("Loudness", at::ATTACK, at::DECAY, at::SUSTAIN),
            ] {
                envelope_display(
                    ui,
                    vec2(ui.available_width(), 24.0),
                    Some(name),
                    p.get(a),
                    p.get(d),
                    p.get(sus),
                    release(d),
                );
                ui.horizontal(|ui| {
                    changed |= knobs(ui, s, p, &[(a, "Attack"), (d, "Decay"), (sus, "Sustain")]);
                    if a == at::ATTACK {
                        changed |= segments(
                            ui,
                            s,
                            p,
                            at::DECAY_RELEASE,
                            Some("Release"),
                            &["Short", "Decay"],
                        );
                    }
                });
            }
        });
        module(ui, "Modulation", vec2(140.0, height), |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            changed |= knobs(
                ui,
                s,
                p,
                &[(at::MOD_MIX, "Osc 3/Noise"), (at::MOD_AMOUNT, "Amount")],
            );
            changed |= segments(ui, s, p, at::OSC_MOD, Some("To pitch"), &["Off", "On"]);
            changed |= segments(ui, s, p, at::FILTER_MOD, Some("To filter"), &["Off", "On"]);
        });
        module(ui, "Playing", vec2(206.0, height), |ui| {
            ui.horizontal(|ui| {
                changed |= segments(ui, s, p, at::PRIORITY, Some("Priority"), PRIORITIES);
                changed |= segments(ui, s, p, at::TRIGGER, Some("Trigger"), &["Legato", "Every"]);
            });
            changed |= knobs(
                ui,
                s,
                p,
                &[
                    (at::GLIDE, "Glide"),
                    (at::BEND, "Bend"),
                    (at::VELOCITY, "Velocity"),
                ],
            );
            ui.horizontal(|ui| {
                changed |= param_knob(ui, s, p, at::LEVEL, Some("Level"), Size::Normal);
                changed |= param_knob(ui, s, p, at::VINTAGE, Some("Drift"), Size::Small);
            });
        });
        changed
    })
}

/// Model Mono's shapes, short enough to read under a knob.
const MONO_WAVES: &[&str] = &["Tri", "Shark", "Saw", "Square", "Wide", "Narrow"];
/// Its footages.
const MONO_RANGES: &[&str] = &["Lo", "32'", "16'", "8'", "4'", "2'"];
/// Oh-One's LFO shapes, short.
const LFO_SHAPES: &[&str] = &["Tri", "Square", "Random", "Noise"];
/// Note priority, short.
const PRIORITIES: &[&str] = &["Low", "High", "Last"];

// ----------------------------------------------------------------------
// Acid Three

/// Acid Three's panel, two units tall: the oscillator, the filter and
/// its envelope, accent and slide, and character.
pub(crate) fn acid_three(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    use acid::at;
    let s = ("acid-three", salt);
    row(ui, s, |ui| {
        let mut changed = false;
        module(ui, "Oscillator", vec2(120.0, height), |ui| {
            changed |= param_switch(ui, s, p, at::WAVE);
            changed |= param_knob(ui, s, p, at::TUNE, Some("Tune"), Size::Small);
        });
        module(ui, "Filter", vec2(274.0, height), |ui| {
            ui.horizontal(|ui| {
                changed |= param_knob(ui, s, p, at::CUTOFF, Some("Cutoff"), Size::Large);
                ui.vertical(|ui| {
                    filter_display(
                        ui,
                        vec2(ui.available_width(), 24.0),
                        p.get(at::CUTOFF),
                        p.get(at::RESONANCE),
                        FilterShape::Low { poles: 3 },
                    );
                    changed |= knobs(
                        ui,
                        s,
                        p,
                        &[
                            (at::RESONANCE, "Resonance"),
                            (at::ENV_MOD, "Env amount"),
                            (at::DECAY, "Decay"),
                        ],
                    );
                });
            });
        });
        module(ui, "Accent and slide", vec2(178.0, height), |ui| {
            changed |= knobs(
                ui,
                s,
                p,
                &[
                    (at::ACCENT, "Accent"),
                    (at::ACCENT_VELOCITY, "Accent at"),
                    (at::SLIDE, "Slide"),
                ],
            );
        });
        module(ui, "Character", vec2(246.0, height), |ui| {
            ui.horizontal(|ui| {
                changed |= knobs(
                    ui,
                    s,
                    p,
                    &[
                        (at::DRIVE, "Drive"),
                        (at::VINTAGE, "Drift"),
                        (at::BEND, "Bend"),
                    ],
                );
                changed |= param_knob(ui, s, p, at::LEVEL, Some("Level"), Size::Normal);
            });
        });
        changed
    })
}

// ----------------------------------------------------------------------
// Oh-One

/// Oh-One's panel: the oscillator, the mixer with the sub, the filter,
/// the envelope and amplifier, the LFO, and playing.
pub(crate) fn oh_one(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    use oh_one::at;
    let s = ("oh-one", salt);
    row(ui, s, |ui| {
        let mut changed = false;
        module(ui, "Oscillator", vec2(184.0, height), |ui| {
            changed |= segments(
                ui,
                s,
                p,
                at::RANGE,
                Some("Range"),
                &["16'", "8'", "4'", "2'"],
            );
            changed |= knobs(ui, s, p, &[(at::LFO_PITCH, "LFO"), (at::WIDTH, "Width")]);
            ui.horizontal(|ui| {
                changed |= segments(
                    ui,
                    s,
                    p,
                    at::PWM_SOURCE,
                    Some("Width moved by"),
                    &["Hand", "LFO", "Env"],
                );
                changed |= param_knob(ui, s, p, at::PWM, Some("Movement"), Size::Small);
            });
        });
        module(ui, "Mixer", vec2(176.0, height), |ui| {
            changed |= knobs(ui, s, p, &[(at::SAW, "Saw"), (at::PULSE, "Pulse")]);
            changed |= knobs(ui, s, p, &[(at::SUB, "Sub"), (at::NOISE, "Noise")]);
            changed |= segments(
                ui,
                s,
                p,
                at::SUB_SHAPE,
                Some("Sub"),
                &["1 down", "2 down", "2 narrow"],
            );
        });
        module(ui, "Filter", vec2(214.0, height), |ui| {
            filter_display(
                ui,
                vec2(ui.available_width(), 44.0),
                p.get(at::CUTOFF),
                p.get(at::RESONANCE),
                FilterShape::Low { poles: 4 },
            );
            ui.horizontal(|ui| {
                changed |= param_knob(ui, s, p, at::CUTOFF, Some("Cutoff"), Size::Large);
                ui.vertical(|ui| {
                    changed |= knobs(
                        ui,
                        s,
                        p,
                        &[(at::RESONANCE, "Resonance"), (at::FILTER_ENV, "Env")],
                    );
                    changed |= knobs(
                        ui,
                        s,
                        p,
                        &[(at::FILTER_LFO, "LFO"), (at::KEY_TRACK, "Keys")],
                    );
                });
            });
        });
        module(ui, "Envelope", vec2(234.0, height), |ui| {
            ui.horizontal(|ui| {
                ui.label(dim(ui, "Amplifier follows"));
                changed |= param_switch(ui, s, p, at::AMP);
            });
            envelope_display(
                ui,
                vec2(ui.available_width(), 30.0),
                None,
                p.get(at::ATTACK),
                p.get(at::DECAY),
                p.get(at::SUSTAIN),
                p.get(at::RELEASE),
            );
            changed |= knobs(
                ui,
                s,
                p,
                &[
                    (at::ATTACK, "Attack"),
                    (at::DECAY, "Decay"),
                    (at::SUSTAIN, "Sustain"),
                    (at::RELEASE, "Release"),
                ],
            );
        });
        let shapes = Switch::width_of(ui, LFO_SHAPES) + 20.0;
        module(ui, "LFO", vec2(shapes.max(150.0), height), |ui| {
            changed |= segments(ui, s, p, at::LFO_WAVE, None, LFO_SHAPES);
            changed |= param_knob(ui, s, p, at::LFO_RATE, Some("Rate"), Size::Normal);
        });
        module(ui, "Playing", vec2(256.0, height), |ui| {
            ui.horizontal(|ui| {
                changed |= segments(
                    ui,
                    s,
                    p,
                    at::PORTAMENTO,
                    Some("Glide"),
                    &["Off", "On", "Overlap"],
                );
                changed |= segments(ui, s, p, at::LEGATO, Some("Legato"), &["Off", "On"]);
            });
            changed |= segments(ui, s, p, at::PRIORITY, Some("Priority"), PRIORITIES);
            ui.spacing_mut().item_spacing.x = 12.0;
            changed |= knobs(
                ui,
                s,
                p,
                &[
                    (at::GLIDE, "Time"),
                    (at::BEND, "Bend"),
                    (at::BEND_FILTER, "Bend filter"),
                    (at::VELOCITY, "Velocity"),
                ],
            );
        });
        module(ui, "Output", vec2(126.0, height), |ui| {
            changed |= param_knob(ui, s, p, at::LEVEL, Some("Level"), Size::Normal);
            changed |= knobs(ui, s, p, &[(at::DRIVE, "Drive"), (at::VINTAGE, "Drift")]);
        });
        changed
    })
}
