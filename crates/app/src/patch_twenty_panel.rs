// SPDX-License-Identifier: GPL-3.0-or-later
//! Patch Twenty's front panel (UI polish step 5): the semi-modular
//! monosynth on the panel kit.
//!
//! Its modules run left to right in signal order: the two oscillators,
//! the mod generator, the two filters in series with their joint
//! response drawn live, the two envelopes, the amplifier with the
//! external input, the patch points (its four cords, drawn as a small
//! diagram of jacks), and playing. Every control is a setting of the
//! synth, so an assistant reaches each through `set_parameter`.

use daw_instruments::params::Params;
use daw_instruments::patch_twenty::{CORDS, at, cord_at};
use eframe::egui::{self, Pos2, RichText, Stroke, Ui, pos2, vec2};

use crate::panel_kit::{
    Size, envelope_display, module, param_combo, param_knob, param_switch, param_switch_as, screen,
    trace,
};
use crate::theme;

/// The panel's modules, `height` tall, scrolling sideways when the pane
/// is narrower than they are. True if a setting moved.
pub(crate) fn patch_twenty(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    egui::ScrollArea::horizontal()
        .id_salt(("patch-twenty", salt))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let s = ("patch-twenty", salt);
                changed |= oscillators(ui, p, s, height);
                changed |= mod_generator(ui, p, s, height);
                changed |= filters(ui, p, s, height);
                changed |= envelopes(ui, p, s, height);
                changed |= amplifier(ui, p, s, height);
                changed |= patch_points(ui, p, s, height);
                changed |= playing(ui, p, s, height);
            });
        });
    changed
}

type Salt = (&'static str, u64);

fn dim(ui: &Ui, text: &str) -> RichText {
    let look = theme::look(ui.ctx());
    RichText::new(text)
        .size(look.small)
        .color(look.c().text_dim)
}

fn knobs(ui: &mut Ui, s: Salt, p: &Params, row: &[(usize, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for &(i, label) in row {
            changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
        }
    });
    changed
}

fn oscillators(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Oscillators", vec2(288.0, height), |ui| {
        ui.horizontal(|ui| {
            ui.label(dim(ui, "1"));
            changed |= param_switch_as(ui, s, p, at::OSC1, &["Tri", "Saw", "Pulse", "Noise"]);
            changed |= param_switch(ui, s, p, at::OSC1_OCTAVE);
        });
        ui.horizontal(|ui| {
            ui.label(dim(ui, "2"));
            changed |= param_switch_as(ui, s, p, at::OSC2, &["Saw", "Sq", "Narrow", "Ring"]);
            changed |= param_switch(ui, s, p, at::OSC2_OCTAVE);
        });
        changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::OSC1_LEVEL, "Level 1"),
                (at::WIDTH, "Width"),
                (at::OSC2_LEVEL, "Level 2"),
                (at::OSC2_PITCH, "Pitch 2"),
            ],
        );
        changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::PITCH_MG, "Mod gen"),
                (at::PITCH_EG1, "Env 1"),
                (at::GLIDE, "Glide"),
            ],
        );
    });
    changed
}

fn mod_generator(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Mod gen", vec2(132.0, height), |ui| {
        mod_shape_display(ui, vec2(ui.available_width(), 34.0), p.get(at::MG_SHAPE));
        ui.horizontal(|ui| {
            changed |= param_knob(ui, s, p, at::MG_SHAPE, Some("Shape"), Size::Small);
            changed |= param_knob(ui, s, p, at::MG_RATE, Some("Rate"), Size::Small);
        });
        ui.label(dim(ui, "Patch it below\nto move what you like"));
    });
    changed
}

