// SPDX-License-Identifier: GPL-3.0-or-later
//! Lucky Seven's front panel (UI polish step 5): the six-operator FM
//! synth on the panel kit.
//!
//! Its modules run left to right in signal order: the algorithm, drawn
//! as a small diagram of how the operators modulate each other; the six
//! operators side by side, each with its level, frequency and envelope;
//! the one operator being edited in full (its frequency, its envelope's
//! four rates and levels, or its keyboard scaling and touch); the LFO
//! and pitch envelope; and playing, with the bank. Every control is a
//! setting of the synth, so an assistant reaches each through
//! `set_parameter`; which operator is being edited is only the view.

use daw_instruments::lucky_seven::{self, OPS, at, op, op_at};
use daw_instruments::params::Params;
use eframe::egui::{self, Pos2, Rect, RichText, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::panel_kit::{
    Size, Switch, module, param_combo, param_knob, param_switch, param_switch_as, screen, trace,
};
use crate::theme;

/// What the panel's view keeps between frames: the operator being
/// edited and which page of it shows.
#[derive(Clone, Copy, Default)]
struct View {
    op: usize,
    scaling: bool,
}

/// What the panel asks of the instrument around it.
#[derive(Default)]
pub(crate) struct Asked {
    pub changed: bool,
    pub load_bank: bool,
}

/// The panel's modules, `height` tall, scrolling sideways when the pane
/// is narrower than they are. `bank` is the bank loaded, if one is; a
/// sound chosen from it is set with `choose`.
pub(crate) fn lucky_seven(
    ui: &mut Ui,
    p: &Params,
    salt: u64,
    height: f32,
    bank: Option<&lucky_seven::bank::Bank>,
    choose: &mut Option<usize>,
) -> Asked {
    let mut asked = Asked::default();
    let id = egui::Id::unique(("lucky-seven-view", salt));
    let mut view: View = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    egui::ScrollArea::horizontal()
        .id_salt(("lucky-seven", salt))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let s = ("lucky-seven", salt);
                asked.changed |= algorithm(ui, p, s, height, view.op);
                asked.changed |= operators(ui, p, s, height, &mut view);
                asked.changed |= operator(ui, p, s, height, &mut view);
                asked.changed |= modulation(ui, p, s, height);
                playing(ui, p, s, height, bank, choose, &mut asked);
            });
        });
    ui.ctx().data_mut(|d| d.insert_temp(id, view));
    asked
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

// ----------------------------------------------------------------------
// The algorithm

/// Where each operator sits in an algorithm's diagram: (column, row),
/// carriers on row 0 at the bottom, each modulator a row above the
/// first operator it modulates, over the middle of its own modulators.
pub(crate) fn layout(alg: &lucky_seven::Algorithm) -> ([(f32, usize); OPS], f32) {
    struct Lay<'a> {
        alg: &'a lucky_seven::Algorithm,
        at: [Option<(f32, usize)>; OPS],
        next: f32,
    }
    fn place(l: &mut Lay, o: usize, row: usize) -> f32 {
        if let Some((col, _)) = l.at[o] {
            return col;
        }
        // Held now, so a loop can't come back to it.
        l.at[o] = Some((0.0, row));
        let mods = l.alg.modulators[o];
        let mut cols = Vec::new();
        for m in 0..OPS {
            if m != o && mods & (1 << m) != 0 && l.at[m].is_none() {
                cols.push(place(l, m, row + 1));
            }
        }
        let col = if cols.is_empty() {
            let c = l.next;
            l.next += 1.0;
            c
        } else {
            cols.iter().sum::<f32>() / cols.len() as f32
        };
        l.at[o] = Some((col, row));
        col
    }
    let mut l = Lay {
        alg,
        at: [None; OPS],
        next: 0.0,
    };
    for o in 0..OPS {
        if alg.carriers & (1 << o) != 0 {
            place(&mut l, o, 0);
        }
    }
    // Anything left (none of the algorithms has one) goes on the end.
    for o in 0..OPS {
        if l.at[o].is_none() {
            place(&mut l, o, 0);
        }
    }
    (l.at.map(|a| a.unwrap_or((0.0, 0))), l.next.max(1.0))
}

