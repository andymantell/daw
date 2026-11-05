// SPDX-License-Identifier: GPL-3.0-or-later
//! The panel kit (UI polish step 4; `docs/plans/ui-polish.md` section
//! 5): the parts every built-in instrument's and effect's front panel is
//! drawn from, so each reads as one piece of gear and all of them as one
//! rack.
//!
//! - [`Knob`]: a rotary control (the mixer's pan pot grown up), with its
//!   label and its value in units under it. [`param_knob`] makes one for
//!   an instrument setting.
//! - [`Switch`]: a row of segments for a small choice (a wave shape, a
//!   play mode), in place of a drop-down. [`param_switch`] makes one for
//!   a setting, falling back to a drop-down for a long list.
//! - [`module`]: a framed block with a heading, laid left to right in
//!   signal order.
//! - [`filter_display`] and [`envelope_display`]: small screens drawn
//!   from the settings, so the filter's response and an envelope's shape
//!   are seen as the knobs turn.
//! - [`header`]: the unit's name plate (its name in the theme's heading
//!   face, in the badge colour), a line on what it is, the sound with
//!   previous, next, save and compare, and room on the right for the
//!   "Notes in" buttons.
//! - [`Rack`]: the hint of a rack: a rail down each side of the panel
//!   with screw marks, and each unit a front plate standing a little
//!   proud of it, at a fixed height in units, or collapsed to a slim
//!   strip, or hidden; with a "Show in rack" menu on the rail.
//!
//! Everything is drawn flat from theme tokens (`rail`, `plate`,
//! `plate_edge`, `plate_shadow`, `screw`, `badge`, and the `[rack]`
//! section): no image textures, no lighting on the knob caps.

use daw_instruments::params::{Params, Unit};
use daw_project::RackView;
use eframe::egui::{
    self, Align, CornerRadius, Id, Layout, Pos2, Rect, Response, RichText, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::character::{
    self, Band, Character, KnobStyle, Lettering, ModuleStyle, ScreenStyle, SwitchStyle,
};
use crate::desk::{asked_value, key_steps, wheel_steps};
use crate::icons::{Icon, IconButton};
use crate::{spoken, theme};

/// A rail's width, each side of the rack.
pub(crate) const RAIL: f32 = 18.0;
/// One rack unit's height, in points.
pub(crate) const UNIT: f32 = 88.0;
/// A collapsed unit's height.
pub(crate) const STRIP: f32 = 34.0;
/// Space between the rail and a plate, and between plates.
const PLATE_GAP: f32 = 4.0;
/// The ears at a plate's corners, where the screws go.
pub(crate) const EAR: f32 = 14.0;

// ----------------------------------------------------------------------
// Knobs

/// A knob's size: the small ones for an envelope's four, the large one
/// for the control that matters most (a filter's cutoff).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Size {
    Small,
    Normal,
    Large,
}

impl Size {
    /// The knob's diameter.
    fn diameter(self) -> f32 {
        match self {
            Size::Small => 26.0,
            Size::Normal => 34.0,
            Size::Large => 54.0,
        }
    }

    /// The width of the knob with its label and readout.
    pub(crate) fn cell(self) -> f32 {
        match self {
            Size::Small => 50.0,
            Size::Normal => 62.0,
            Size::Large => 84.0,
        }
    }

    /// The height of the knob with its label and readout.
    pub(crate) fn height(self) -> f32 {
        self.diameter() + 8.0 + 2.0 * LINE
    }
}

/// A line of small text under a knob.
const LINE: f32 = 13.0;

/// A rotary control. Drag up or right to turn it up, Shift for fine
/// steps; the wheel and the arrow keys step it; a double-click puts it
/// back to its default. A screen reader hears it as a slider.
pub(crate) struct Knob<'a> {
    value: &'a mut f64,
    min: f64,
    max: f64,
    log: bool,
    stepped: bool,
    default: Option<f64>,
    name: String,
    label: String,
    readout: String,
    size: Size,
    /// The arc runs from the middle, for a setting that goes both ways
    /// (a pitch offset, a pan).
    bipolar: bool,
}

impl<'a> Knob<'a> {
    /// A knob over `value` in `min..=max`. `name` is what a screen
    /// reader says; the label under the knob is the name unless
    /// [`Knob::label`] gives a shorter one.
    pub(crate) fn new(value: &'a mut f64, min: f64, max: f64, name: impl Into<String>) -> Self {
        let name = name.into();
        Knob {
            value,
            min,
            max,
            log: false,
            stepped: false,
            default: None,
            label: name.clone(),
            name,
            readout: String::new(),
            size: Size::Normal,
            bipolar: min < 0.0 && max > 0.0,
        }
    }

    /// Turns through the range logarithmically (a frequency, a time).
    pub(crate) fn log(mut self, log: bool) -> Self {
        self.log = log && self.min > 0.0;
        self
    }

    /// Whole numbers only.
    pub(crate) fn stepped(mut self, stepped: bool) -> Self {
        self.stepped = stepped;
        self
    }

    /// Where a double-click puts it.
    pub(crate) fn default(mut self, v: f64) -> Self {
        self.default = Some(v);
        self
    }

    /// The short label under the knob.
    pub(crate) fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// The value in its units, under the label.
    pub(crate) fn readout(mut self, text: impl Into<String>) -> Self {
        self.readout = text.into();
        self
    }

    pub(crate) fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Where `v` is in the range, 0 to 1.
    fn norm(&self, v: f64) -> f64 {
        let k = if self.log {
            (v / self.min).ln() / (self.max / self.min).ln()
        } else if self.max > self.min {
            (v - self.min) / (self.max - self.min)
        } else {
            0.0
        };
        k.clamp(0.0, 1.0)
    }

    fn denorm(&self, k: f64) -> f64 {
        let k = k.clamp(0.0, 1.0);
        let v = if self.log {
            self.min * (self.max / self.min).powf(k)
        } else {
            self.min + (self.max - self.min) * k
        };
        self.fix(v)
    }

    fn fix(&self, v: f64) -> f64 {
        let v = if self.stepped { v.round() } else { v };
        v.clamp(self.min, self.max)
    }

    /// Draws the knob and handles its input. The response is marked
    /// changed when the value moved.
    pub(crate) fn show(
        self,
        ui: &mut Ui,
        salt: impl std::hash::Hash + std::fmt::Debug,
    ) -> Response {
        let size = self.size;
        let cell = vec2(size.cell(), size.height());
        let (_, rect) = ui.allocate_space(cell);
        let id = ui.scope_id().with(salt);
        let d = size.diameter();
        let knob = Rect::from_center_size(pos2(rect.center().x, rect.top() + d / 2.0), vec2(d, d));
        let r = ui.interact(knob, id, Sense::click_and_drag());
        let before = *self.value;
        let fine = ui.input(|i| i.modifiers.shift);
        if r.drag_started() {
            ui.data_mut(|m| m.insert_temp(id, self.norm(*self.value)));
        }
        if r.dragged() {
            let delta = r.drag_delta();
            let mut k = ui
                .data(|m| m.get_temp::<f64>(id))
                .unwrap_or(self.norm(*self.value));
            k += f64::from(delta.x - delta.y) / 160.0 * if fine { 0.1 } else { 1.0 };
            k = k.clamp(0.0, 1.0);
            ui.data_mut(|m| m.insert_temp(id, k));
            *self.value = self.denorm(k);
        }
        if r.double_clicked()
            && let Some(d) = self.default
        {
            *self.value = self.fix(d);
        }
        let steps = key_steps(ui, &r, true) + wheel_steps(ui, &r);
        if steps != 0.0 {
            if self.stepped {
                *self.value = self.fix(*self.value + steps);
            } else {
                let step = if fine { 0.002 } else { 0.02 };
                *self.value = self.denorm(self.norm(*self.value) + steps * step);
            }
        }
        if let Some(v) = asked_value(ui, id) {
            *self.value = self.fix(v);
        }
        let mut r = r;
        if *self.value != before {
            r.mark_changed();
        }
        let value = *self.value;
        let name = self.name.clone();
        r.widget_info(|| egui::WidgetInfo::slider(true, value, &name));
        let tip = if self.readout.is_empty() {
            self.name.clone()
        } else {
            format!("{}: {}", self.name, self.readout)
        };
        let r = r.accessible_name(self.name.clone()).on_hover_text(tip);
        self.paint(ui, rect, knob, r.has_focus() || r.hovered());
        r
    }