/// One cycle of the mod generator: its rise takes `shape` of the cycle
/// (a falling saw at 0, a triangle at a half, a rising saw at 1), with
/// the square high for the same part.
fn mod_shape_display(ui: &mut Ui, size: egui::Vec2, shape: f64) {
    let (rect, look) = screen(ui, size, 4, 2);
    let inner = rect.shrink(4.0);
    let w = shape.clamp(0.02, 0.98) as f32;
    let x = |t: f32| inner.left() + inner.width() * t;
    let y = |v: f32| inner.center().y - inner.height() * 0.5 * v;
    // Two cycles, so the shape reads.
    let mut points = Vec::new();
    for cycle in 0..2 {
        let at = cycle as f32 * 0.5;
        points.push(pos2(x(at), y(-1.0)));
        points.push(pos2(x(at + 0.5 * w), y(1.0)));
    }
    points.push(pos2(x(1.0), y(-1.0)));
    let square = Stroke::new(1.0, theme::mix(look.c().accent, look.c().meter_bg, 0.55));
    let p = ui.painter();
    for cycle in 0..2 {
        let at = cycle as f32 * 0.5;
        p.line_segment([pos2(x(at), y(0.8)), pos2(x(at + 0.5 * w), y(0.8))], square);
    }
    trace(ui, rect, points, &look);
}

/// The two-pole response at `w` (the frequency over the cutoff) with
/// damping `k`; high-pass when `high`.
fn two_pole(w: f64, k: f64, high: bool) -> f64 {
    let g = 1.0 / ((1.0 - w * w).powi(2) + (k * w).powi(2)).sqrt();
    if high { g * w * w } else { g }
}

/// The two filters in series: the high-pass then the low-pass, 20 Hz
/// to 20 kHz across, −36 to +18 dB up.
fn filters_display(ui: &mut Ui, size: egui::Vec2, p: &Params) {
    let (rect, look) = screen(ui, size, 6, 3);
    let inner = rect.shrink(2.0);
    let damping = |peak: f64| 2.0 - 1.9 * peak.clamp(0.0, 1.0);
    let (hp, hk) = (p.get(at::HP_CUTOFF), damping(p.get(at::HP_PEAK)));
    let (lp, lk) = (p.get(at::LP_CUTOFF), damping(p.get(at::LP_PEAK)));
    let n = 72;
    let points: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let f = 20.0 * 1000f64.powf(t);
            let gain = two_pole(f / hp.max(10.0), hk, true) * two_pole(f / lp.max(10.0), lk, false);
            let db = (20.0 * gain.max(1e-6).log10()).clamp(-36.0, 18.0);
            let y = inner.bottom() - inner.height() * ((db + 36.0) / 54.0) as f32;
            pos2(inner.left() + inner.width() * t as f32, y)
        })
        .collect();
    trace(ui, rect, points, &look);
}

fn filters(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Filters", vec2(336.0, height), |ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        filters_display(ui, vec2(ui.available_width(), 34.0), p);
        ui.horizontal(|ui| {
            changed |= param_knob(ui, s, p, at::DRIVE, Some("Drive"), Size::Small);
            changed |= param_knob(ui, s, p, at::HP_CUTOFF, Some("High-pass"), Size::Normal);
            for (i, label) in [
                (at::HP_PEAK, "Peak"),
                (at::HP_MG, "Mod gen"),
                (at::HP_EG2, "Env 2"),
            ] {
                changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(Size::Small.cell() + 12.0);
            changed |= param_knob(ui, s, p, at::LP_CUTOFF, Some("Low-pass"), Size::Normal);
            for (i, label) in [
                (at::LP_PEAK, "Peak"),
                (at::LP_MG, "Mod gen"),
                (at::LP_EG2, "Env 2"),
            ] {
                changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
            }
        });
    });
    changed
}

/// Envelope 1's shape: a wait, a rise and, once the key is let go, a
/// fall; drawn as the kit's envelope shapes are.
fn delay_attack_release(ui: &mut Ui, size: egui::Vec2, delay: f64, attack: f64, release: f64) {
    let (rect, look) = screen(ui, size, 4, 2);
    let inner = rect.shrink(3.0);
    let share = |t: f64| ((t.max(0.001) / 0.001).ln() / (8.0f64 / 0.001).ln()).clamp(0.0, 1.0);
    let wait = if delay <= 0.0 {
        0.0
    } else {
        share(delay) * 0.8 + 0.1
    };
    let (a, r) = (share(attack) * 0.8 + 0.2, share(release) * 0.8 + 0.2);
    let hold = 0.6;
    let total = wait + a + hold + r;
    let x = |t: f64| inner.left() + inner.width() * (t / total) as f32;
    let points = vec![
        pos2(inner.left(), inner.bottom()),
        pos2(x(wait), inner.bottom()),
        pos2(x(wait + a), inner.top()),
        pos2(x(wait + a + hold), inner.top()),
        pos2(inner.right(), inner.bottom()),
    ];
    trace(ui, rect, points, &look);
    ui.painter().text(
        rect.right_top() + vec2(-4.0, 2.0),
        egui::Align2::RIGHT_TOP,
        "Env 1",
        look.font(look.small),
        look.c().text_dim,
    );
}