/// The algorithm's diagram: a box per operator, filled when it is heard
/// (a carrier), lines from each modulator down to what it modulates,
/// the feedback loop at the side, and the operator being edited ringed.
fn algorithm_display(ui: &mut Ui, size: egui::Vec2, p: &Params, editing: usize) {
    let (rect, look) = screen(ui, size, 1, 1);
    let c = look.c();
    let alg = lucky_seven::algorithm(p.get(at::ALGORITHM) as f32);
    let (at_, columns) = layout(alg);
    let rows = at_.iter().map(|a| a.1).max().unwrap_or(0) + 1;
    let inner = rect.shrink2(vec2(10.0, 6.0));
    let bus = inner.bottom() - 2.0;
    let area = Rect::from_min_max(inner.min, pos2(inner.right(), bus - 6.0));
    let cell_w = area.width() / columns;
    let row_h = (area.height() / rows as f32).min(24.0);
    let size = vec2(cell_w.min(22.0), (row_h - 6.0).clamp(10.0, 16.0));
    let centre = |o: usize| {
        let (col, row) = at_[o];
        pos2(
            area.left() + cell_w * (col + 0.5),
            area.bottom() - row_h * (row as f32 + 0.5),
        )
    };
    let boxes: Vec<Rect> = (0..OPS)
        .map(|o| Rect::from_center_size(centre(o), size))
        .collect();
    let painter = ui.painter();
    let wire = Stroke::new(1.0, c.text_dim);
    // Modulator to modulated.
    for o in 0..OPS {
        for m in 0..OPS {
            if m != o && alg.modulators[o] & (1 << m) != 0 {
                painter.line_segment([boxes[m].center_bottom(), boxes[o].center_top()], wire);
            }
        }
    }
    // Carriers to the output bus.
    let carriers: Vec<usize> = (0..OPS).filter(|o| alg.carriers & (1 << o) != 0).collect();
    if let (Some(first), Some(last)) = (carriers.first(), carriers.last()) {
        for &o in &carriers {
            painter.line_segment(
                [boxes[o].center_bottom(), pos2(boxes[o].center().x, bus)],
                wire,
            );
        }
        painter.line_segment(
            [
                pos2(boxes[*first].center().x, bus),
                pos2(boxes[*last].center().x, bus),
            ],
            Stroke::new(1.5, c.accent),
        );
    }
    // The feedback loop, out of one operator's right side and back into
    // another's (or its own) top.
    let (from, to) = (
        (alg.feedback_from as usize).clamp(1, OPS) - 1,
        (alg.feedback_to as usize).clamp(1, OPS) - 1,
    );
    let fb = if p.get(at::FEEDBACK) > 0.0 {
        Stroke::new(1.5, c.accent)
    } else {
        wire
    };
    let out = boxes[from].right_center() + vec2(5.0, 0.0);
    let top = boxes[to].center_top() - vec2(0.0, 3.0);
    painter.add(egui::Shape::line(
        vec![
            boxes[from].right_center(),
            out,
            pos2(out.x, top.y),
            top,
            boxes[to].center_top(),
        ],
        fb,
    ));
    let font = look.numerals_font_at(look.small);
    for (o, b) in boxes.iter().enumerate() {
        let heard = alg.carriers & (1 << o) != 0;
        let (fill, text) = if heard {
            (c.accent, c.on_accent)
        } else {
            (c.meter_bg, c.text)
        };
        let ring = if o == editing {
            Stroke::new(2.0, c.text)
        } else {
            Stroke::new(1.0, c.accent)
        };
        painter.rect(*b, 2.0, fill, ring, StrokeKind::Outside);
        painter.text(
            b.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}", o + 1),
            font.clone(),
            text,
        );
    }
}