    fn paint(&self, ui: &Ui, cell: Rect, knob: Rect, lit: bool) {
        let look = theme::look(ui.ctx());
        let c = look.c();
        let ch = character::current(ui.ctx());
        let accent = ch.accent(&look);
        let p = ui.painter();
        let centre = knob.center();
        let radius = knob.width() / 2.0 - 3.0;
        // Through 270°, from bottom left to bottom right.
        let angle = |k: f32| std::f32::consts::PI * (0.75 + 1.5 * k);
        let at = |a: f32, r: f32| centre + r * Vec2::angled(a);
        let arc = |from: f32, to: f32| -> Vec<Pos2> {
            let n = 24;
            (0..=n)
                .map(|i| at(from + (to - from) * i as f32 / n as f32, radius + 2.0))
                .collect()
        };
        let k = self.norm(*self.value) as f32;
        let weight = if self.size == Size::Large { 2.5 } else { 2.0 };
        let plate = ch.plate(&look);
        match ch.knob {
            KnobStyle::Skirted => {
                // The skirt, with a tick at each tenth of the turn, under
                // a smaller cap.
                p.circle_filled(centre, radius + 3.0, theme::mix(plate, c.plate_shadow, 0.6));
                for i in 0..=10 {
                    let a = angle(i as f32 / 10.0);
                    let long = i % 5 == 0;
                    p.line_segment(
                        [
                            at(a, radius - 1.0),
                            at(a, radius + if long { 3.0 } else { 1.5 }),
                        ],
                        Stroke::new(1.0, theme::mix(c.text_dim, plate, 0.3)),
                    );
                }
            }
            KnobStyle::Flat => {
                p.add(Shape::line(
                    arc(angle(0.0), angle(1.0)),
                    Stroke::new(1.0, theme::mix(c.fader_slot, plate, 0.4)),
                ));
            }
            _ => {
                p.add(Shape::line(
                    arc(angle(0.0), angle(1.0)),
                    Stroke::new(weight, c.fader_slot),
                ));
            }
        }
        let from = if self.bipolar { 0.5 } else { 0.0 };
        if (k - from).abs() > 0.002 && ch.knob != KnobStyle::Skirted {
            p.add(Shape::line(
                arc(angle(from), angle(k)),
                Stroke::new(
                    if ch.knob == KnobStyle::Flat {
                        1.5
                    } else {
                        weight
                    },
                    accent,
                ),
            ));
        }
        let edge = if lit { accent } else { c.text_dim };
        match ch.knob {
            KnobStyle::Cap => {
                p.circle_filled(centre + vec2(0.0, 1.5), radius - 1.0, c.shadow);
                p.circle_filled(centre, radius - 1.0, c.fader_cap);
                p.circle_stroke(centre, radius - 1.0, Stroke::new(1.0, edge));
                p.line_segment(
                    [at(angle(k), 3.0), at(angle(k), radius - 2.5)],
                    Stroke::new(weight, c.fader_line),
                );
            }
            KnobStyle::Skirted => {
                let r = radius - 4.0;
                p.circle_filled(centre + vec2(0.0, 1.0), r, c.shadow);
                p.circle_filled(centre, r, c.fader_cap);
                p.circle_stroke(centre, r, Stroke::new(1.0, edge));
                p.line_segment(
                    [at(angle(k), 1.0), at(angle(k), radius + 1.0)],
                    Stroke::new(weight, if lit { accent } else { c.fader_line }),
                );
            }
            KnobStyle::Pointer => {
                p.circle_filled(centre + vec2(0.0, 1.5), radius - 1.0, c.shadow);
                p.circle_filled(centre, radius - 1.0, c.fader_cap);
                p.circle_stroke(centre, radius - 1.0, Stroke::new(1.0, edge));
                // A wedge from the back of the cap to its rim.
                let a = angle(k);
                let tip = at(a, radius - 1.5);
                let side = 2.4;
                let left = at(a + std::f32::consts::FRAC_PI_2, side)
                    - (radius - 5.0) * Vec2::angled(a) * 0.55;
                let right = at(a - std::f32::consts::FRAC_PI_2, side)
                    - (radius - 5.0) * Vec2::angled(a) * 0.55;
                p.add(Shape::convex_polygon(
                    vec![tip, left, right],
                    c.fader_line,
                    Stroke::NONE,
                ));
            }
            KnobStyle::Dot => {
                p.circle_filled(centre + vec2(0.0, 1.0), radius - 1.0, c.shadow);
                p.circle_filled(
                    centre,
                    radius - 1.0,
                    theme::mix(c.fader_cap, c.plate_shadow, 0.3),
                );
                p.circle_stroke(centre, radius - 1.0, Stroke::new(1.0, edge));
                p.circle_filled(
                    at(angle(k), radius - 4.5),
                    2.2,
                    if lit { accent } else { c.fader_line },
                );
            }
            KnobStyle::Flat => {
                p.circle_stroke(centre, radius - 1.0, Stroke::new(1.0, edge));
                p.line_segment(
                    [at(angle(k), radius * 0.45), at(angle(k), radius - 2.0)],
                    Stroke::new(weight, if lit { accent } else { c.fader_line }),
                );
            }
        }
        if self.bipolar {
            p.circle_filled(at(angle(0.5), radius + 5.5), 1.2, c.text_dim);
        }
        let small = look.font(look.small);
        let y = knob.bottom() + 6.0;
        p.text(
            pos2(cell.center().x, y),
            egui::Align2::CENTER_TOP,
            &self.label,
            small.clone(),
            c.text,
        );
        if !self.readout.is_empty() {
            p.text(
                pos2(cell.center().x, y + LINE),
                egui::Align2::CENTER_TOP,
                &self.readout,
                look.numerals_font_at(look.small),
                c.text_dim,
            );
        }
    }
}

/// A setting's value under its knob: as the setting gives it, but
/// octaves to one place, so two side by side don't run together.
fn readout(info: &daw_instruments::Param, v: f64) -> String {
    match info.unit {
        Unit::Octaves => format!("{v:+.1} oct"),
        _ => info.text(v),
    }
}