fn envelopes(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Envelopes", vec2(282.0, height), |ui| {
        let w = ui.available_width();
        delay_attack_release(
            ui,
            vec2(w, 26.0),
            p.get(at::EG1_DELAY),
            p.get(at::EG1_ATTACK),
            p.get(at::EG1_RELEASE),
        );
        changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::EG1_DELAY, "Delay"),
                (at::EG1_ATTACK, "Attack"),
                (at::EG1_RELEASE, "Release"),
            ],
        );
        envelope_display(
            ui,
            vec2(w, 26.0),
            Some("Env 2"),
            p.get(at::EG2_ATTACK),
            p.get(at::EG2_DECAY),
            p.get(at::EG2_SUSTAIN),
            p.get(at::EG2_RELEASE),
        );
        changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::EG2_HOLD, "Hold"),
                (at::EG2_ATTACK, "Attack"),
                (at::EG2_DECAY, "Decay"),
                (at::EG2_SUSTAIN, "Sustain"),
                (at::EG2_RELEASE, "Release"),
            ],
        );
    });
    changed
}

fn amplifier(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Amplifier and input", vec2(236.0, height), |ui| {
        ui.horizontal(|ui| {
            ui.label(dim(ui, "Follows"));
            changed |= param_switch_as(ui, s, p, at::VCA_SOURCE, &["Env 2", "Env 1", "Gate"]);
        });
        changed |= knobs(ui, s, p, &[(at::DRONE, "Drone"), (at::LEVEL, "Level")]);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(dim(ui, "Input")).on_hover_text(
                    "Feeds the track's audio input through the filters and amplifier: \
                         pick the input on the mixer strip and turn on Monitor.",
                );
                changed |= param_switch(ui, s, p, at::EXT_IN);
            });
            changed |= param_knob(ui, s, p, at::EXT_GAIN, Some("Gain"), Size::Small);
            ui.vertical(|ui| {
                ui.label(dim(ui, "Triggers"))
                    .on_hover_text("Starts the envelopes when the input gets loud");
                changed |= param_switch(ui, s, p, at::EXT_TRIGGER);
            });
            changed |= param_knob(ui, s, p, at::EXT_THRESHOLD, Some("Threshold"), Size::Small);
        });
    });
    changed
}

/// Short names for the jacks a cord can come from (after "Off") and go
/// to (after "Nothing"), in the order of the synth's lists.
const FROM_JACKS: &[&str] = &[
    "Tri", "Sq", "Env1", "Env2", "S&H", "Noise", "Key", "Vel", "Ext",
];
const TO_JACKS: &[&str] = &[
    "Pit1", "Pit2", "Pit", "PW", "HP", "LP", "HP+LP", "Amp", "Rate",
];

