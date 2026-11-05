// SPDX-License-Identifier: GPL-3.0-or-later
//! The classic poly synths' front panels (UI polish step 5): the June,
//! the Oracle and the Eighty, each laid out on the panel kit in the
//! same grammar as the poly-synth (`synth_panel.rs`), with its own
//! modules from its own architecture.
//!
//! - **June**: one oscillator with its mixer, the high-pass and the
//!   low-pass filter, one envelope, the LFO and chorus, playing, output.
//! - **Oracle**: oscillators A and B, the mixer, poly-mod, the two
//!   filter boards, the two envelopes, the LFO, playing.
//! - **Eighty**: the two channels side by side (each its oscillator,
//!   filters and envelopes), mix and touch, the sub-oscillator, the
//!   ring modulator, playing.
//!
//! Every control is a setting of its synth, so an assistant reaches each
//! through `set_parameter`.

use daw_instruments::eighty::{self, ch};
use daw_instruments::june;
use daw_instruments::oracle;
use daw_instruments::params::Params;
use eframe::egui::{self, RichText, Ui, vec2};

use crate::panel_kit::{
    FilterShape, Size, Switch, envelope_display, filter_display, module, param_knob, param_switch,
};
use crate::theme;

/// Lays out `modules` left to right, scrolling sideways when the pane is
/// narrower than they are. True if a setting moved.
fn row(ui: &mut Ui, salt: (&str, u64), modules: impl FnOnce(&mut Ui) -> bool) -> bool {
    let mut changed = false;
    egui::ScrollArea::horizontal()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                changed = modules(ui);
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

/// A lit button for on/off setting `i`, captioned `label`: a wave a
/// mixer adds, a route switched in. True if it changed.
fn toggle(ui: &mut Ui, p: &Params, i: usize, label: &str) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    let look = theme::look(ui.ctx());
    let on = p.get(i) >= 0.5;
    // ai: set_parameter
    let r = ui
        .add(egui::Button::new(RichText::new(label).size(look.small)).selected(on))
        .on_hover_text(format!("{}: {}", info.name, if on { "on" } else { "off" }));
    let r = r.accessible_name(format!("{}: {label}", info.name));
    if r.clicked() {
        p.set(i, if on { 0.0 } else { 1.0 });
        return true;
    }
    false
}

/// Rows of knobs close together, so three fit a three-unit plate in
/// every theme.
fn tight(ui: &mut Ui) {
    ui.spacing_mut().item_spacing.y = 1.0;
}

/// The gap between knobs in a row: enough that two readouts in octaves
/// ("+0.80 oct") side by side don't run together.
const GAP: f32 = 10.0;

/// Knobs for `(setting, label)` in a row, all `size`.
fn knobs(ui: &mut Ui, salt: (&str, u64), p: &Params, size: Size, which: &[(usize, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for &(i, label) in which {
            changed |= param_knob(ui, salt, p, i, Some(label), size);
        }
    });
    changed
}

/// The width of `n` knobs of `size` in a row, with the module's margins.
fn across(n: usize, size: Size) -> f32 {
    n as f32 * size.cell() + n.saturating_sub(1) as f32 * GAP + 18.0
}

// ----------------------------------------------------------------------
// June