/// A knob for setting `i` of `p`, labelled `label` (the setting's name
/// when `None`). True if it moved.
pub(crate) fn param_knob(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    p: &Params,
    i: usize,
    label: Option<&str>,
    size: Size,
) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    let mut v = p.get(i);
    let first = spoken::start(ui);
    // ai: set_parameter
    let r = Knob::new(&mut v, info.min, info.max, info.name.as_str())
        .log(info.log)
        .stepped(info.stepped())
        .default(info.default)
        .label(label.unwrap_or(info.name.as_str()))
        .readout(readout(info, p.get(i)))
        .size(size)
        .show(ui, (salt, i));
    let hear = match info.unit {
        Unit::Db => spoken::Hear::Level(v),
        Unit::Pan => spoken::Hear::Pan(v),
        _ => spoken::steady(v, info.min, info.max),
    };
    spoken::hearable(ui, first, info.text(v), hear);
    if r.changed() {
        p.set(i, v);
    }
    r.changed()
}

// ----------------------------------------------------------------------
// Switches

/// A row of segments, one chosen: a small choice (a wave shape, a play
/// mode) shown whole, in place of a drop-down.
pub(crate) struct Switch<'a> {
    name: String,
    choices: &'a [&'a str],
    chosen: usize,
    small: bool,
}

impl<'a> Switch<'a> {
    pub(crate) fn new(name: impl Into<String>, choices: &'a [&'a str], chosen: usize) -> Self {
        Switch {
            name: name.into(),
            choices,
            chosen,
            small: false,
        }
    }

    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// The width `choices` take as a small switch in `ui`'s font, for
    /// sizing a module around one.
    pub(crate) fn width_of(ui: &Ui, choices: &[&str]) -> f32 {
        let look = theme::look(ui.ctx());
        let font = look.font(look.small);
        (choices.iter())
            .map(|t| {
                ui.fonts_mut(|f| {
                    f.layout_no_wrap(t.to_string(), font.clone(), egui::Color32::WHITE)
                        .size()
                        .x
                }) + 12.0
            })
            .sum()
    }

    /// Draws the switch; the choice clicked, if one was.
    pub(crate) fn show(
        self,
        ui: &mut Ui,
        salt: impl std::hash::Hash + std::fmt::Debug,
    ) -> Option<usize> {
        let look = theme::look(ui.ctx());
        let c = look.c();
        let font = look.font(if self.small { look.small } else { look.body });
        let pad = if self.small { 6.0 } else { 8.0 };
        let h = if self.small { 18.0 } else { 22.0 };
        let widths: Vec<f32> = (self.choices.iter())
            .map(|t| {
                ui.fonts_mut(|f| {
                    f.layout_no_wrap(t.to_string(), font.clone(), c.text)
                        .size()
                        .x
                }) + 2.0 * pad
            })
            .collect();
        let style = character::current(ui.ctx()).switch;
        let gap = match style {
            SwitchStyle::Segments => 0.0,
            SwitchStyle::Lit => 4.0,
            SwitchStyle::Slide => 2.0,
        };
        let lamp = if style == SwitchStyle::Lit { 10.0 } else { 0.0 };
        let total: f32 = widths.iter().sum::<f32>() + (gap + lamp) * self.choices.len() as f32;
        let (_, rect) = ui.allocate_space(vec2(total, h));
        let id = ui.scope_id().with(salt);
        let radius = CornerRadius::same(look.radius.min(6.0) as u8);
        let visuals = ui.visuals().clone();
        let p = ui.painter();
        match style {
            SwitchStyle::Segments => {
                p.rect(
                    rect,
                    radius,
                    visuals.widgets.inactive.bg_fill,
                    Stroke::new(1.0, c.line),
                    StrokeKind::Inside,
                );
            }
            SwitchStyle::Lit => {}
            SwitchStyle::Slide => {
                // The slot the block slides in.
                p.rect(
                    rect,
                    radius,
                    c.fader_slot,
                    Stroke::new(1.0, c.plate_shadow),
                    StrokeKind::Inside,
                );
            }
        }
        let mut picked = None;
        let mut x = rect.left();
        for (k, (text, w)) in self.choices.iter().zip(&widths).enumerate() {
            let w = w + lamp;
            let seg = Rect::from_min_size(pos2(x, rect.top()), vec2(w, h));
            x += w + gap;
            let r = ui.interact(seg, id.with(k), Sense::click());
            let on = k == self.chosen;
            let label = format!("{}: {text}", self.name);
            r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, &label));
            let r = r.accessible_name(label);
            if r.clicked() && !on {
                picked = Some(k);
            }
            let p = ui.painter();
            let colour = match style {
                SwitchStyle::Segments => {
                    if on {
                        p.rect_filled(seg.shrink(1.5), radius, visuals.selection.bg_fill);
                        visuals.selection.stroke.color
                    } else if r.hovered() {
                        p.rect_filled(seg.shrink(1.5), radius, visuals.widgets.hovered.bg_fill);
                        c.text
                    } else {
                        c.text_dim
                    }
                }
                SwitchStyle::Lit => {
                    // A button with a lamp at its left end.
                    let fill = if r.hovered() {
                        visuals.widgets.hovered.bg_fill
                    } else {
                        visuals.widgets.inactive.bg_fill
                    };
                    p.rect(
                        seg,
                        radius,
                        fill,
                        Stroke::new(1.0, c.line),
                        StrokeKind::Inside,
                    );
                    let at = pos2(seg.left() + 7.0, seg.center().y);
                    if on {
                        p.circle_filled(at, 3.5, c.accent.gamma_multiply(0.35));
                        p.circle_filled(at, 2.2, c.accent);
                    } else {
                        p.circle_filled(at, 2.2, c.fader_slot);
                    }
                    if on { c.text } else { c.text_dim }
                }
                SwitchStyle::Slide => {
                    if on {
                        // The block, raised, with a line across it.
                        let block = seg.shrink(2.0);
                        p.rect_filled(block.translate(vec2(0.0, 1.0)), radius, c.shadow);
                        p.rect(
                            block,
                            radius,
                            c.fader_cap,
                            Stroke::new(1.0, c.plate_shadow),
                            StrokeKind::Inside,
                        );
                        p.line_segment(
                            [
                                pos2(block.center().x, block.top() + 3.0),
                                pos2(block.center().x, block.bottom() - 3.0),
                            ],
                            Stroke::new(1.0, c.fader_line),
                        );
                        c.fader_line
                    } else if r.hovered() {
                        c.text
                    } else {
                        c.text_dim
                    }
                }
            };
            let centre = if style == SwitchStyle::Lit {
                pos2(seg.center().x + lamp / 2.0, seg.center().y)
            } else {
                seg.center()
            };
            p.text(
                centre,
                egui::Align2::CENTER_CENTER,
                *text,
                font.clone(),
                colour,
            );
            if style == SwitchStyle::Segments
                && k + 1 < self.choices.len()
                && !on
                && k + 1 != self.chosen
            {
                p.line_segment(
                    [
                        pos2(seg.right(), seg.top() + 4.0),
                        pos2(seg.right(), seg.bottom() - 4.0),
                    ],
                    Stroke::new(1.0, c.line),
                );
            }
        }
        picked
    }
}

/// How many choices a switch shows as segments; more go in a drop-down.
const SEGMENTS: usize = 5;

