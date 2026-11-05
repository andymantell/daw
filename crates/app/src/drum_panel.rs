// SPDX-License-Identifier: GPL-3.0-or-later
//! The drum machine's front panel (UI polish step 5): the sixteen pads,
//! the kit's level, the chosen pad's own settings, and then what the pad
//! is made of: its four layers, its effects, or where it comes out.
//!
//! Modules run left to right as the sound goes: the pads (the lowest at
//! the bottom left, as on hardware), the kit, the pad, and then one of
//! three views chosen on the pad's module (Layers, Effects, Outputs).
//! Every setting is a drum machine setting an assistant reaches through
//! `set_parameter`; the layers' sounds through `set_sound`, the pads'
//! effects through `edit_pad_effect`, the outputs through
//! `set_instrument_output`, and which pad and view show through
//! `show_pad`.

use std::sync::Arc;

use daw_instruments::drum_machine as dm;
use daw_instruments::params::Params;
use daw_instruments::{Kind, PadFx};
use eframe::egui::{
    self, Align2, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::commands::Cmd;
use crate::instrument::{Ctx, Shown, Slot};
use crate::panel_kit::{HEADER, Knob, Size, Switch, UNIT, module, param_knob, param_switch};
use crate::theme;

/// The drum machine's height in the rack, in units.
pub(crate) const UNITS: u8 = 3;

/// The tallest the panel's body is: its rack units, less the plate's
/// margins and the header.
const BODY: f32 = UNITS as f32 * UNIT - 12.0 - HEADER - 8.0;

/// What the pad module shows beside it.
pub(crate) const PARTS: [&str; 3] = ["Layers", "Effects", "Outputs"];

/// The ids `show_pad` and `get_instrument` give the parts.
pub(crate) const PART_IDS: [&str; 3] = ["layers", "effects", "outputs"];

const PAD_W: f32 = 60.0;
const PAD_H: f32 = 38.0;
const PAD_GAP: f32 = 4.0;

/// The drum machine's panel in `slot`, with where its extra outputs go.
/// True if the instrument needs making again (a sound or an effect
/// came or went).
pub(crate) fn drum_machine(
    ui: &mut Ui,
    slot: &mut Slot,
    routes: &mut Vec<daw_project::InstrumentOutput>,
    cx: &Ctx,
    shown: &mut Shown,
) -> bool {
    let height = ui.available_height().clamp(120.0, BODY);
    let id = slot.id;
    let names: Vec<String> = (0..dm::PADS).map(|p| slot.pad_name(p)).collect();
    let in_use = slot.outputs_in_use();
    let mut pad = slot.pad.min(dm::PADS - 1);
    let mut part = slot.tab.min(PARTS.len() - 1);
    let mut renew = false;
    let Some(b) = slot.builtin_mut() else {
        return false;
    };
    if b.kind() != Kind::DrumMachine {
        return false;
    }
    let p = b.params().clone();
    let marks: Vec<PadMarks> = (0..dm::PADS)
        .map(|k| PadMarks {
            sound: (0..dm::LAYERS).any(|l| p.get(dm::layer_at(k, l, dm::layer::SOURCE)) > 0.0),
            effects: b.fx.iter().filter(|f| f.pad == k).count(),
            output: p.get(dm::pad_at(k, dm::pad::OUTPUT)) as usize,
        })
        .collect();
    egui::ScrollArea::horizontal()
        .id_salt(("drum-machine", id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(k) = pads(ui, &names, &marks, pad, height) {
                    pad = k;
                }
                kit(ui, &p, id, height);
                pad_module(ui, &p, id, pad, &names[pad], &mut part, height);
                match part {
                    0 => {
                        for l in 0..dm::LAYERS {
                            renew |= layer(ui, b, &p, id, pad, l, cx, shown, height);
                        }
                    }
                    1 => renew |= effects(ui, b, id, pad, height),
                    _ => outputs(ui, &p, id, pad, &in_use, routes, cx, shown, height),
                }
            });
        });
    slot.pad = pad;
    slot.tab = part;
    renew
}

/// What a pad shows on its face besides its name.
struct PadMarks {
    sound: bool,
    effects: usize,
    output: usize,
}