fn algorithm(ui: &mut Ui, p: &Params, s: Salt, height: f32, editing: usize) -> bool {
    let mut changed = false;
    module(ui, "Algorithm", vec2(196.0, height), |ui| {
        algorithm_display(ui, vec2(ui.available_width(), 86.0), p, editing);
        ui.horizontal(|ui| {
            changed |= param_knob(ui, s, p, at::ALGORITHM, Some("Algorithm"), Size::Small);
            changed |= param_knob(ui, s, p, at::FEEDBACK, Some("Feedback"), Size::Small);
            ui.vertical(|ui| {
                ui.label(dim(ui, "Key sync"))
                    .on_hover_text("The operators start from the top of their wave on each note");
                changed |= param_switch(ui, s, p, at::OSC_SYNC);
            });
        });
    });
    changed
}

// ----------------------------------------------------------------------
// The operators

/// What an operator's frequency reads as: a ratio of the note, or Hz.
pub(crate) fn frequency_text(p: &Params, o: usize) -> String {
    let coarse = p.get(op_at(o, op::COARSE)).round();
    let fine = p.get(op_at(o, op::FINE));
    if p.get(op_at(o, op::MODE)) >= 0.5 {
        let hz = 10f64.powi((coarse as i32).rem_euclid(4)) * 10f64.powf(fine / 100.0);
        if hz >= 1000.0 {
            format!("{:.2} kHz", hz / 1000.0)
        } else {
            format!("{hz:.1} Hz")
        }
    } else {
        let base = if coarse < 0.5 { 0.5 } else { coarse };
        format!("× {:.2}", base * (1.0 + fine / 100.0))
    }
}

/// Seconds a segment of an envelope takes at `rate` (0 to 99) to move
/// `by` levels of 99, as the synth runs it, roughly.
fn segment(rate: f64, by: f64) -> f64 {
    0.004 * ((99.0 - rate.clamp(0.0, 99.0)) / 7.0).exp2() * (by.abs() / 99.0).max(0.05)
}

/// A four-rate, four-level envelope: from level 4 to levels 1, 2 and 3
/// in turn, holding 3 while the key is down, then back to 4; drawn
/// with each segment's length on a log scale, and levels around the
/// middle when `centred` (a pitch envelope, whose 50 is no change).
fn rates_levels_display(
    ui: &mut Ui,
    size: egui::Vec2,
    caption: Option<&str>,
    rates: [f64; 4],
    levels: [f64; 4],
    centred: bool,
) {
    let (rect, look) = screen(ui, size, 4, if centred { 2 } else { 1 });
    let inner = rect.shrink(3.0);
    let share = |t: f64| ((t.max(0.001) / 0.001).ln() / (20.0f64 / 0.001).ln()).clamp(0.0, 1.0);
    let mut from = levels[3];
    let mut lengths = [0.0; 4];
    for (k, len) in lengths.iter_mut().enumerate() {
        let to = levels[if k == 3 { 3 } else { k }];
        *len = share(segment(rates[k], to - from)) * 0.8 + 0.1;
        from = to;
    }
    let hold = 0.5;
    let total: f64 = lengths.iter().sum::<f64>() + hold;
    let x = |t: f64| inner.left() + inner.width() * (t / total) as f32;
    let y = |l: f64| inner.bottom() - inner.height() * (l / 99.0).clamp(0.0, 1.0) as f32;
    let mut t = 0.0;
    let mut points = vec![pos2(inner.left(), y(levels[3]))];
    for k in 0..3 {
        t += lengths[k];
        points.push(pos2(x(t), y(levels[k])));
    }
    t += hold;
    points.push(pos2(x(t), y(levels[2])));
    points.push(pos2(inner.right(), y(levels[3])));
    if centred {
        trace_line(ui, points, &look);
    } else {
        // Close the fill along the bottom from the last point.
        trace(ui, rect, points, &look);
    }
    if let Some(caption) = caption {
        ui.painter().text(
            rect.right_top() + vec2(-4.0, 2.0),
            egui::Align2::RIGHT_TOP,
            caption,
            look.font(look.small),
            look.c().text_dim,
        );
    }
}

/// A trace with no fill under it, for a line that swings both ways.
fn trace_line(ui: &Ui, points: Vec<Pos2>, look: &theme::Look) {
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(1.5, look.c().accent)));
}