/// A switch for setting `i` of `p`: segments for a short list or an
/// on/off, a drop-down for a long one. True if it changed.
pub(crate) fn param_switch(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    p: &Params,
    i: usize,
) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    let v = p.get(i);
    let changed = match info.unit {
        Unit::Switch => {
            let on = v >= 0.5;
            // ai: set_parameter
            Switch::new(info.name.as_str(), &["Off", "On"], usize::from(on))
                .small()
                .show(ui, (salt, i))
                .map(|k| p.set(i, k as f64))
        }
        Unit::Count if (info.max - info.min) < SEGMENTS as f64 => {
            let names: Vec<String> = (info.min as i64..=info.max as i64)
                .map(|n| n.to_string())
                .collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let chosen = (v - info.min).round().max(0.0) as usize;
            // ai: set_parameter
            Switch::new(info.name.as_str(), &names, chosen)
                .small()
                .show(ui, (salt, i))
                .map(|k| p.set(i, info.min + k as f64))
        }
        Unit::Choice if info.choices.len() <= SEGMENTS => {
            // ai: set_parameter
            Switch::new(info.name.as_str(), info.choices, v as usize)
                .small()
                .show(ui, (salt, i))
                .map(|k| p.set(i, k as f64))
        }
        Unit::Choice => {
            let mut c = v as usize;
            let mut moved = None;
            egui::ComboBox::from_id_salt(ui.scope_id().with((salt, i)))
                .selected_text(info.text(v))
                .show_ui(ui, |ui| {
                    for (k, name) in info.choices.iter().enumerate() {
                        // ai: set_parameter
                        if ui.selectable_value(&mut c, k, *name).changed() {
                            moved = Some(());
                        }
                    }
                })
                .response
                .accessible_name(info.name.as_str());
            moved.map(|()| p.set(i, c as f64))
        }
        _ => None,
    };
    changed.is_some()
}

/// A switch for choice (or on/off) setting `i` of `p` with shorter
/// `labels` on its segments than the setting's own names, for a panel
/// short of room; a screen reader still hears the setting's name. True
/// if it changed.
pub(crate) fn param_switch_as(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    p: &Params,
    i: usize,
    labels: &[&str],
) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    let chosen = (p.get(i) - info.min).round().max(0.0) as usize;
    // ai: set_parameter
    let picked = Switch::new(info.name.as_str(), labels, chosen)
        .small()
        .show(ui, (salt, i));
    if let Some(k) = picked {
        p.set(i, info.min + k as f64);
    }
    picked.is_some()
}

/// A drop-down `width` wide for choice setting `i` of `p`, for a long
/// list in a narrow module. True if it changed.
pub(crate) fn param_combo(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    p: &Params,
    i: usize,
    width: f32,
) -> bool {
    let Some(info) = p.list().get(i) else {
        return false;
    };
    let v = p.get(i);
    let mut c = v as usize;
    let mut moved = false;
    egui::ComboBox::from_id_salt(ui.scope_id().with((salt, i)))
        .width(width)
        .selected_text(RichText::new(info.text(v)).size(theme::look(ui.ctx()).small))
        .show_ui(ui, |ui| {
            for (k, name) in info.choices.iter().enumerate() {
                // ai: set_parameter
                moved |= ui.selectable_value(&mut c, k, *name).changed();
            }
        })
        .response
        .accessible_name(info.name.as_str());
    if moved {
        p.set(i, c as f64);
    }
    moved
}

// ----------------------------------------------------------------------
// Modules and displays

/// A framed block with a heading, `size` big, holding `add`'s controls.
pub(crate) fn module<R>(
    ui: &mut Ui,
    heading: &str,
    size: Vec2,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let ch = character::current(ui.ctx());
    let (_, rect) = ui.allocate_space(size);
    let radius = CornerRadius::same(look.rack_radius.min(8.0) as u8);
    let p = ui.painter();
    let head = Rect::from_min_size(rect.min + vec2(8.0, 5.0), vec2(rect.width() - 16.0, 16.0));
    let rule = Stroke::new(1.0, theme::mix(c.line, c.plate, 0.3));
    let font = look.font(look.small);
    match ch.module {
        ModuleStyle::Framed => {
            p.rect(
                rect,
                radius,
                theme::mix(c.plate, c.plate_shadow, 0.22),
                Stroke::new(look.rack_line, theme::mix(c.line, c.plate, 0.3)),
                StrokeKind::Inside,
            );
            p.text(
                head.left_center(),
                egui::Align2::LEFT_CENTER,
                heading.to_uppercase(),
                font,
                c.badge,
            );
            p.line_segment(
                [
                    pos2(head.left(), head.bottom() + 1.0),
                    pos2(head.right(), head.bottom() + 1.0),
                ],
                rule,
            );
        }
        ModuleStyle::Lines => {
            // Straight on the plate: the heading, a rule under it, and
            // a thin divider down the right-hand side.
            p.text(
                head.left_center(),
                egui::Align2::LEFT_CENTER,
                heading.to_uppercase(),
                font,
                c.badge,
            );
            let line = Stroke::new(1.0, theme::mix(c.text_dim, c.plate, 0.4));
            p.line_segment(
                [
                    pos2(head.left(), head.bottom() + 1.0),
                    pos2(head.right(), head.bottom() + 1.0),
                ],
                line,
            );
            p.line_segment(
                [
                    pos2(rect.right() - 1.0, rect.top() + 6.0),
                    pos2(rect.right() - 1.0, rect.bottom() - 6.0),
                ],
                line,
            );
        }
        ModuleStyle::Band => {
            // The heading on a filled band across the top of the block.
            p.rect(
                rect,
                radius,
                theme::mix(c.plate, c.plate_edge, 0.25),
                Stroke::new(look.rack_line, theme::mix(c.text_dim, c.plate, 0.5)),
                StrokeKind::Inside,
            );
            let band = Rect::from_min_max(rect.min, pos2(rect.right(), head.bottom() + 4.0));
            p.rect_filled(
                band,
                CornerRadius {
                    nw: radius.nw,
                    ne: radius.ne,
                    sw: 0,
                    se: 0,
                },
                c.on_accent,
            );
            p.text(
                head.left_center(),
                egui::Align2::LEFT_CENTER,
                heading.to_uppercase(),
                font,
                c.accent,
            );
        }
        ModuleStyle::Boxed => {
            // A thin frame in the accent, the heading set into its top
            // edge on a knock-out of the plate.
            let frame = Rect::from_min_max(pos2(rect.left(), rect.top() + 8.0), rect.max);
            p.rect_stroke(
                frame,
                radius,
                Stroke::new(1.0, c.accent),
                StrokeKind::Inside,
            );
            let text = heading.to_uppercase();
            let w = ui.fonts_mut(|f| {
                f.layout_no_wrap(text.clone(), font.clone(), c.accent)
                    .size()
                    .x
            });
            let knock =
                Rect::from_min_size(pos2(rect.left() + 8.0, rect.top()), vec2(w + 8.0, 16.0));
            p.rect_filled(knock, 0, c.plate);
            p.text(
                pos2(knock.left() + 4.0, knock.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                font,
                c.accent,
            );
        }
    }
    let inner = Rect::from_min_max(
        pos2(rect.left() + 8.0, head.bottom() + 6.0),
        rect.max - vec2(8.0, 6.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .id_salt(heading)
            .layout(Layout::top_down(Align::Min)),
    );
    child.spacing_mut().item_spacing = vec2(6.0, 4.0);
    add(&mut child)
}

/// The response a filter display draws: a low-pass of so many poles, a
/// band-pass, a high-pass or a notch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FilterShape {
    Low { poles: u8 },
    Band,
    High,
    Notch,
}

/// A screen `size` big, drawn with `draw` given its rect and painter,
/// over a faint grid of `columns` by `rows`.
pub(crate) fn screen(ui: &mut Ui, size: Vec2, columns: usize, rows: usize) -> (Rect, theme::Look) {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let ch = character::current(ui.ctx());
    let (_, rect) = ui.allocate_space(size);
    let radius = CornerRadius::same(look.rack_radius.min(4.0) as u8);
    let p = ui.painter();
    let ink = screen_ink(&ch, c);
    match ch.screen {
        ScreenStyle::Scope | ScreenStyle::Lamp => {
            p.rect(
                rect,
                radius,
                c.meter_bg,
                Stroke::new(1.0, c.line),
                StrokeKind::Inside,
            );
            let grid = Stroke::new(1.0, theme::mix(c.line, c.meter_bg, 0.5));
            for i in 1..columns {
                let x = rect.left() + rect.width() * i as f32 / columns as f32;
                p.line_segment(
                    [pos2(x, rect.top() + 2.0), pos2(x, rect.bottom() - 2.0)],
                    grid,
                );
            }
            for i in 1..rows {
                let y = rect.top() + rect.height() * i as f32 / rows as f32;
                p.line_segment(
                    [pos2(rect.left() + 2.0, y), pos2(rect.right() - 2.0, y)],
                    grid,
                );
            }
        }
        ScreenStyle::Lcd => {
            // A bezel, pale glass, and a dot grid like unlit pixels.
            p.rect(
                rect,
                radius,
                c.meter_bg,
                Stroke::new(2.0, c.plate_shadow),
                StrokeKind::Inside,
            );
            let dot = theme::mix(ink, c.meter_bg, 0.85);
            let mut y = rect.top() + 4.0;
            while y < rect.bottom() - 2.0 {
                let mut x = rect.left() + 4.0;
                while x < rect.right() - 2.0 {
                    p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(1.0, 1.0)), 0, dot);
                    x += 3.0;
                }
                y += 3.0;
            }
        }
    }
    (rect, look)
}