/// Cuts `text` to fit `width` in `font`, with an ellipsis.
fn fit(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    let wide = |t: &str| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(t.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    if wide(text) <= width {
        return text.to_string();
    }
    let mut s: String = text.into();
    while !s.is_empty() && wide(&format!("{s}…")) > width {
        s.pop();
    }
    format!("{}…", s.trim_end())
}

/// The sixteen pads, four by four with the lowest at the bottom left;
/// the pad clicked, if one was.
fn pads(
    ui: &mut Ui,
    names: &[String],
    marks: &[PadMarks],
    chosen: usize,
    height: f32,
) -> Option<usize> {
    let width = 4.0 * PAD_W + 3.0 * PAD_GAP + 16.0;
    let mut picked = None;
    module(ui, "Pads", vec2(width, height), |ui| {
        let look = theme::look(ui.ctx());
        let c = look.c();
        let visuals = ui.visuals().clone();
        let small = look.font(look.small);
        let tiny = look.numerals_font_at(look.small * 0.9);
        let (_, area) = ui.allocate_space(vec2(
            4.0 * PAD_W + 3.0 * PAD_GAP,
            4.0 * PAD_H + 3.0 * PAD_GAP,
        ));
        let radius = CornerRadius::same(look.radius.min(5.0) as u8);
        for row in 0..4 {
            for col in 0..4 {
                // The bottom row is the first four.
                let k = (3 - row) * 4 + col;
                let rect = Rect::from_min_size(
                    pos2(
                        area.left() + col as f32 * (PAD_W + PAD_GAP),
                        area.top() + row as f32 * (PAD_H + PAD_GAP),
                    ),
                    vec2(PAD_W, PAD_H),
                );
                let note = daw_instruments::params::note_name(dm::FIRST_NOTE as i32 + k as i32);
                let label = format!("Pad {}: {}, {note}", k + 1, names[k]);
                // ai: show_pad.pad
                let r = ui.interact(rect, ui.scope_id().with(("pad", k)), Sense::click());
                let on = k == chosen;
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &label));
                let r = r.accessible_name(label).on_hover_text(format!(
                    "{}: choose the pad to change. It plays on {note}",
                    names[k]
                ));
                if r.clicked() {
                    picked = Some(k);
                }
                let m = &marks[k];
                let fill = if on {
                    visuals.selection.bg_fill
                } else if r.hovered() {
                    visuals.widgets.hovered.bg_fill
                } else if m.sound {
                    visuals.widgets.inactive.bg_fill
                } else {
                    theme::mix(c.plate, c.plate_shadow, 0.35)
                };
                let text = if on {
                    visuals.selection.stroke.color
                } else if m.sound {
                    c.text
                } else {
                    c.text_dim
                };
                let p = ui.painter();
                p.rect(
                    rect,
                    radius,
                    fill,
                    Stroke::new(1.0, if on { c.accent } else { c.line }),
                    StrokeKind::Inside,
                );
                let name = fit(ui, &names[k], &small, PAD_W - 8.0);
                p.text(
                    rect.left_top() + vec2(4.0, 3.0),
                    Align2::LEFT_TOP,
                    name,
                    small.clone(),
                    text,
                );
                p.text(
                    rect.left_bottom() + vec2(4.0, -3.0),
                    Align2::LEFT_BOTTOM,
                    note,
                    tiny.clone(),
                    if on { text } else { c.text_dim },
                );
                // Marks for a pad with effects or an output of its own.
                let mut tag = String::new();
                if m.effects > 0 {
                    tag.push_str("fx");
                }
                if m.output > 0 {
                    if !tag.is_empty() {
                        tag.push(' ');
                    }
                    tag.push_str(&format!("›{}", m.output));
                }
                if !tag.is_empty() {
                    p.text(
                        rect.right_bottom() + vec2(-4.0, -3.0),
                        Align2::RIGHT_BOTTOM,
                        tag,
                        tiny.clone(),
                        if on { text } else { c.accent },
                    );
                }
            }
        }
    });
    picked
}

/// The whole kit's level and how much velocity matters.
fn kit(ui: &mut Ui, p: &Params, id: u64, height: f32) {
    module(ui, "Kit", vec2(76.0, height), |ui| {
        ui.vertical_centered(|ui| {
            param_knob(
                ui,
                ("drum-kit", id),
                p,
                dm::at::LEVEL,
                Some("Level"),
                Size::Normal,
            );
            param_knob(
                ui,
                ("drum-kit", id),
                p,
                dm::at::VELOCITY,
                Some("Velocity"),
                Size::Small,
            );
        });
    });
}