fn op_rates(p: &Params, o: usize) -> ([f64; 4], [f64; 4]) {
    (
        std::array::from_fn(|k| p.get(op_at(o, op::RATE1 + k))),
        std::array::from_fn(|k| p.get(op_at(o, op::LEVEL1 + k))),
    )
}

const OP_NAMES: [&str; OPS] = ["1", "2", "3", "4", "5", "6"];

/// The six operators side by side: each one's envelope, level and
/// frequency, heard ones marked; and which is being edited.
fn operators(ui: &mut Ui, p: &Params, s: Salt, height: f32, view: &mut View) -> bool {
    let mut changed = false;
    let alg = lucky_seven::algorithm(p.get(at::ALGORITHM) as f32);
    module(ui, "Operators", vec2(352.0, height), |ui| {
        ui.horizontal(|ui| {
            ui.label(dim(ui, "Edit"));
            // ai: view
            if let Some(k) = Switch::new("Edit operator", &OP_NAMES, view.op)
                .small()
                .show(ui, (s, "edit-op"))
            {
                view.op = k;
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for o in 0..OPS {
                ui.vertical(|ui| {
                    ui.set_width(Size::Small.cell());
                    let heard = alg.carriers & (1 << o) != 0;
                    let look = theme::look(ui.ctx());
                    let tag = if heard { "heard" } else { "mod" };
                    let colour = if heard {
                        look.c().accent
                    } else {
                        look.c().text_dim
                    };
                    ui.label(
                        RichText::new(format!("{} {tag}", o + 1))
                            .size(look.small)
                            .color(colour),
                    );
                    let (rates, levels) = op_rates(p, o);
                    rates_levels_display(
                        ui,
                        vec2(Size::Small.cell(), 22.0),
                        None,
                        rates,
                        levels,
                        false,
                    );
                    changed |=
                        param_knob(ui, s, p, op_at(o, op::LEVEL), Some("Level"), Size::Small);
                    ui.label(
                        RichText::new(frequency_text(p, o))
                            .font(look.numerals_font_at(look.small))
                            .color(look.c().text_dim),
                    );
                });
            }
        });
    });
    changed
}

/// The operator being edited in full: its frequency on the left, and
/// on the right its envelope or its keyboard scaling and touch.
fn operator(ui: &mut Ui, p: &Params, s: Salt, height: f32, view: &mut View) -> bool {
    let mut changed = false;
    let o = view.op.min(OPS - 1);
    module(
        ui,
        &format!("Operator {}", o + 1),
        vec2(356.0, height),
        |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(2.0 * Size::Small.cell() + 6.0);
                    changed |= param_switch_as(ui, s, p, op_at(o, op::MODE), &["Ratio", "Fixed"]);
                    changed |= knobs(
                        ui,
                        s,
                        p,
                        &[
                            (op_at(o, op::COARSE), "Coarse"),
                            (op_at(o, op::FINE), "Fine"),
                        ],
                    );
                    changed |= knobs(
                        ui,
                        s,
                        p,
                        &[
                            (op_at(o, op::DETUNE), "Detune"),
                            (op_at(o, op::LEVEL), "Level"),
                        ],
                    );
                    ui.label(
                        RichText::new(frequency_text(p, o)).font(
                            theme::look(ui.ctx()).numerals_font_at(theme::look(ui.ctx()).small),
                        ),
                    );
                });
                ui.vertical(|ui| {
                    let pages = ["Envelope", "Scaling and touch"];
                    // ai: view
                    if let Some(k) = Switch::new("Operator page", &pages, usize::from(view.scaling))
                        .small()
                        .show(ui, (s, "op-page"))
                    {
                        view.scaling = k == 1;
                    }
                    if view.scaling {
                        changed |= scaling(ui, p, s, o);
                    } else {
                        let (rates, levels) = op_rates(p, o);
                        rates_levels_display(
                            ui,
                            vec2(4.0 * Size::Small.cell() + 18.0, 24.0),
                            None,
                            rates,
                            levels,
                            false,
                        );
                        let row = |k0: usize, name: &str| -> Vec<(usize, String)> {
                            (0..4)
                                .map(|k| (op_at(o, k0 + k), format!("{name} {}", k + 1)))
                                .collect()
                        };
                        for (k0, name) in [(op::RATE1, "Rate"), (op::LEVEL1, "Level")] {
                            let r = row(k0, name);
                            let r: Vec<(usize, &str)> =
                                r.iter().map(|(i, l)| (*i, l.as_str())).collect();
                            changed |= knobs(ui, s, p, &r);
                        }
                    }
                });
            });
        },
    );
    changed
}