/// Draws `points` (already in the rect) as a trace, filled beneath.
pub(crate) fn trace(ui: &Ui, rect: Rect, points: Vec<Pos2>, look: &theme::Look) {
    let ch = character::current(ui.ctx());
    let ink = screen_ink(&ch, look.c());
    let p = ui.painter();
    // The fill under the trace, a strip down from each segment: the
    // trace needn't be convex.
    let bottom = rect.bottom() - 2.0;
    let colour = ink.gamma_multiply(if ch.screen == ScreenStyle::Lcd {
        0.25
    } else {
        0.12
    });
    let mut mesh = egui::Mesh::default();
    for w in points.windows(2) {
        let i = mesh.vertices.len() as u32;
        for at in [w[0], w[1], pos2(w[1].x, bottom), pos2(w[0].x, bottom)] {
            mesh.colored_vertex(at, colour);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    p.add(Shape::mesh(mesh));
    if ch.screen == ScreenStyle::Lamp {
        // A glow around the trace.
        p.add(Shape::line(
            points.clone(),
            Stroke::new(5.0, ink.gamma_multiply(0.18)),
        ));
    }
    let weight = if ch.screen == ScreenStyle::Lcd {
        2.0
    } else {
        1.5
    };
    p.add(Shape::line(points, Stroke::new(weight, ink)));
}

/// What a screen draws its trace in: the character's ink, or the
/// theme's accent.
fn screen_ink(ch: &Character, c: &theme::Colours) -> egui::Color32 {
    if ch.plain { c.accent } else { ch.screen_ink }
}

/// A filter's response, 20 Hz to 20 kHz across, −36 to +18 dB up.
pub(crate) fn filter_display(
    ui: &mut Ui,
    size: Vec2,
    cutoff: f64,
    resonance: f64,
    shape: FilterShape,
) {
    let (rect, look) = screen(ui, size, 6, 3);
    let inner = rect.shrink(2.0);
    let n = 64;
    // Q from 0.5 to about 25, as the synth's filter sets it.
    let k = 2.0 - 1.96 * resonance.clamp(0.0, 1.0);
    let points: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let f = 20.0 * 1000f64.powf(t);
            let w = f / cutoff.max(10.0);
            let two_pole = 1.0 / ((1.0 - w * w).powi(2) + (k * w).powi(2)).sqrt();
            let gain = match shape {
                FilterShape::Low { poles } => {
                    // A ladder's extra poles steepen the slope; its
                    // resonance peaks like the two-pole's.
                    let extra = f64::from(poles.saturating_sub(2)) / 2.0;
                    two_pole * (1.0 / (1.0 + w * w).sqrt()).powf(extra)
                }
                FilterShape::Band => two_pole * k * w,
                FilterShape::High => two_pole * w * w,
                FilterShape::Notch => two_pole * (1.0 - w * w).abs(),
            };
            let db = (20.0 * gain.max(1e-6).log10()).clamp(-36.0, 18.0);
            let y = inner.bottom() - inner.height() * ((db + 36.0) / 54.0) as f32;
            pos2(inner.left() + inner.width() * t as f32, y)
        })
        .collect();
    trace(ui, rect, points, &look);
}

/// An envelope's shape: attack, decay and release in seconds (drawn on
/// a log scale, so a short attack still shows), sustain 0 to 1; with a
/// caption in the corner, when given.
pub(crate) fn envelope_display(
    ui: &mut Ui,
    size: Vec2,
    caption: Option<&str>,
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
) {
    let (rect, look) = screen(ui, size, 4, 2);
    let inner = rect.shrink(3.0);
    // Each stage takes a share of the width by its length, 1 ms to 8 s.
    let share =
        |t: f64| ((t.max(0.001) / 0.001).ln() / (8.0f64 / 0.001).ln()).clamp(0.0, 1.0) * 0.8 + 0.2;
    let (a, d, r) = (share(attack), share(decay), share(release));
    let hold = 0.6;
    let total = a + d + hold + r;
    let x = |t: f64| inner.left() + inner.width() * (t / total) as f32;
    let y = |level: f64| inner.bottom() - inner.height() * level as f32;
    let points = vec![
        pos2(inner.left(), inner.bottom()),
        pos2(x(a), y(1.0)),
        pos2(x(a + d), y(sustain)),
        pos2(x(a + d + hold), y(sustain)),
        pos2(inner.right(), inner.bottom()),
    ];
    trace(ui, rect, points, &look);
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

// ----------------------------------------------------------------------
// The header

/// What a unit's header shows.
pub(crate) struct Header<'a> {
    pub name: &'a str,
    pub about: &'a str,
    /// The sound playing, where the unit has sounds to step through.
    pub sound: Option<String>,
    pub has_sounds: bool,
    /// Compare is showing the sound as it was.
    pub comparing: bool,
}

/// What was clicked in a header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeaderAction {
    Previous,
    Next,
    Save,
    Compare,
}

/// The header's height.
pub(crate) const HEADER: f32 = 36.0;