/// The June's panel, `height` tall. True if a setting moved.
pub(crate) fn june(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    use june::at;
    let s = ("june", salt);
    row(ui, s, |ui| {
        let mut changed = false;
        let range = Switch::width_of(ui, &["16' (an octave down)", "8'", "4' (an octave up)"]);
        module(
            ui,
            "Oscillator",
            vec2((range + 18.0).max(across(4, Size::Small)), height),
            |ui| {
                tight(ui);
                changed |= param_switch(ui, s, p, at::RANGE);
                ui.horizontal(|ui| {
                    ui.label(dim(ui, "Width moved by"));
                    changed |= param_switch(ui, s, p, at::PWM_SOURCE);
                });
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::WIDTH, "Width"),
                        (at::PWM, "Movement"),
                        (at::LFO_PITCH, "LFO"),
                    ],
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::SAW, "Saw"),
                        (at::PULSE, "Pulse"),
                        (at::SUB, "Sub"),
                        (at::NOISE, "Noise"),
                    ],
                );
            },
        );
        let highpass =
            Switch::width_of(ui, &["Bass lift", "Flat", "Cut at 250 Hz", "Cut at 600 Hz"]);
        let width = (highpass + 18.0).max(Size::Large.cell() + across(4, Size::Small) + GAP);
        module(ui, "Filter", vec2(width, height), |ui| {
            tight(ui);
            changed |= param_switch(ui, s, p, at::HIGHPASS);
            filter_display(
                ui,
                vec2(ui.available_width(), 40.0),
                p.get(at::CUTOFF),
                p.get(at::RESONANCE),
                FilterShape::Low { poles: 4 },
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                changed |= param_knob(ui, s, p, at::CUTOFF, Some("Cutoff"), Size::Large);
                for (i, label) in [
                    (at::RESONANCE, "Resonance"),
                    (at::FILTER_ENV, "Envelope"),
                    (at::FILTER_LFO, "LFO"),
                    (at::KEY_TRACK, "Keys"),
                ] {
                    changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
                }
            });
        });
        module(ui, "Envelope", vec2(across(4, Size::Small), height), |ui| {
            tight(ui);
            ui.horizontal(|ui| {
                ui.label(dim(ui, "Level follows"));
                changed |= param_switch(ui, s, p, at::AMP);
            });
            envelope_display(
                ui,
                vec2(ui.available_width(), 40.0),
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
                Size::Small,
                &[
                    (at::ATTACK, "Attack"),
                    (at::DECAY, "Decay"),
                    (at::SUSTAIN, "Sustain"),
                    (at::RELEASE, "Release"),
                ],
            );
        });
        let chorus = Switch::width_of(ui, daw_instruments::vintage::chorus::ChorusMode::NAMES);
        module(
            ui,
            "LFO and chorus",
            vec2((chorus + 18.0).max(across(2, Size::Small) + 30.0), height),
            |ui| {
                tight(ui);
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[(at::LFO_RATE, "Rate"), (at::LFO_DELAY, "Delay")],
                );
                ui.add_space(4.0);
                ui.label(dim(ui, "Chorus"));
                changed |= param_switch(ui, s, p, at::CHORUS);
            },
        );
        module(ui, "Playing", vec2(across(4, Size::Small), height), |ui| {
            tight(ui);
            ui.horizontal(|ui| {
                changed |= param_switch(ui, s, p, at::MODE);
                changed |= toggle(ui, p, at::PORTAMENTO, "Portamento");
            });
            changed |= knobs(
                ui,
                s,
                p,
                Size::Small,
                &[
                    (at::VOICES, "Voices"),
                    (at::UNISON_DETUNE, "Detune"),
                    (at::GLIDE, "Glide"),
                    (at::WHEEL, "Wheel"),
                ],
            );
            changed |= knobs(
                ui,
                s,
                p,
                Size::Small,
                &[
                    (at::BEND, "Bend"),
                    (at::BEND_FILTER, "Bend filter"),
                    (at::VELOCITY, "Velocity"),
                    (at::VELOCITY_FILTER, "Vel. filter"),
                ],
            );
        });
        module(ui, "Output", vec2(across(2, Size::Small), height), |ui| {
            tight(ui);
            changed |= knobs(ui, s, p, Size::Normal, &[(at::LEVEL, "Level")]);
            changed |= knobs(
                ui,
                s,
                p,
                Size::Small,
                &[(at::DRIVE, "Drive"), (at::VINTAGE, "Drift")],
            );
        });
        changed
    })
}

// ----------------------------------------------------------------------
// Oracle

/// The response board B draws for its output choice.
fn oracle_shape(p: &Params) -> FilterShape {
    use oracle::at;
    if p.get(at::BOARD) < 0.5 {
        return FilterShape::Low { poles: 4 };
    }
    match p.get(at::MODE) as u32 {
        1 => FilterShape::Notch,
        2 => FilterShape::High,
        3 => FilterShape::Band,
        _ => FilterShape::Low { poles: 2 },
    }
}