/// Operator `o`'s keyboard scaling (how its level changes either side
/// of a break point) and touch (velocity, the LFO on its level, how its
/// envelope speeds up higher up the keys).
fn scaling(ui: &mut Ui, p: &Params, s: Salt, o: usize) -> bool {
    let mut changed = false;
    changed |= knobs(
        ui,
        s,
        p,
        &[
            (op_at(o, op::BREAK), "Break"),
            (op_at(o, op::LEFT_DEPTH), "Left"),
            (op_at(o, op::RIGHT_DEPTH), "Right"),
            (op_at(o, op::RATE_SCALE), "Rate keys"),
        ],
    );
    ui.horizontal(|ui| {
        ui.label(dim(ui, "Curves"));
        changed |= param_combo(ui, s, p, op_at(o, op::LEFT_CURVE), 76.0);
        changed |= param_combo(ui, s, p, op_at(o, op::RIGHT_CURVE), 76.0);
    });
    changed |= knobs(
        ui,
        s,
        p,
        &[
            (op_at(o, op::VELOCITY), "Velocity"),
            (op_at(o, op::AMP_MOD), "LFO level"),
        ],
    );
    changed
}

// ----------------------------------------------------------------------
// Modulation and playing

fn modulation(ui: &mut Ui, p: &Params, s: Salt, height: f32) -> bool {
    let mut changed = false;
    module(ui, "LFO and pitch envelope", vec2(408.0, height), |ui| {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(3.0 * Size::Small.cell() + 12.0);
                ui.horizontal(|ui| {
                    changed |= param_combo(ui, s, p, at::LFO_WAVE, 96.0);
                    changed |= param_switch(ui, s, p, at::LFO_SYNC);
                });
                changed |= knobs(
                    ui,
                    s,
                    p,
                    &[
                        (at::LFO_SPEED, "Speed"),
                        (at::LFO_DELAY, "Delay"),
                        (at::PITCH_MOD_SENS, "Pitch sens"),
                    ],
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    &[(at::LFO_PITCH, "To pitch"), (at::LFO_AMP, "To level")],
                );
            });
            ui.vertical(|ui| {
                let rates = std::array::from_fn(|k| p.get(at::PITCH_RATE1 + k));
                let levels = std::array::from_fn(|k| p.get(at::PITCH_LEVEL1 + k));
                rates_levels_display(
                    ui,
                    vec2(4.0 * Size::Small.cell() + 18.0, 26.0),
                    Some("Pitch"),
                    rates,
                    levels,
                    true,
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    &[
                        (at::PITCH_RATE1, "Rate 1"),
                        (at::PITCH_RATE1 + 1, "Rate 2"),
                        (at::PITCH_RATE1 + 2, "Rate 3"),
                        (at::PITCH_RATE1 + 3, "Rate 4"),
                    ],
                );
                changed |= knobs(
                    ui,
                    s,
                    p,
                    &[
                        (at::PITCH_LEVEL1, "Level 1"),
                        (at::PITCH_LEVEL1 + 1, "Level 2"),
                        (at::PITCH_LEVEL1 + 2, "Level 3"),
                        (at::PITCH_LEVEL1 + 3, "Level 4"),
                    ],
                );
            });
        });
    });
    changed
}