/// The unit's name as its maker's badge, lettered in the character in
/// force, `size` high.
pub(crate) fn name_plate(ui: &mut Ui, name: &str, look: &theme::Look, size: f32) {
    let ch = character::current(ui.ctx());
    let colour = ch.badge(look);
    let (text, font, spacing) = match ch.lettering {
        Lettering::Caps | Lettering::Badge => (name.to_uppercase(), look.font(size), 0.0),
        Lettering::Wide => (name.to_uppercase(), look.font(size * 0.9), size * 0.18),
        Lettering::Lower => (name.to_lowercase(), look.font(size * 1.05), 0.0),
        Lettering::Readout => (
            name.to_uppercase(),
            look.numerals_font_at(size * 0.9),
            size * 0.08,
        ),
    };
    let glyphs: Vec<(String, f32)> = text
        .chars()
        .map(|g| {
            let g = g.to_string();
            let w = ui.fonts_mut(|f| f.layout_no_wrap(g.clone(), font.clone(), colour).size().x);
            (g, w)
        })
        .collect();
    let width: f32 = glyphs.iter().map(|(_, w)| w).sum::<f32>()
        + spacing * glyphs.len().saturating_sub(1) as f32;
    let pad = if ch.lettering == Lettering::Badge {
        7.0
    } else {
        0.0
    };
    let (rect, r) = ui.allocate_exact_size(vec2(width + 2.0 * pad, size + 6.0), Sense::hover());
    // Read as the kit's plain capitals, whatever the lettering, so the
    // plate is told apart from the unit's name in the rack menu.
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, name.to_uppercase()));
    let p = ui.painter();
    let mut ink = colour;
    if ch.lettering == Lettering::Badge {
        p.rect_filled(
            rect,
            CornerRadius::same(look.rack_radius.min(3.0) as u8),
            colour,
        );
        ink = ch.plate(look);
    }
    let mut x = rect.left() + pad;
    for (g, w) in &glyphs {
        p.text(
            pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            g,
            font.clone(),
            ink,
        );
        x += w + spacing;
    }
    if ch.lettering == Lettering::Lower {
        // A rule under the wordmark.
        p.line_segment(
            [
                pos2(rect.left(), rect.bottom() - 1.0),
                pos2(rect.right(), rect.bottom() - 1.0),
            ],
            Stroke::new(1.5, colour),
        );
    }
}

/// A unit's header: the name plate, what it is, the sound buttons, and
/// `right`'s buttons at the right edge.
pub(crate) fn header(ui: &mut Ui, h: Header, right: impl FnOnce(&mut Ui)) -> Option<HeaderAction> {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let mut action = None;
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), HEADER),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_space(4.0);
            name_plate(ui, h.name, &look, look.heading);
            // The line on what it is gives way to the buttons: cut short
            // (the whole of it on hover) when the plate is narrow.
            let reserve = 150.0 + if h.has_sounds { 240.0 } else { 0.0 } + 400.0;
            let room = (ui.available_width() - reserve).max(60.0);
            ui.allocate_ui(vec2(room, HEADER), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(h.about).size(look.small).color(c.text_dim))
                        .truncate(),
                );
            });
            if h.has_sounds {
                ui.add_space(8.0);
                ui.label(RichText::new("Sound").size(look.small).color(c.text_dim));
                // ai: previous-sound
                if ui
                    .add(IconButton::new(Icon::Previous, "Previous sound").small())
                    .clicked()
                {
                    action = Some(HeaderAction::Previous);
                }
                let name = h.sound.clone().unwrap_or_else(|| "Edited".into());
                ui.label(RichText::new(name).strong());
                // ai: next-sound
                if ui
                    .add(IconButton::new(Icon::Next, "Next sound").small())
                    .clicked()
                {
                    action = Some(HeaderAction::Next);
                }
            }
            // ai: save-track-preset
            if ui
                .small_button("Save")
                .on_hover_text("Keep this track's set-up as a preset")
                .clicked()
            {
                action = Some(HeaderAction::Save);
            }
            // ai: compare-sound
            if ui
                .add(egui::Button::new("Compare").small().selected(h.comparing))
                .on_hover_text(
                    "Hear the sound as it was when you started changing it; click again \
                     for your changes",
                )
                .clicked()
            {
                action = Some(HeaderAction::Compare);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                right(ui);
            });
        },
    );
    action
}

// ----------------------------------------------------------------------
// The rack

/// A unit's height in the rack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Height {
    /// So many rack units.
    Units(u8),
    /// As tall as its contents (a unit not yet laid out on the kit).
    Fit,
}

/// A unit the "Show in rack" menu lists.
pub(crate) struct RackEntry {
    /// `instrument`, or an effect's id.
    pub id: String,
    pub name: String,
    pub view: RackView,
}

/// The rack frame: rails down each side of the area given, with the
/// "Show in rack" menu at the top of the left rail, and the units drawn
/// between them by [`Rack::unit`].
pub(crate) struct Rack {
    rails: Rect,
    inner: Rect,
    y: f32,
    /// A half-width unit on the left of the row, its bottom: the next
    /// half-width unit goes beside it.
    left_half: Option<f32>,
    rail_shape: egui::layers::ShapeIdx,
}

/// Paints the two rails down the sides of `rails` into `shape`.
fn paint_rails(ui: &Ui, shape: egui::layers::ShapeIdx, rails: Rect) {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let radius = CornerRadius::same(look.rack_radius.min(4.0) as u8);
    let mut shapes = Vec::new();
    for side in [
        Rect::from_min_size(rails.min, vec2(RAIL, rails.height())),
        Rect::from_min_size(
            pos2(rails.right() - RAIL, rails.top()),
            vec2(RAIL, rails.height()),
        ),
    ] {
        shapes.push(Shape::rect_filled(side, radius, c.rail));
        // Mounting holes down the rail, three to a unit.
        let mut y = side.top() + UNIT / 6.0;
        while y < side.bottom() - 4.0 {
            shapes.push(Shape::circle_filled(
                pos2(side.center().x, y),
                1.6,
                theme::mix(c.rail, c.plate_shadow, 0.7),
            ));
            y += UNIT / 3.0;
        }
    }
    ui.painter().set(shape, Shape::Vec(shapes));
}

impl Rack {
    /// Draws the rails over the area `ui` has, with the menu listing
    /// `units`; a unit whose view the menu changed, with the new view.
    pub(crate) fn new(ui: &mut Ui, units: &[RackEntry]) -> (Rack, Option<(String, RackView)>) {
        let look = theme::look(ui.ctx());
        let c = look.c();
        let rails = ui.available_rect_before_wrap();
        let rail_shape = ui.painter().add(Shape::Noop);
        paint_rails(ui, rail_shape, rails);
        let inner = Rect::from_min_max(
            pos2(rails.left() + RAIL, rails.top()),
            pos2(rails.right() - RAIL, rails.bottom()),
        );
        let mut changed = None;
        // The menu, on the left rail.
        let at = Rect::from_min_size(
            pos2(rails.left() + 1.0, rails.top() + 2.0),
            vec2(RAIL - 2.0, 18.0),
        );
        let mut menu_ui = ui.new_child(egui::UiBuilder::new().max_rect(at).id_salt("rack-menu"));
        // ai: view
        IconButton::new(Icon::More, "Show in rack")
            .small()
            .frame(false)
            .colour(c.screw)
            .tip("Which of this track's units show in the rack, in full or as a strip")
            .menu(&mut menu_ui, |ui| {
                ui.label(RichText::new("Show in rack").strong());
                for u in units {
                    ui.horizontal(|ui| {
                        let mut on = u.view != RackView::Hidden;
                        // ai: set_rack.view
                        if ui.checkbox(&mut on, &u.name).changed() {
                            let view = if on {
                                RackView::Shown
                            } else {
                                RackView::Hidden
                            };
                            changed = Some((u.id.clone(), view));
                        }
                        let mut collapsed = u.view == RackView::Collapsed;
                        // ai: set_rack.view
                        if ui
                            .add_enabled(on, egui::Checkbox::new(&mut collapsed, "as a strip"))
                            .accessible_name(format!("{} as a strip", u.name))
                            .changed()
                        {
                            let view = if collapsed {
                                RackView::Collapsed
                            } else {
                                RackView::Shown
                            };
                            changed = Some((u.id.clone(), view));
                        }
                    });
                }
                if units.iter().any(|u| u.view != RackView::Shown) {
                    // ai: set_rack.view
                    if ui.button("Show all").clicked() {
                        changed = Some(("all".into(), RackView::Shown));
                        ui.close();
                    }
                }
            });
        (
            Rack {
                rails,
                inner,
                y: rails.top() + PLATE_GAP,
                left_half: None,
                rail_shape,
            },
            changed,
        )
    }