/// The Oracle's panel, `height` tall. True if a setting moved.
pub(crate) fn oracle(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    use oracle::at;
    let s = ("oracle", salt);
    row(ui, s, |ui| {
        let mut changed = false;
        module(ui, "Osc A", vec2(across(3, Size::Small), height), |ui| {
            tight(ui);
            ui.horizontal(|ui| {
                changed |= toggle(ui, p, at::A_SAW, "Saw");
                changed |= toggle(ui, p, at::A_TRI, "Triangle");
                changed |= toggle(ui, p, at::A_PULSE, "Pulse");
            });
            changed |= toggle(ui, p, at::A_SYNC, "Sync to B");
            changed |= knobs(ui, s, p, Size::Small, &[(at::A_WIDTH, "Width")]);
        });
        module(ui, "Osc B", vec2(across(3, Size::Small), height), |ui| {
            tight(ui);
            ui.horizontal(|ui| {
                changed |= toggle(ui, p, at::B_SAW, "Saw");
                changed |= toggle(ui, p, at::B_TRI, "Triangle");
                changed |= toggle(ui, p, at::B_PULSE, "Pulse");
            });
            ui.horizontal(|ui| {
                changed |= toggle(ui, p, at::B_LOW, "Low");
                changed |= toggle(ui, p, at::B_KEYS, "Keys");
            });
            changed |= knobs(
                ui,
                s,
                p,
                Size::Small,
                &[
                    (at::B_COARSE, "Frequency"),
                    (at::B_FINE, "Fine"),
                    (at::B_WIDTH, "Width"),
                ],
            );
        });
        module(
            ui,
            "Mixer",
            vec2(across(1, Size::Small) + 10.0, height),
            |ui| {
                tight(ui);
                for (i, label) in [
                    (at::A_LEVEL, "Osc A"),
                    (at::B_LEVEL, "Osc B"),
                    (at::NOISE_LEVEL, "Noise"),
                ] {
                    changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
                }
            },
        );
        module(ui, "Poly-mod", vec2(across(2, Size::Small), height), |ui| {
            tight(ui);
            changed |= knobs(
                ui,
                s,
                p,
                Size::Small,
                &[(at::PM_ENV, "Filter env"), (at::PM_OSC_B, "Osc B")],
            );
            ui.label(dim(ui, "To"));
            ui.horizontal(|ui| {
                changed |= toggle(ui, p, at::PM_FREQ, "Pitch A");
                changed |= toggle(ui, p, at::PM_WIDTH, "Width A");
            });
            changed |= toggle(ui, p, at::PM_FILTER, "Filter");
        });
        let board = Switch::width_of(ui, &["A: 4-pole cascade", "B: 2-pole state-variable"]);
        let width = (board + 18.0).max(Size::Large.cell() + across(2, Size::Small) + GAP);
        module(ui, "Filter", vec2(width, height), |ui| {
            tight(ui);
            changed |= param_switch(ui, s, p, at::BOARD);
            ui.horizontal(|ui| {
                ui.label(dim(ui, "Board B"));
                changed |= param_switch(ui, s, p, at::MODE);
            });
            filter_display(
                ui,
                vec2(ui.available_width(), 36.0),
                p.get(at::CUTOFF),
                p.get(at::RESONANCE),
                oracle_shape(p),
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                changed |= param_knob(ui, s, p, at::CUTOFF, Some("Cutoff"), Size::Large);
                ui.vertical(|ui| {
                    changed |= knobs(
                        ui,
                        s,
                        p,
                        Size::Small,
                        &[(at::RESONANCE, "Resonance"), (at::ENV_AMOUNT, "Envelope")],
                    );
                    ui.horizontal(|ui| {
                        ui.label(dim(ui, "Keys"));
                        changed |= param_switch(ui, s, p, at::KEY_TRACK);
                    });
                });
            });
        });
        module(
            ui,
            "Envelopes",
            vec2(across(5, Size::Small), height),
            |ui| {
                tight(ui);
                for (name, a, d, su, r, vel) in [
                    (
                        "Filter",
                        at::F_ATTACK,
                        at::F_DECAY,
                        at::F_SUSTAIN,
                        at::F_RELEASE,
                        at::VEL_FILTER,
                    ),
                    (
                        "Level",
                        at::ATTACK,
                        at::DECAY,
                        at::SUSTAIN,
                        at::RELEASE,
                        at::VEL_AMP,
                    ),
                ] {
                    envelope_display(
                        ui,
                        vec2(ui.available_width(), 24.0),
                        Some(name),
                        p.get(a),
                        p.get(d),
                        p.get(su),
                        p.get(r),
                    );
                    changed |= knobs(
                        ui,
                        s,
                        p,
                        Size::Small,
                        &[
                            (a, "Attack"),
                            (d, "Decay"),
                            (su, "Sustain"),
                            (r, "Release"),
                            (vel, "Velocity"),
                        ],
                    );
                }
            },
        );
        module(
            ui,
            "LFO",
            vec2(across(3, Size::Small) + 20.0, height),
            |ui| {
                tight(ui);
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[(at::LFO_RATE, "Rate"), (at::LFO_INITIAL, "Amount")],
                );
                ui.horizontal(|ui| {
                    changed |= toggle(ui, p, at::LFO_SAW, "Saw");
                    changed |= toggle(ui, p, at::LFO_TRI, "Triangle");
                    changed |= toggle(ui, p, at::LFO_SQUARE, "Square");
                });
                ui.label(dim(ui, "To"));
                ui.horizontal(|ui| {
                    changed |= toggle(ui, p, at::LFO_FREQ_A, "Pitch A");
                    changed |= toggle(ui, p, at::LFO_FREQ_B, "Pitch B");
                    changed |= toggle(ui, p, at::LFO_FILTER, "Filter");
                });
                ui.horizontal(|ui| {
                    changed |= toggle(ui, p, at::LFO_WIDTH_A, "Width A");
                    changed |= toggle(ui, p, at::LFO_WIDTH_B, "Width B");
                });
            },
        );
        module(ui, "Playing", vec2(across(4, Size::Small), height), |ui| {
            tight(ui);
            changed |= knobs(
                ui,
                s,
                p,
                Size::Small,
                &[
                    (at::VOICES, "Voices"),
                    (at::UNISON, "Unison"),
                    (at::DETUNE, "Detune"),
                    (at::SPREAD, "Spread"),
                ],
            );
            changed |= knobs(
                ui,
                s,
                p,
                Size::Small,
                &[
                    (at::GLIDE, "Glide"),
                    (at::BEND, "Bend"),
                    (at::LEVEL, "Level"),
                    (at::VINTAGE, "Drift"),
                ],
            );
            changed |= toggle(ui, p, at::LEGATO, "Legato");
        });
        changed
    })
}