fn playing(
    ui: &mut Ui,
    p: &Params,
    s: Salt,
    height: f32,
    bank: Option<&lucky_seven::bank::Bank>,
    choose: &mut Option<usize>,
    asked: &mut Asked,
) {
    module(ui, "Playing", vec2(236.0, height), |ui| {
        asked.changed |= param_switch_as(ui, s, p, at::PLAYING, &["Poly", "Mono", "Legato"]);
        asked.changed |= knobs(
            ui,
            s,
            p,
            &[
                (at::VOICES, "Voices"),
                (at::GLIDE, "Glide"),
                (at::BEND, "Bend"),
                (at::WHEEL, "Wheel"),
            ],
        );
        asked.changed |= knobs(
            ui,
            s,
            p,
            &[(at::TRANSPOSE, "Transpose"), (at::LEVEL, "Level")],
        );
        ui.horizontal(|ui| {
            // ai: load-synth-bank
            if ui
                .small_button("Load a bank…")
                .on_hover_text(
                    "Reads your own sounds from a file in the public six-operator FM bank format (.syx)",
                )
                .clicked()
            {
                asked.load_bank = true;
            }
            if let Some(bank) = bank {
                egui::ComboBox::from_id_salt((s, "bank"))
                    .width(110.0)
                    .selected_text(RichText::new("From the bank…").size(theme::look(ui.ctx()).small))
                    .show_ui(ui, |ui| {
                        for (k, v) in bank.voices.iter().enumerate() {
                            // ai: set_instrument
                            if ui.selectable_label(false, &v.name).clicked() {
                                *choose = Some(k);
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
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_algorithm_lays_its_operators_out_apart() {
        for (n, alg) in lucky_seven::ALGORITHM_TABLE.iter().enumerate() {
            let (at, columns) = layout(alg);
            for a in 0..OPS {
                for b in a + 1..OPS {
                    assert!(
                        at[a] != at[b],
                        "algorithm {}: operators {} and {} on one spot",
                        n + 1,
                        a + 1,
                        b + 1
                    );
                }
                assert!(at[a].0 < columns, "algorithm {}", n + 1);
                let heard = alg.carriers & (1 << a) != 0;
                assert_eq!(at[a].1 == 0, heard, "algorithm {} op {}", n + 1, a + 1);
            }
        }
    }

    #[test]
    fn frequencies_read_as_ratios_or_hz() {
        let p = Params::new(lucky_seven::params());
        assert_eq!(frequency_text(&p, 0), "× 1.00");
        p.set(op_at(0, op::COARSE), 0.0);
        assert_eq!(frequency_text(&p, 0), "× 0.50");
        p.set(op_at(0, op::MODE), 1.0);
        p.set(op_at(0, op::COARSE), 2.0);
        assert_eq!(frequency_text(&p, 0), "100.0 Hz");
    }
}

/// The panel driven through the assistant interface and the controls.
#[cfg(test)]
mod e2e {
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
    fn lucky_seven_is_a_three_unit_panel_an_assistant_sees_and_steers() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "lucky-seven"})).unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();
        // All six levels side by side; operator 1 in full, so its level
        // shows twice.
        for o in 1..=6 {
            let n = d.h.query_all_by_label(&format!("Op {o} level")).count();
            assert_eq!(n, if o == 1 { 2 } else { 1 }, "op {o}");
        }
        for label in [
            "Algorithm",
            "Feedback",
            "Op 1 coarse",
            "Op 1 rate 4",
            "Pitch rate 1",
            "Load a bank…",
        ] {
            d.h.get_by_label(label);
        }
        assert!(d.h.query_by_label("Op 3 coarse").is_none());
        // Editing another operator is the view; its settings are the
        // assistant's as ever.
        d.h.get_by_label("Edit operator: 3").click();
        d.h.run();
        d.h.get_by_label("Op 3 coarse");
        d.h.get_by_label("Operator page: Scaling and touch").click();
        d.h.run();
        d.h.get_by_label("Op 3 break point");
        d.h.get_by_label("Op 3 frequency: Fixed").click();
        d.h.run();
        assert_eq!(value(&mut d, &t, "op3_mode"), 1.0);
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.algorithm", "value": 32.0}),
        )
        .unwrap();
        d.h.run();
        d.h.get_by_value("32");
        // The sounds in the header, and the bank button runs its command.
        d.h.get_by_label("Next sound").click();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert!(info["sound"].is_string(), "{info}");
    }
}