    /// One unit's front plate, `height` tall (`Fit` grows to its
    /// contents), holding `add`'s panel; drawn under the one before.
    pub(crate) fn unit<R>(
        &mut self,
        ui: &mut Ui,
        id: Id,
        height: Height,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.unit_of(ui, id, height, false, add)
    }

    /// The width a half-width unit gets in this rack.
    pub(crate) fn half_width(&self) -> f32 {
        (self.inner.width() - 3.0 * PLATE_GAP) / 2.0
    }

    /// A unit as [`Rack::unit`], half the rack's width when `half`: two
    /// half-width units share a row, left then right, as small outboard
    /// does in a real rack.
    pub(crate) fn unit_of<R>(
        &mut self,
        ui: &mut Ui,
        id: Id,
        height: Height,
        half: bool,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        let look = theme::look(ui.ctx());
        let c = look.c();
        // Beside a half-width unit, or under the row.
        let (left, top, width) = match (half, self.left_half) {
            (true, Some(_)) => (
                self.inner.left() + 2.0 * PLATE_GAP + self.half_width(),
                self.y,
                self.half_width(),
            ),
            (true, None) => (self.inner.left() + PLATE_GAP, self.y, self.half_width()),
            (false, _) => {
                self.end_row();
                (
                    self.inner.left() + PLATE_GAP,
                    self.y,
                    self.inner.width() - 2.0 * PLATE_GAP,
                )
            }
        };
        let fixed = match height {
            Height::Units(n) => Some(UNIT * f32::from(n.max(1))),
            Height::Fit => None,
        };
        // The plate is painted once the contents say how tall it is.
        let behind = ui.painter().add(Shape::Noop);
        let surface = ui.painter().add(Shape::Noop);
        let content = Rect::from_min_size(
            pos2(left + EAR, top + 6.0),
            vec2(width - 2.0 * EAR, fixed.map_or(f32::INFINITY, |h| h - 12.0)),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content)
                .id_salt(id)
                .layout(Layout::top_down(Align::Min)),
        );
        if let Some(h) = fixed {
            child.set_min_height(h - 12.0);
            child.set_clip_rect(content.intersect(ui.clip_rect()));
        }
        character::style(&mut child);
        let out = add(&mut child);
        let h = fixed.unwrap_or_else(|| child.min_rect().height() + 12.0);
        let plate = Rect::from_min_size(pos2(left, top), vec2(width, h));
        self.paint_plate(ui, plate, &look, behind, surface);
        // Keep the frame's layout in step: the space the plate took.
        ui.allocate_rect(plate, Sense::hover());
        let _ = c;
        match (half, self.left_half) {
            (true, None) => self.left_half = Some(plate.bottom()),
            (true, Some(other)) => {
                self.left_half = None;
                self.y = plate.bottom().max(other) + PLATE_GAP;
            }
            (false, _) => self.y = plate.bottom() + PLATE_GAP,
        }
        out
    }

    /// Ends a row a half-width unit started alone: what comes next goes
    /// under it.
    pub(crate) fn end_row(&mut self) {
        if let Some(bottom) = self.left_half.take() {
            self.y = bottom + PLATE_GAP;
        }
    }

    /// Runs the rails down past the last unit, where the units went
    /// lower than the area the rack started with (in a scroll area).
    pub(crate) fn finish(&mut self, ui: &mut Ui) {
        self.end_row();
        if self.y > self.rails.bottom() {
            let rails = Rect::from_min_max(self.rails.min, pos2(self.rails.right(), self.y));
            paint_rails(ui, self.rail_shape, rails);
            ui.allocate_rect(rails, Sense::hover());
        }
    }

    /// A line of small text across the rack, between units: what the
    /// units under it are part of (a layer's or a container's chain).
    pub(crate) fn caption(
        &mut self,
        ui: &mut Ui,
        text: &str,
        indent: f32,
        extra: impl FnOnce(&mut Ui),
    ) {
        self.end_row();
        let look = theme::look(ui.ctx());
        let at = Rect::from_min_size(
            pos2(self.inner.left() + PLATE_GAP + indent, self.y),
            vec2(self.inner.width() - 2.0 * PLATE_GAP - indent, 22.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(at)
                .id_salt(("rack-caption", text, indent as i32))
                .layout(Layout::left_to_right(Align::Center)),
        );
        if !text.is_empty() {
            child.label(
                RichText::new(text.to_uppercase())
                    .size(look.small)
                    .color(look.c().text_dim),
            );
        }
        extra(&mut child);
        ui.allocate_rect(at, Sense::hover());
        self.y = at.bottom() + PLATE_GAP;
    }

    /// A unit collapsed to a strip: its name plate, `extra` beside it.
    pub(crate) fn strip(&mut self, ui: &mut Ui, id: Id, name: &str, extra: impl FnOnce(&mut Ui)) {
        self.end_row();
        let look = theme::look(ui.ctx());
        let top = self.y;
        let width = self.inner.width() - 2.0 * PLATE_GAP;
        let behind = ui.painter().add(Shape::Noop);
        let surface = ui.painter().add(Shape::Noop);
        let content = Rect::from_min_size(
            pos2(self.inner.left() + PLATE_GAP + EAR, top),
            vec2(width - 2.0 * EAR, STRIP),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content)
                .id_salt(id)
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 8.0;
        character::style(&mut child);
        child.add_space(4.0);
        name_plate(&mut child, name, &look, look.heading * 0.8);
        extra(&mut child);
        let plate =
            Rect::from_min_size(pos2(self.inner.left() + PLATE_GAP, top), vec2(width, STRIP));
        self.paint_plate(ui, plate, &look, behind, surface);
        ui.allocate_rect(plate, Sense::hover());
        self.y = plate.bottom() + PLATE_GAP;
    }

    /// Paints a front plate into the shapes reserved for it: the plate a
    /// shade apart from the rack, a light along its top edge, a shade
    /// under its bottom edge, ears at its corners with a screw in each.
    fn paint_plate(
        &self,
        ui: &Ui,
        plate: Rect,
        look: &theme::Look,
        behind: egui::layers::ShapeIdx,
        surface: egui::layers::ShapeIdx,
    ) {
        let c = look.c();
        let ch = character::current(ui.ctx());
        let radius = CornerRadius::same(look.rack_radius as u8);
        let mut shapes = Vec::new();
        // The shade under the plate, then the plate.
        shapes.push(Shape::rect_filled(
            plate.translate(vec2(0.0, 2.0)),
            radius,
            c.plate_shadow.gamma_multiply(0.6),
        ));
        shapes.push(Shape::rect_filled(plate, radius, ch.plate(look)));
        shapes.push(Shape::rect_stroke(
            plate,
            radius,
            Stroke::new(look.rack_line, c.plate_shadow),
            StrokeKind::Inside,
        ));
        // The light along the top edge, inside the stroke.
        let inset = look.rack_line + 0.5;
        let r = radius.nw as f32;
        shapes.push(Shape::line_segment(
            [
                pos2(plate.left() + r + inset, plate.top() + inset),
                pos2(plate.right() - r - inset, plate.top() + inset),
            ],
            Stroke::new(1.0, c.plate_edge),
        ));
        ui.painter().set(behind, Shape::Vec(shapes));
        let mut top = Vec::new();
        // The plate's surface, drawn by code.
        let inner = plate.shrink(inset + 1.0);
        match look.plate_surface {
            theme::Plate::Plain => {}
            theme::Plate::Brushed => {
                let line = Stroke::new(1.0, c.plate_edge.gamma_multiply(0.18));
                let mut y = inner.top() + 3.0;
                let mut i = 0u32;
                while y < inner.bottom() {
                    let wobble = (theme::hash(i as i32, 7) % 5) as f32;
                    top.push(Shape::line_segment(
                        [
                            pos2(inner.left() + wobble, y),
                            pos2(inner.right() - wobble, y),
                        ],
                        line,
                    ));
                    y += 3.0;
                    i += 1;
                }
            }
            theme::Plate::Ruled => {
                let line = Stroke::new(1.0, c.plate_shadow.gamma_multiply(0.12));
                let mut x = inner.left() + 12.0;
                while x < inner.right() {
                    top.push(Shape::line_segment(
                        [pos2(x, inner.top()), pos2(x, inner.bottom())],
                        line,
                    ));
                    x += 12.0;
                }
            }
        }
        if let Some(wood) = ch.cheeks {
            // Side cheeks the full height of the plate, at each end.
            for side in [
                Rect::from_min_size(plate.min, vec2(EAR, plate.height())),
                Rect::from_min_size(
                    pos2(plate.right() - EAR, plate.top()),
                    vec2(EAR, plate.height()),
                ),
            ] {
                top.push(Shape::rect_filled(side, radius, wood));
                top.push(Shape::rect_filled(side.shrink2(vec2(0.0, 1.0)), 0, wood));
                // The grain, as a few darker lines down the cheek.
                let line = Stroke::new(1.0, theme::mix(wood, egui::Color32::BLACK, 0.25));
                for dx in [4.0, 7.5, 10.5] {
                    top.push(Shape::line_segment(
                        [
                            pos2(side.left() + dx, side.top() + 2.0),
                            pos2(side.left() + dx, side.bottom() - 2.0),
                        ],
                        line,
                    ));
                }
            }
        }
        band(&mut top, plate, inset, &ch, look);
        // Ears and screws.
        for (x, y) in [
            (plate.left() + EAR / 2.0, plate.top() + EAR / 2.0),
            (plate.right() - EAR / 2.0, plate.top() + EAR / 2.0),
            (plate.left() + EAR / 2.0, plate.bottom() - EAR / 2.0),
            (plate.right() - EAR / 2.0, plate.bottom() - EAR / 2.0),
        ] {
            let at = pos2(x, y);
            top.push(Shape::circle_filled(at, 3.2, c.plate_shadow));
            top.push(Shape::circle_filled(at, 2.6, c.screw));
            // The slot, each at its own turn.
            let turn = theme::hash(x as i32, y as i32) % 180;
            let a = (turn as f32).to_radians();
            let d = 1.9 * Vec2::angled(a);
            top.push(Shape::line_segment(
                [at - d, at + d],
                Stroke::new(1.0, c.plate_shadow),
            ));
        }
        ui.painter().set(surface, Shape::Vec(top));
        let _ = self.rails;
    }
}

