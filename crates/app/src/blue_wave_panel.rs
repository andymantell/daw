// SPDX-License-Identifier: GPL-3.0-or-later
//! Blue Wave's front panel (UI polish step 5): the wavetable synth on
//! the panel kit.
//!
//! Its modules run left to right in signal order: the wavetable, with
//! the waves the two oscillators read drawn live and the place in the
//! table they read from; oscillator B; the wave envelope that moves
//! through the table; the filter with its response; the filter and
//! level envelopes; the LFO; and playing. Every control is a setting of
//! the synth, so an assistant reaches each through `set_parameter`.

use daw_instruments::blue_wave::at;
use daw_instruments::params::Params;
use daw_instruments::wavetable::{self, Table};
use eframe::egui::{self, Pos2, RichText, Stroke, Ui, pos2, vec2};

use crate::panel_kit::{
    FilterShape, Size, envelope_display, filter_display, module, param_combo, param_knob,
    param_switch, param_switch_as, screen,
};
use crate::theme;

/// The panel's modules, `height` tall, carrying on in a
/// second row when the pane is narrower than they are. True if a setting moved.
pub(crate) fn blue_wave(ui: &mut Ui, p: &Params, salt: u64, height: f32) -> bool {
    let mut changed = false;
    crate::panel_kit::modules(ui, 8.0, |ui| {
        let s = ("blue-wave", salt);
        changed |= wavetable_module(ui, p, s, height);
        changed |= oscillator_b(ui, p, s, height);
        changed |= wave_envelope(ui, p, s, height);
        changed |= filter(ui, p, s, height);
        changed |= envelopes(ui, p, s, height);
        changed |= lfo(ui, p, s, height);
        changed |= playing(ui, p, s, height);
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

/// The table's screen: the wave oscillator A reads (bright) and the one
/// B reads (faint), as the table holds them, in the character chosen;
/// under them a strip of the table's 64 places with where each reads.
fn table_display(ui: &mut Ui, size: egui::Vec2, p: &Params) {
    let (rect, look) = screen(ui, size, 4, 2);
    let c = look.c();
    let table = Table::get(p.get(at::TABLE) as usize);
    let eight_bit = p.get(at::CHARACTER) >= 0.5;
    let last = (wavetable::WAVES - 1) as f32;
    let a = (p.get(at::WAVE) as f32).clamp(0.0, last);
    let b = (a + p.get(at::B_WAVE) as f32).clamp(0.0, last);
    let strip = 9.0;
    let wave_rect =
        egui::Rect::from_min_max(rect.min + vec2(3.0, 3.0), rect.max - vec2(3.0, strip + 5.0));
    let read = |position: f32, phase: f32| {
        if eight_bit {
            table.rough(position, phase)
        } else {
            table.smooth(position, 0, phase)
        }
    };
    let line = |position: f32| -> Vec<Pos2> {
        let n = 96;
        // The waves are made to peak near one; scale to the tallest so
        // a quiet one still shows its shape.
        let peak = (0..=n)
            .map(|i| read(position, i as f32 / n as f32).abs())
            .fold(0.05f32, f32::max);
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let v = read(position, t) / peak;
                pos2(
                    wave_rect.left() + wave_rect.width() * t,
                    wave_rect.center().y - wave_rect.height() * 0.46 * v,
                )
            })
            .collect()
    };
    if (b - a).abs() > 0.01 {
        ui.painter().add(egui::Shape::line(
            line(b),
            Stroke::new(1.0, theme::mix(c.accent, c.meter_bg, 0.55)),
        ));
    }
    ui.painter()
        .add(egui::Shape::line(line(a), Stroke::new(1.5, c.accent)));
    // The places in the table.
    let bar = egui::Rect::from_min_max(
        pos2(rect.left() + 4.0, rect.bottom() - strip - 3.0),
        pos2(rect.right() - 4.0, rect.bottom() - 3.0),
    );
    let painter = ui.painter();
    let x = |w: f32| bar.left() + bar.width() * w / last;
    for w in 0..wavetable::WAVES {
        let tall = if w % 8 == 0 { 1.0 } else { 0.5 };
        let px = x(w as f32);
        painter.line_segment(
            [
                pos2(px, bar.bottom()),
                pos2(px, bar.bottom() - bar.height() * tall),
            ],
            Stroke::new(1.0, theme::mix(c.line, c.meter_bg, 0.3)),
        );
    }
    painter.circle_filled(
        pos2(x(b), bar.center().y),
        2.5,
        theme::mix(c.accent, c.meter_bg, 0.45),
    );
    painter.circle_filled(pos2(x(a), bar.center().y), 3.0, c.accent);
    painter.text(
        rect.right_top() + vec2(-4.0, 2.0),
        egui::Align2::RIGHT_TOP,
        format!("A {:.0}  B {:.0}", a, b),
        look.numerals_font_at(look.small),
        c.text_dim,
    );
}

fn wavetable_module(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Wavetable", vec2(318.0, height), |ui| {
        ui.horizontal(|ui| {
            changed |= param_combo(ui, s, p, at::TABLE, 112.0);
            changed |= param_switch(ui, s, p, at::CHARACTER);
        });
        table_display(ui, vec2(ui.available_width(), 56.0), p);
        ui.horizontal(|ui| {
            changed |= param_knob(ui, s, p, at::WAVE, Some("Wave"), Size::Normal);
            for (i, label) in [
                (at::WAVE_ENV, "Envelope"),
                (at::WAVE_LFO, "LFO"),
                (at::KEY_WAVE, "Keys"),
                (at::WHEEL_WAVE, "Wheel"),
            ] {
                changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
            }
        });
    });
    changed
}