fn dim(ui: &Ui, text: &str) -> RichText {
    let look = theme::look(ui.ctx());
    RichText::new(text)
        .size(look.small)
        .color(look.c().text_dim)
}

/// The chosen pad: its level, pan and tune, its choke group, how it
/// plays its notes, and which of its parts shows beside it.
fn pad_module(
    ui: &mut Ui,
    p: &Params,
    id: u64,
    pad: usize,
    name: &str,
    part: &mut usize,
    height: f32,
) {
    let heading = format!("Pad {} · {name}", pad + 1);
    let modes = Switch::width_of(ui, dm::MODES) + 20.0;
    module(ui, &heading, vec2(modes.max(236.0), height), |ui| {
        let salt = ("drum-pad", id, pad);
        ui.horizontal(|ui| {
            for (k, label) in [
                (dm::pad::LEVEL, "Level"),
                (dm::pad::PAN, "Pan"),
                (dm::pad::TUNE, "Tune"),
            ] {
                param_knob(ui, salt, p, dm::pad_at(pad, k), Some(label), Size::Small);
            }
        });
        ui.label(dim(ui, "Choke group")).on_hover_text(
            "A pad cuts off the others in its group, as an open hi-hat is cut by a closed one",
        );
        let choke = dm::pad_at(pad, dm::pad::CHOKE);
        let groups: Vec<String> = std::iter::once("Off".to_string())
            .chain((1..=dm::CHOKES).map(|g| g.to_string()))
            .collect();
        let groups: Vec<&str> = groups.iter().map(String::as_str).collect();
        // ai: set_parameter
        if let Some(g) = Switch::new("Choke group", &groups, p.get(choke) as usize)
            .small()
            .show(ui, (salt, "choke"))
        {
            p.set(choke, g as f64);
        }
        param_switch(ui, salt, p, dm::pad_at(pad, dm::pad::MODE));
        ui.add_space(2.0);
        ui.label(dim(ui, "Show"));
        // ai: show_pad.part
        if let Some(k) = Switch::new("Show", &PARTS, *part).show(ui, (salt, "part")) {
            *part = k;
        }
    });
}

/// One of the pad's layers: what it plays (off, its sample, or a drum
/// made here), and its level, tune, pan, decay, tone or start, and the
/// velocities it plays for. True if its sound changed.
#[allow(clippy::too_many_arguments)]
fn layer(
    ui: &mut Ui,
    b: &mut daw_instruments::Builtin,
    p: &Params,
    id: u64,
    pad: usize,
    l: usize,
    cx: &Ctx,
    shown: &mut Shown,
    height: f32,
) -> bool {
    let src = dm::layer_at(pad, l, dm::layer::SOURCE);
    let source = p.get(src) as usize;
    let on = source > 0;
    let sample = source == dm::SAMPLE as usize;
    let width = match (on, sample) {
        (false, _) => 128.0,
        (true, false) => 234.0,
        (true, true) => 310.0,
    };
    let mut renew = false;
    let salt = ("drum-layer", id, pad, l);
    module(ui, &format!("Layer {}", l + 1), vec2(width, height), |ui| {
        ui.horizontal(|ui| {
            param_switch(ui, salt, p, src);
            if sample {
                let key = dm::sample_key(pad, l);
                let current = b
                    .sample(&key)
                    .map(|s| s.reference.clone())
                    .unwrap_or_default();
                if let Some(r) = crate::instrument::sound_menu(ui, (salt, "sound"), &current)
                    && let Some(s) = crate::instrument::read_sound(&r, cx, shown)
                {
                    b.set_sample(&key, Some(s));
                    renew = true;
                }
            }
        });
        if !on {
            ui.label(dim(ui, "Off: choose a drum or a sample to layer it"));
            return;
        }
        // Tone means nothing to a sample, start nothing to a drum made
        // here.
        let colour = if sample {
            (dm::layer::START, "Start")
        } else {
            (dm::layer::TONE, "Tone")
        };
        for row in [
            &[
                (dm::layer::LEVEL, "Level"),
                (dm::layer::TUNE, "Tune"),
                (dm::layer::PAN, "Pan"),
                (dm::layer::DECAY, "Decay"),
            ][..],
            &[
                colour,
                (dm::layer::VEL_LO, "Softest"),
                (dm::layer::VEL_HI, "Hardest"),
            ][..],
        ] {
            ui.horizontal(|ui| {
                for &(k, label) in row {
                    param_knob(
                        ui,
                        salt,
                        p,
                        dm::layer_at(pad, l, k),
                        Some(label),
                        Size::Small,
                    );
                }
            });
        }
    });
    renew
}