// ----------------------------------------------------------------------
// Eighty

/// One of the Eighty's two channels in three rows: its oscillator, its
/// two filters with the low-pass slope and the filter envelope, and its
/// level envelope.
fn eighty_channel(ui: &mut Ui, p: &Params, salt: u64, c: usize, height: f32) -> bool {
    let s = ("eighty", salt);
    let at = |k: usize| eighty::ch_at(c, k);
    let mut changed = false;
    let heading = if c == 0 { "Channel 1" } else { "Channel 2" };
    module(ui, heading, vec2(across(8, Size::Small), height), |ui| {
        tight(ui);
        ui.spacing_mut().item_spacing.x = GAP;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(Size::Small.cell());
                changed |= toggle(ui, p, at(ch::SAW), "Saw");
                changed |= toggle(ui, p, at(ch::SQUARE), "Square");
            });
            for (i, label) in [
                (at(ch::WIDTH), "Width"),
                (at(ch::PWM), "Sweep"),
                (at(ch::PWM_SPEED), "Speed"),
                (at(ch::SINE), "Sine"),
                (at(ch::NOISE), "Noise"),
                (at(ch::TUNE), "Tune"),
                (at(ch::LEVEL), "Level"),
            ] {
                changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
            }
        });
        ui.horizontal(|ui| {
            for (i, label) in [
                (at(ch::HP), "High-pass"),
                (at(ch::HP_RES), "Resonance"),
                (at(ch::LP), "Low-pass"),
                (at(ch::LP_RES), "Resonance"),
            ] {
                changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
            }
            ui.vertical(|ui| {
                ui.add_space(6.0);
                ui.label(dim(ui, "Slope"));
                changed |= param_switch(ui, s, p, at(ch::SLOPE));
            });
            for (i, label) in [
                (at(ch::F_INITIAL), "Initial"),
                (at(ch::F_ATTACK_LEVEL), "Peak"),
            ] {
                changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
            }
        });
        changed |= knobs(
            ui,
            s,
            p,
            Size::Small,
            &[
                (at(ch::F_ATTACK), "F attack"),
                (at(ch::F_DECAY), "F decay"),
                (at(ch::F_RELEASE), "F release"),
                (at(ch::ATTACK), "Attack"),
                (at(ch::DECAY), "Decay"),
                (at(ch::SUSTAIN), "Sustain"),
                (at(ch::RELEASE), "Release"),
            ],
        );
    });
    changed
}