/// The character's band along the top of `plate`, under the edge light,
/// between the ears.
fn band(top: &mut Vec<Shape>, plate: Rect, inset: f32, ch: &Character, look: &theme::Look) {
    let accent = ch.accent(look);
    let left = plate.left() + EAR + 2.0;
    let right = plate.right() - EAR - 2.0;
    let y = plate.top() + inset + 2.0;
    match ch.band {
        Band::None => {}
        Band::Stripe => {
            top.push(Shape::rect_filled(
                Rect::from_min_max(pos2(left, y), pos2(right, y + 3.0)),
                CornerRadius::same(1),
                accent.gamma_multiply(0.85),
            ));
        }
        Band::Sections => {
            let n = 4;
            let w = (right - left) / n as f32;
            for (i, colour) in ch.sections().iter().enumerate() {
                let x = left + w * i as f32;
                top.push(Shape::rect_filled(
                    Rect::from_min_max(pos2(x + 1.0, y), pos2(x + w - 1.0, y + 3.0)),
                    CornerRadius::same(1),
                    *colour,
                ));
            }
        }
        Band::Pinstripe => {
            for dy in [0.0, 3.0] {
                top.push(Shape::line_segment(
                    [pos2(left, y + dy), pos2(right, y + dy)],
                    Stroke::new(1.0, accent.gamma_multiply(0.7)),
                ));
            }
        }
        Band::Jacks => {
            let c = look.c();
            let mut x = left + 6.0;
            while x < right - 4.0 {
                let at = pos2(x, y + 1.5);
                top.push(Shape::circle_filled(
                    at,
                    3.0,
                    theme::mix(accent, ch.plate(look), 0.5),
                ));
                top.push(Shape::circle_filled(at, 1.6, c.plate_shadow));
                x += 16.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_knob_maps_its_range_both_ways() {
        let mut v = 2500.0;
        let k = Knob::new(&mut v, 30.0, 18_000.0, "Cutoff").log(true);
        let n = k.norm(2500.0);
        assert!((k.denorm(n) - 2500.0).abs() < 1e-6);
        assert_eq!(k.norm(30.0), 0.0);
        assert_eq!(k.norm(18_000.0), 1.0);
        let mut v = 0.0;
        let k = Knob::new(&mut v, -24.0, 24.0, "Pitch").stepped(true);
        assert_eq!(k.norm(0.0), 0.5);
        assert_eq!(k.denorm(0.52), 1.0);
        assert!(k.bipolar);
    }

    #[test]
    fn switches_and_knobs_draw_and_take_a_click() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let p = std::sync::Arc::new(Params::new(daw_instruments::synth::params()));
        let shown = p.clone();
        let mut h = Harness::new_ui(move |ui| {
            param_switch(ui, "t", &shown, 0);
            param_knob(ui, "t", &shown, 12, Some("Cutoff"), Size::Large);
        });
        h.run();
        h.get_by_label("Oscillator 1: Saw").click();
        h.run();
        assert_eq!(p.get(0), 2.0);
        h.get_by_label("Oscillator 1: Sine").click();
        h.run();
        assert_eq!(p.get(0), 0.0);
        h.get_by_label("Cutoff");
    }
}