/// The patch points: a row of jacks the cords come from over a row they
/// go to, with a line for each cord in use, brighter the more it
/// carries and dashed when it turns things down. A diagram, not
/// drawn cables.
fn patch_display(ui: &mut Ui, size: egui::Vec2, p: &Params) {
    let (rect, look) = screen(ui, size, 1, 1);
    let c = look.c();
    let font = look.font(look.small * 0.9);
    let inner = rect.shrink2(vec2(6.0, 3.0));
    let jack =
        |row: &[&str], k: usize| inner.left() + inner.width() * (k as f32 + 0.5) / row.len() as f32;
    let top = inner.top() + 14.0;
    let bottom = inner.bottom() - 14.0;
    let painter = ui.painter();
    for (row, y, label_y, align) in [
        (FROM_JACKS, top, inner.top(), egui::Align2::CENTER_TOP),
        (
            TO_JACKS,
            bottom,
            inner.bottom(),
            egui::Align2::CENTER_BOTTOM,
        ),
    ] {
        for (k, name) in row.iter().enumerate() {
            let x = jack(row, k);
            painter.circle(pos2(x, y), 3.0, c.meter_bg, Stroke::new(1.0, c.text_dim));
            painter.text(pos2(x, label_y), align, *name, font.clone(), c.text_dim);
        }
    }
    for cord in 0..CORDS {
        let from = p.get(cord_at(cord, 0)) as usize;
        let to = p.get(cord_at(cord, 1)) as usize;
        let amount = p.get(cord_at(cord, 2)) as f32;
        if from == 0 || to == 0 {
            continue;
        }
        let a = pos2(jack(FROM_JACKS, from - 1), top);
        let b = pos2(jack(TO_JACKS, to - 1), bottom);
        let colour = theme::mix(c.accent, c.meter_bg, 0.75 - 0.75 * amount.abs().min(1.0));
        let stroke = Stroke::new(2.0, colour);
        if amount < 0.0 {
            painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 4.0, 3.0));
        } else {
            painter.line_segment([a, b], stroke);
        }
        painter.circle_filled(a, 2.5, c.accent);
        painter.circle_filled(b, 2.5, c.accent);
        let mid = a + (b - a) * 0.5;
        painter.text(
            mid + vec2(4.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("{}", cord + 1),
            font.clone(),
            c.text,
        );
    }
}

fn patch_points(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Patch points", vec2(318.0, height), |ui| {
        patch_display(ui, vec2(ui.available_width(), 58.0), p);
        for pair in [[0, 1], [2, 3]] {
            ui.horizontal(|ui| {
                for cord in pair {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(dim(ui, &format!("{}", cord + 1)));
                            changed |= param_combo(ui, s, p, cord_at(cord, 0), 82.0);
                        });
                        ui.horizontal(|ui| {
                            ui.label(dim(ui, "to"));
                            changed |= param_combo(ui, s, p, cord_at(cord, 1), 82.0);
                        });
                    });
                    changed |= param_knob(ui, s, p, cord_at(cord, 2), Some("Amount"), Size::Small);
                }
            });
        }
    });
    changed
}

fn playing(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Playing", vec2(190.0, height), |ui| {
        ui.label(dim(ui, "Which note plays"));
        changed |= param_switch_as(ui, s, p, at::PRIORITY, &["Lowest", "Highest", "Last"]);
        ui.horizontal(|ui| {
            ui.label(dim(ui, "Retrigger"));
            changed |= param_switch(ui, s, p, at::RETRIGGER);
        });
        changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::VELOCITY, "Velocity"),
                (at::BEND, "Bend"),
                (at::VINTAGE, "Drift"),
            ],
        );
    });
    changed
}

/// The panel driven through the assistant interface and the controls.
#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    fn value(d: &mut Driver, t: &str, param: &str) -> f64 {
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == format!("instrument.{param}"))
            .and_then(|p| p["value"].as_f64())
            .unwrap()
    }

    #[test]
    fn patch_twenty_is_a_three_unit_panel_an_assistant_sees_and_steers() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "patch-twenty"}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();
        // Its modules' knobs and switches, the patch points among them.
        for label in [
            "High-pass cutoff",
            "Low-pass cutoff",
            "Mod generator shape",
            "Envelope 2 hold",
            "Trigger threshold",
            "Cord 1 amount",
            "Osc 1 wave: Tri",
            "Amplifier follows: Gate",
            "Show in rack",
        ] {
            d.h.get_by_label(label);
        }
        // A switch sets what an assistant reads...
        d.h.get_by_label("Osc 2 wave: Ring").click();
        d.h.run();
        assert_eq!(value(&mut d, &t, "osc2"), 3.0);
        d.h.get_by_label("External input: On").click();
        d.h.run();
        assert_eq!(value(&mut d, &t, "ext_in"), 1.0);
        // ...and what an assistant sets shows on the panel.
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.lp_cutoff", "value": 500.0}),
        )
        .unwrap();
        d.h.run();
        d.h.get_by_value("500 Hz");
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.patch2_to", "value": 8.0}),
        )
        .unwrap();
        d.h.run();
        d.h.get_by_label("Cord 2 to");
        // The unit is the kit's three units high, in the rack the
        // assistant reads.
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["rack"][0]["view"], "shown", "{info}");
    }
}