/// The pad's effects in order, each a module of its controls, and one
/// to add another. True if an effect came, went, or was switched.
fn effects(
    ui: &mut Ui,
    b: &mut daw_instruments::Builtin,
    id: u64,
    pad: usize,
    height: f32,
) -> bool {
    let mut renew = false;
    let none = !b.fx.iter().any(|f| f.pad == pad);
    module(ui, "Add an effect", vec2(150.0, height), |ui| {
        egui::ComboBox::from_id_salt(("pad-fx-add", id, pad))
            .selected_text("Add…")
            .width(120.0)
            .show_ui(ui, |ui| {
                for k in crate::instrument::pad_effects() {
                    if ui
                        // ai: edit_pad_effect.add
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
        ui.label(dim(
            ui,
            if none {
                "No effects on this pad yet. They play in order, left to right."
            } else {
                "They play in order, left to right."
            },
        ));
    });
    let mut remove = None;
    for (fi, f) in b.fx.iter_mut().enumerate().filter(|(_, f)| f.pad == pad) {
        let kind = f.effect.kind();
        let params = kind.params();
        let knobs = params
            .iter()
            .filter(|x| {
                !matches!(
                    x.unit,
                    daw_effects::Unit::Switch | daw_effects::Unit::Choice
                )
            })
            .count();
        let width = (knobs.div_ceil(2) as f32 * 62.0 + 16.0).max(196.0);
        module(ui, kind.name(), vec2(width, height), |ui| {
            ui.horizontal(|ui| {
                // ai: edit_pad_effect.on
                if let Some(k) = Switch::new(
                    format!("{} on", kind.name()),
                    &["Off", "On"],
                    usize::from(!f.bypass),
                )
                .small()
                .show(ui, ("pad-fx-on", &f.id))
                {
                    f.bypass = k == 0;
                    renew = true;
                }
                // ai: edit_pad_effect.remove
                if ui.small_button("Remove").clicked() {
                    remove = Some(fi);
                }
            });
            egui::ScrollArea::vertical()
                .id_salt(("pad-fx-scroll", &f.id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        effect_controls(ui, &f.id, &f.effect);
                    });
                });
        });
    }
    if let Some(fi) = remove {
        b.fx.remove(fi);
        renew = true;
    }
    renew
}

/// A built-in effect's settings as knobs, with switches for its
/// choices.
fn effect_controls(ui: &mut Ui, key: &str, e: &daw_effects::Effect) {
    let kind = e.kind();
    for (i, x) in kind.params().iter().enumerate() {
        if !e.shown(x.key) {
            continue;
        }
        let v = e.get(i);
        match x.unit {
            daw_effects::Unit::Switch => {
                ui.vertical(|ui| {
                    ui.label(dim(ui, x.name));
                    // ai: edit_pad_effect.settings
                    if let Some(k) = Switch::new(x.name, &["Off", "On"], usize::from(v >= 0.5))
                        .small()
                        .show(ui, ("pad-fx", key, i))
                    {
                        e.set(i, k as f64);
                    }
                });
            }
            daw_effects::Unit::Choice if x.choices.len() <= 4 => {
                ui.vertical(|ui| {
                    ui.label(dim(ui, x.name));
                    // ai: edit_pad_effect.settings
                    if let Some(k) = Switch::new(x.name, x.choices, v as usize)
                        .small()
                        .show(ui, ("pad-fx", key, i))
                    {
                        e.set(i, k as f64);
                    }
                });
            }
            daw_effects::Unit::Choice => {
                ui.vertical(|ui| {
                    ui.label(dim(ui, x.name));
                    let mut pick = v as usize;
                    let r = egui::ComboBox::from_id_salt(("pad-fx", key, i))
                        .selected_text(x.text(v))
                        // ai: edit_pad_effect.settings
                        .show_index(ui, &mut pick, x.choices.len(), |c| x.choices[c])
                        .accessible_name(x.name);
                    if r.changed() {
                        e.set(i, pick as f64);
                    }
                });
            }
            _ => {
                let mut nv = v;
                let first = crate::spoken::start(ui);
                // ai: edit_pad_effect.settings
                let r = Knob::new(&mut nv, x.min, x.max, x.name)
                    .label(short(x.name))
                    .log(x.log)
                    .default(x.default)
                    .readout(x.text(v))
                    .size(Size::Small)
                    .show(ui, ("pad-fx", key, i));
                let hear = if x.unit == daw_effects::Unit::Db {
                    crate::spoken::Hear::Level(nv)
                } else {
                    crate::spoken::steady(nv, x.min, x.max)
                };
                crate::spoken::hearable(ui, first, x.text(nv), hear);
                if r.changed() {
                    e.set(i, nv);
                }
            }
        }
    }
    if kind.has_meter() {
        let db = e.reduction_db();
        ui.vertical(|ui| {
            ui.label(dim(ui, "Turning down"));
            ui.label(dim(ui, &format!("{:.1} dB", (-db).max(0.0))));
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// A setting's name short enough to sit under a small knob.
fn short(name: &str) -> String {
    name.replace(" width (Q)", " Q")
        .replace("frequency", "freq")
        .replace("middle", "mid")
}

/// Where the pad comes out (the track, or an output of its own) and its
/// two sends; then where each output in use goes.
#[allow(clippy::too_many_arguments)]
fn outputs(
    ui: &mut Ui,
    p: &Params,
    id: u64,
    pad: usize,
    in_use: &[(u32, String)],
    routes: &mut Vec<daw_project::InstrumentOutput>,
    cx: &Ctx,
    shown: &mut Shown,
    height: f32,
) {
    module(ui, "Pad out", vec2(196.0, height), |ui| {
        ui.label(dim(ui, "Output"));
        let i = dm::pad_at(pad, dm::pad::OUTPUT);
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
            .width(150.0)
            .show_ui(ui, |ui| {
                for n in 0..=dm::OUTPUTS {
                    // ai: set_parameter
                    ui.selectable_value(&mut o, n, text(n));
                }
            })
            .response
            .accessible_name("Output")
            .on_hover_text(
                "An output of its own can go to a bus in the mixer, beside. One that goes \
                 nowhere plays through the track",
            );
        if o != p.get(i) as usize {
            p.set(i, o as f64);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (k, label) in [(dm::pad::SEND_A, "Send A"), (dm::pad::SEND_B, "Send B")] {
                param_knob(
                    ui,
                    ("drum-pad", id, pad),
                    p,
                    dm::pad_at(pad, k),
                    Some(label),
                    Size::Small,
                );
            }
        });
    });
    module(ui, "Where the outputs go", vec2(400.0, height), |ui| {
        if ui
            // ai: pad-buses
            .small_button("Put each pad on a bus")
            .on_hover_text(
                "Gives every pad with a sound an output of its own and a bus in the mixer for it",
            )
            .clicked()
        {
            shown.cmd = Some(Cmd::PadBuses);
        }
        if in_use.is_empty() {
            ui.label(dim(
                ui,
                "Every pad plays through the track. Give a pad an output, or turn up a send, to route it.",
            ));
        }
        egui::ScrollArea::vertical()
            .id_salt(("drum-routes", id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                crate::instrument::route_rows(ui, id, in_use, routes, cx, shown);
            });
    });
}

/// Which pad and part the panel shows, for an assistant.
pub(crate) fn shown(slot: &Slot) -> serde_json::Value {
    let pad = slot.pad.min(dm::PADS - 1);
    serde_json::json!({
        "pad": pad,
        "name": slot.pad_name(pad),
        "part": PART_IDS[slot.tab.min(PART_IDS.len() - 1)],
    })
}

/// A drum machine's pads for `get_instrument`: each one's key, name,
/// whether it has a sound, its layers playing, its output (0 the
/// track) and how many effects it has; with the pad and part shown.
/// `None` for any other instrument.
pub(crate) fn pads_value(slot: &Slot) -> Option<serde_json::Value> {
    let b = slot
        .builtin_ref()
        .filter(|b| !slot.is_plugin() && b.kind() == Kind::DrumMachine)?;
    let p = b.params();
    let pads: Vec<serde_json::Value> = (0..dm::PADS)
        .map(|k| {
            let layers: Vec<serde_json::Value> = (0..dm::LAYERS)
                .filter_map(|l| {
                    let src = p.get(dm::layer_at(k, l, dm::layer::SOURCE)) as usize;
                    (src > 0).then(|| {
                        serde_json::json!({"layer": l, "plays": dm::SOURCES.get(src).copied().unwrap_or("?")})
                    })
                })
                .collect();
            serde_json::json!({
                "pad": k,
                "key": dm::FIRST_NOTE as usize + k,
                "name": slot.pad_name(k),
                "layers": layers,
                "output": p.get(dm::pad_at(k, dm::pad::OUTPUT)) as usize,
                "effects": b.fx.iter().filter(|f| f.pad == k).count(),
            })
        })
        .collect();
    Some(serde_json::json!({"list": pads, "shown": shown(slot)}))
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    fn param(d: &mut Driver, t: &str, key: &str) -> f64 {
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == key)
            .and_then(|p| p["value"].as_f64())
            .unwrap_or_else(|| panic!("no {key}"))
    }

    fn shown(d: &mut Driver, t: &str) -> Value {
        d.call("get_instrument", json!({"track": t})).unwrap()["pads"]["shown"].clone()
    }

    #[test]
    fn the_drum_machine_panel_is_a_rack_unit_an_assistant_sees_and_steers() {
        let mut d = Driver::new();
        let t = d
            .call(
                "add_track",
                json!({"kind": "drum-machine", "name": "Drums"}),
            )
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();

        // The pads, the kit and the pad's module, in the rack.
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        let pads = info["pads"]["list"].as_array().unwrap();
        assert_eq!(pads.len(), 16, "{info}");
        assert_eq!(pads[0]["name"], "Kick");
        assert_eq!(pads[0]["key"], 36);
        assert_eq!(
            info["pads"]["shown"],
            json!({"pad": 0, "name": "Kick", "part": "layers"})
        );
        assert_eq!(info["rack"][0]["view"], "shown");
        d.h.get_by_label("Show in rack");
        d.h.get_by_label("Pad 1: Kick, C2");
        d.h.get_by_label("Show: Layers");

        // A click on a pad shows it, and an assistant reads which.
        d.h.get_by_label("Pad 3: Snare, D2").click();
        d.h.run();
        assert_eq!(shown(&mut d, &t)["pad"], 2);
        assert_eq!(shown(&mut d, &t)["name"], "Snare");

        // The choke switch sets the pad's setting.
        d.h.get_by_label("Choke group: 2").click();
        d.h.run();
        assert_eq!(param(&mut d, &t, "instrument.pad2.choke"), 2.0);

        // The layers' knobs set the layer's settings.
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.pad2.layer1.source", "value": 0.0}),
        )
        .unwrap();
        d.h.run();
        let off = "Off: choose a drum or a sample to layer it";
        assert_eq!(d.h.query_all_by_label(off).count(), 3, "layers 2 to 4 off");

        // An assistant shows the pad's effects; one added shows as a
        // module, and its switch turns it off.
        d.call("show_pad", json!({"track": t, "part": "effects"}))
            .unwrap();
        d.h.run();
        d.h.get_by_label("Add an effect to the pad");
        let kind = crate::instrument::pad_effects().next().unwrap();
        d.call(
            "edit_pad_effect",
            json!({"track": t, "pad": 2, "add": kind.id()}),
        )
        .unwrap();
        d.h.run();
        d.h.run();
        d.h.get_by_label(&format!("{} on: Off", kind.name()))
            .click();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["instrument"]["pad_effects"][0]["on"], false, "{info}");
        assert_eq!(info["pads"]["list"][2]["effects"], 1);

        // The outputs: the pad's output and the bus button; the part
        // switch goes back to the layers.
        d.call("show_pad", json!({"track": t, "pad": 0, "part": "outputs"}))
            .unwrap();
        d.h.run();
        assert_eq!(
            shown(&mut d, &t),
            json!({"pad": 0, "name": "Kick", "part": "outputs"})
        );
        d.h.get_by_label("Put each pad on a bus");
        d.h.get_by_label("Send A");
        d.h.get_by_label("Show: Layers").click();
        d.h.run();
        assert_eq!(shown(&mut d, &t)["part"], "layers");

        // Bad asks are refused.
        assert!(d.call("show_pad", json!({"track": t, "pad": 16})).is_err());
        assert!(
            d.call("show_pad", json!({"track": t, "part": "sounds"}))
                .is_err()
        );
    }
}