/// The Eighty's panel, `height` tall. True if a setting moved.
pub(crate) fn eighty(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    use eighty::at;
    let s = ("eighty", salt);
    row(ui, s, |ui| {
        let mut changed = false;
        changed |= eighty_channel(ui, p, salt, 0, height);
        changed |= eighty_channel(ui, p, salt, 1, height);
        module(
            ui,
            "Mix and touch",
            vec2(across(3, Size::Small), height),
            |ui| {
                tight(ui);
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::MIX, "Mix 1 / 2"),
                        (at::BRILLIANCE, "Brilliance"),
                        (at::KEY_TRACK, "Keys"),
                    ],
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::TOUCH_LEVEL, "Vel. level"),
                        (at::TOUCH_CUTOFF, "Vel. bright"),
                    ],
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::AFTER_LEVEL, "Press level"),
                        (at::AFTER_CUTOFF, "Press bright"),
                        (at::AFTER_VIBRATO, "Press vib."),
                    ],
                );
            },
        );
        let shapes = Switch::width_of(ui, &["Sine", "Saw up", "Saw down", "Square", "Random"]);
        module(
            ui,
            "Modulation",
            vec2((shapes + 18.0).max(across(5, Size::Small)), height),
            |ui| {
                tight(ui);
                ui.horizontal(|ui| {
                    ui.label(dim(ui, "Sub-oscillator"));
                    changed |= param_switch(ui, s, p, at::LFO_SHAPE);
                });
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::LFO_SPEED, "Speed"),
                        (at::LFO_PITCH, "Vibrato"),
                        (at::LFO_FILTER, "To filter"),
                        (at::LFO_LEVEL, "Tremolo"),
                    ],
                );
                ui.label(dim(ui, "Ring modulator"));
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::RING_DEPTH, "Depth"),
                        (at::RING_SPEED, "Speed"),
                        (at::RING_SWEEP, "Sweep"),
                        (at::RING_ATTACK, "Attack"),
                        (at::RING_DECAY, "Decay"),
                    ],
                );
            },
        );
        let chorus = Switch::width_of(ui, daw_instruments::vintage::chorus::ChorusMode::NAMES);
        module(
            ui,
            "Playing",
            vec2((chorus + 18.0).max(across(3, Size::Small)), height),
            |ui| {
                tight(ui);
                ui.label(dim(ui, "Chorus"));
                changed |= param_switch(ui, s, p, at::CHORUS);
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[
                        (at::VOICES, "Voices"),
                        (at::GLIDE, "Glide"),
                        (at::BEND, "Ribbon"),
                    ],
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    Size::Small,
                    &[(at::LEVEL, "Level"), (at::VINTAGE, "Drift")],
                );
            },
        );
        changed
    })
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    fn value(d: &mut Driver, t: &str, param: &str) -> f64 {
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == format!("instrument.{param}"))
            .and_then(|p| p["value"].as_f64())
            .unwrap_or_else(|| panic!("no {param} in {p}"))
    }

    fn open(kind: &str) -> (Driver, String) {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": kind, "name": "Keys"}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();
        (d, t)
    }

    /// Each panel's controls, clicked, set the setting an assistant reads;
    /// a setting an assistant sets shows on the panel; and the header's
    /// sound buttons step through the synth's sounds.
    fn drive(kind: &str, labels: &[&str], click: &str, param: &str, want: f64, knob: (&str, f64)) {
        let (mut d, t) = open(kind);
        for l in labels {
            d.h.get_by_label(l);
        }
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["rack"][0]["view"], "shown", "{info}");

        d.h.get_by_label(click).click();
        d.h.run();
        assert_eq!(value(&mut d, &t, param), want, "{kind}: {click}");

        // An assistant's change reaches the setting the knob shows.
        let (key, v) = knob;
        d.call(
            "set_parameter",
            json!({"track": t, "param": format!("instrument.{key}"), "value": v}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(value(&mut d, &t, key), v);

        d.h.get_by_label("Next sound").click();
        d.h.run();
        let sound: Value = d.call("get_instrument", json!({"track": t})).unwrap()["sound"].clone();
        assert!(sound.is_string(), "{kind}: a sound after Next: {sound}");
        d.command("next-sound");
        d.h.run();
        let next = d.call("get_instrument", json!({"track": t})).unwrap()["sound"].clone();
        assert!(
            next.is_string() && next != sound,
            "{kind}: {sound} then {next}"
        );
    }

    #[test]
    fn the_june_panel_is_steered_by_click_and_by_assistant() {
        drive(
            "june",
            &[
                "Cutoff",
                "Range: 8'",
                "High-pass: Flat",
                "Chorus: I + II",
                "Voices",
            ],
            "Chorus: II",
            "chorus",
            2.0,
            ("cutoff", 640.0),
        );
    }

    #[test]
    fn the_oracle_panel_is_steered_by_click_and_by_assistant() {
        drive(
            "oracle",
            &[
                "Cutoff",
                "Osc A saw: Saw",
                "Osc B low frequency: Low",
                "Poly-mod to filter: Filter",
                "LFO to osc B width: Width B",
            ],
            "Osc A synced to B: Sync to B",
            "a_sync",
            1.0,
            ("resonance", 0.5),
        );
    }

    #[test]
    fn the_eighty_panel_is_steered_by_click_and_by_assistant() {
        drive(
            "eighty",
            &[
                "Ch 1 low-pass",
                "Ch 2 low-pass",
                "Ch 2 saw: Saw",
                "Ring modulator depth",
                "Sub-oscillator shape: Random",
            ],
            "Ch 2 saw: Saw",
            "ch2_saw",
            1.0,
            ("ch1_lp", 2000.0),
        );
    }
}