fn oscillator_b(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Oscillator B", vec2(132.0, height), |ui| {
        changed |= knobs(
            ui,
            s,
            p,
            &[(at::B_WAVE, "Wave offset"), (at::B_SEMI, "Transpose")],
        );
        changed |= knobs(
            ui,
            s,
            p,
            &[(at::B_DETUNE, "Detune"), (at::MIX, "Mix A / B")],
        );
    });
    changed
}

fn wave_envelope(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Wave envelope", vec2(232.0, height), |ui| {
        envelope_display(
            ui,
            vec2(ui.available_width(), 34.0),
            Some("Through the table"),
            p.get(at::W_ATTACK),
            p.get(at::W_DECAY),
            p.get(at::W_SUSTAIN),
            p.get(at::W_RELEASE),
        );
        changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::W_ATTACK, "Attack"),
                (at::W_DECAY, "Decay"),
                (at::W_SUSTAIN, "Sustain"),
                (at::W_RELEASE, "Release"),
            ],
        );
        ui.label(dim(
            ui,
            "Envelope on the wavetable sets how far\nit moves the wave, and which way.",
        ));
    });
    changed
}

fn filter(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Filter", vec2(296.0, height), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        filter_display(
            ui,
            vec2(ui.available_width(), 40.0),
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
                    &[
                        (at::RESONANCE, "Resonance"),
                        (at::FILTER_ENV, "Envelope"),
                        (at::KEY_TRACK, "Keys"),
                    ],
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    &[(at::FILTER_LFO, "LFO"), (at::VELOCITY_FILTER, "Velocity")],
                );
            });
        });
    });
    changed
}

fn envelopes(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Envelopes", vec2(222.0, height), |ui| {
        for (name, a, d, su, r) in [
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
                p.get(su),
                p.get(r),
            );
            changed |= knobs(
                ui,
                s,
                p,
                &[(a, "Attack"), (d, "Decay"), (su, "Sustain"), (r, "Release")],
            );
        }
    });
    changed
}

fn lfo(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "LFO", vec2(176.0, height), |ui| {
        changed |= param_switch_as(ui, s, p, at::LFO_SHAPE, &["Tri", "Saw", "Square", "Random"]);
        changed |= knobs(
            ui,
            s,
            p,
            &[(at::LFO_RATE, "Rate"), (at::LFO_DELAY, "Delay")],
        );
        changed |= knobs(
            ui,
            s,
            p,
            &[(at::LFO_PITCH, "To pitch"), (at::WHEEL, "Wheel")],
        );
    });
    changed
}

fn playing(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "Playing", vec2(244.0, height), |ui| {
        ui.horizontal(|ui| {
            changed |= param_switch(ui, s, p, at::MODE);
            ui.label(dim(ui, "Glide"));
            changed |= param_switch(ui, s, p, at::PORTAMENTO);
        });
        changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::VOICES, "Voices"),
                (at::UNISON_DETUNE, "Detune"),
                (at::SPREAD, "Spread"),
                (at::GLIDE, "Glide time"),
            ],
        );
        ui.horizontal(|ui| {
            changed |= param_knob(ui, s, p, at::LEVEL, Some("Level"), Size::Small);
            for (i, label) in [
                (at::VELOCITY, "Velocity"),
                (at::BEND, "Bend"),
                (at::VINTAGE, "Drift"),
            ] {
                changed |= param_knob(ui, s, p, i, Some(label), Size::Small);
            }
        });
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
    fn blue_wave_is_a_three_unit_panel_an_assistant_sees_and_steers() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "blue-wave"})).unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();
        for label in [
            "Wavetable",
            "Wave",
            "Oscillator B wave offset",
            "Wave attack",
            "Cutoff",
            "Filter release",
            "LFO shape: Random",
            "Playing: Unison",
            "Show in rack",
        ] {
            d.h.get_by_label(label);
        }
        d.h.get_by_label("Character: Eight-bit").click();
        d.h.run();
        assert_eq!(value(&mut d, &t, "character"), 1.0);
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.wave", "value": 40.0}),
        )
        .unwrap();
        d.h.run();
        d.h.get_by_value("40");
        // The header steps through its sounds, which the table follows.
        d.h.get_by_label("Next sound").click();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert!(info["sound"].is_string(), "{info}");
    }
}
