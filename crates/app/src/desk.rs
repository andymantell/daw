// SPDX-License-Identifier: GPL-3.0-or-later
//! The parts of a mixing desk the mixer pane is drawn from (roadmap 2.3,
//! docs/research/MIXER_DESK.md): a long-throw vertical fader with its dB
//! scale and a sticky unity mark, a meter beside it on the same scale
//! with peak hold and a clip light, a pan pot with a centre detent, and
//! the name tag ("scribble strip") at the foot of each strip.
//!
//! Each is a control a screen reader reads as a slider, worked from the
//! keyboard as egui's own are: the arrows step it (Shift for fine
//! steps), and a screen reader's increase, decrease and set value act
//! on it. Colours are the theme's desk tokens.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Id, Key, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    Ui, Vec2, pos2, vec2,
};

use crate::theme;

/// A strip's width, and in narrow mode.
pub(crate) const STRIP_W: f32 = 88.0;
pub(crate) const NARROW_W: f32 = 58.0;

/// The fader's ends, in dB. The bottom is off.
pub(crate) const MIN_DB: f64 = -60.0;
pub(crate) const MAX_DB: f64 = 12.0;

/// The fader law: where along the travel (0 at the bottom, 1 at the top)
/// each level sits, with straight lines in dB between. Most of the throw
/// covers -20 dB to +6 dB, unity is three quarters of the way up, and the
/// bottom fifth covers -20 dB to off, as on a console.
const LAW: [(f64, f64); 7] = [
    (0.0, -60.0),
    (0.08, -40.0),
    (0.2, -20.0),
    (0.4, -10.0),
    (0.75, 0.0),
    (0.88, 6.0),
    (1.0, 12.0),
];

/// How near unity (along the travel) a dragged fader stops on it.
const UNITY_HOLD: f64 = 0.012;

/// The levels printed beside the fader.
const SCALE: [f64; 9] = [12.0, 6.0, 0.0, -5.0, -10.0, -20.0, -30.0, -40.0, -60.0];

/// The fader cap's height; its centre travels the length of the fader.
pub(crate) const CAP_H: f32 = 28.0;
const FADER_W: f32 = 26.0;
const SCALE_W: f32 = 22.0;
const METER_W: f32 = 14.0;

/// How long a meter's peak line holds before falling, in seconds.
const PEAK_HOLD: f64 = 1.5;

/// The level at `p` along the travel, 0 (bottom) to 1 (top).
pub(crate) fn db_at(p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    for w in LAW.windows(2) {
        let ((p0, d0), (p1, d1)) = (w[0], w[1]);
        if p <= p1 {
            return d0 + (d1 - d0) * (p - p0) / (p1 - p0);
        }
    }
    MAX_DB
}

/// Where along the travel the level `db` sits.
pub(crate) fn position_of(db: f64) -> f64 {
    let db = db.clamp(MIN_DB, MAX_DB);
    for w in LAW.windows(2) {
        let ((p0, d0), (p1, d1)) = (w[0], w[1]);
        if db <= d1 {
            return p0 + (p1 - p0) * (db - d0) / (d1 - d0);
        }
    }
    1.0
}

/// The fader's travel in `rect`: the cap's centre at the bottom (0) and
/// the top (1), so the scale and meter beside it line up with it.
fn y_of(rect: Rect, p: f64) -> f32 {
    let bottom = rect.bottom() - CAP_H / 2.0;
    let top = rect.top() + CAP_H / 2.0;
    bottom - (bottom - top) * p as f32
}

/// The scale's words for a level.
fn scale_text(db: f64) -> String {
    if db <= MIN_DB {
        "off".into()
    } else if db > 0.0 {
        format!("+{db:.0}")
    } else {
        format!("{db:.0}")
    }
}

/// Steps asked for from the keyboard and a screen reader this frame: up
/// (and increase) positive. Locks the arrows so they step the control
/// and don't move the focus.
pub(crate) fn key_steps(ui: &Ui, r: &Response, horizontal_too: bool) -> f64 {
    if !r.has_focus() {
        return 0.0;
    }
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            r.id,
            egui::EventFilter {
                vertical_arrows: true,
                horizontal_arrows: horizontal_too,
                ..Default::default()
            },
        )
    });
    ui.input(|i| {
        use egui::accesskit::Action;
        let mut n = i.num_presses(Key::ArrowUp) as f64 - i.num_presses(Key::ArrowDown) as f64;
        if horizontal_too {
            n += i.num_presses(Key::ArrowRight) as f64 - i.num_presses(Key::ArrowLeft) as f64;
        }
        n + i.num_accesskit_action_requests(r.id, Action::Increment) as f64
            - i.num_accesskit_action_requests(r.id, Action::Decrement) as f64
    })
}

/// A value a screen reader asked the control to take.
pub(crate) fn asked_value(ui: &Ui, id: Id) -> Option<f64> {
    ui.input(|i| {
        use egui::accesskit::{Action, ActionData};
        i.accesskit_action_requests(id, Action::SetValue)
            .filter_map(|r| match r.data {
                Some(ActionData::NumericValue(v)) => Some(v),
                _ => None,
            })
            .last()
    })
}

/// Wheel steps over a hovered control (up positive), taken so the pane
/// doesn't scroll as well.
pub(crate) fn wheel_steps(ui: &Ui, r: &Response) -> f64 {
    if !r.hovered() {
        return 0.0;
    }
    let notches: f32 = ui.input(|i| {
        (i.raw.events.iter())
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => Some(delta.y.signum()),
                _ => None,
            })
            .sum()
    });
    if ui.input(|i| i.smooth_scroll_delta.y != 0.0) || notches != 0.0 {
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    notches as f64
}

/// The fader's dB at the end of a drag from `start` along the travel,
/// held at unity when near it, to a tenth of a dB.
fn dragged_to(p: f64) -> f64 {
    let unity = position_of(0.0);
    if (p - unity).abs() < UNITY_HOLD {
        return 0.0;
    }
    (db_at(p) * 10.0).round() / 10.0
}

/// How a fader moved this frame.
pub(crate) struct Moved {
    /// By the pointer (rather than the keyboard), so faders moved with
    /// it may follow by the same amount.
    pub(crate) dragged: bool,
}

/// A long-throw vertical fader for `db`, `travel` points long, named
/// `name` for a screen reader. Dragging moves it from where it is (it
/// doesn't jump to the pointer); Shift moves it finely; the wheel nudges
/// it by half a dB (a tenth with Shift); a double-click puts it back to
/// 0 dB. Draws its scale on the left unless `narrow`.
pub(crate) fn fader(ui: &mut Ui, id: Id, db: &mut f64, travel: f32, name: &str) -> Moved {
    let size = vec2(FADER_W, travel + CAP_H);
    let (_, rect) = ui.allocate_space(size);
    let r = ui.interact(rect, id, Sense::click_and_drag());
    let before = *db;
    let fine = ui.input(|i| i.modifiers.shift);
    let mut dragged = false;
    if r.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, position_of(*db)));
    }
    if r.dragged() {
        let dy = r.drag_delta().y as f64;
        let mut p = ui
            .data(|d| d.get_temp::<f64>(id))
            .unwrap_or(position_of(*db));
        p = (p - dy / travel as f64 * if fine { 0.1 } else { 1.0 }).clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(id, p));
        *db = dragged_to(p);
        dragged = true;
    }
    if r.double_clicked() {
        *db = 0.0;
    }
    let step = if fine { 0.1 } else { 0.5 };
    let steps = key_steps(ui, &r, false) + wheel_steps(ui, &r);
    if steps != 0.0 {
        *db = ((*db + steps * step) * 10.0).round() / 10.0;
    }
    if let Some(v) = asked_value(ui, id) {
        *db = v;
    }
    *db = db.clamp(MIN_DB, MAX_DB);
    let mut r = r;
    if *db != before {
        r.mark_changed();
    }
    let value = *db;
    r.widget_info(|| egui::WidgetInfo::slider(true, value, name));
    let r = r
        .accessible_name(name)
        .on_hover_text("Drag to set; Shift for fine steps; double-click for 0 dB");
    paint_fader(ui, rect, *db, r.has_focus() || r.hovered());
    let _ = r;
    Moved { dragged }
}

fn paint_fader(ui: &Ui, rect: Rect, db: f64, lit: bool) {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let p = ui.painter();
    let cx = rect.center().x;
    // The slot.
    let slot = Rect::from_min_max(
        pos2(cx - 2.0, y_of(rect, 1.0) - 2.0),
        pos2(cx + 2.0, y_of(rect, 0.0) + 2.0),
    );
    p.rect_filled(slot, 2.0, c.fader_slot);
    // Ticks either side of it, unity longer.
    for &d in &SCALE {
        let y = y_of(rect, position_of(d));
        let reach = if d == 0.0 { 11.0 } else { 6.0 };
        let stroke = Stroke::new(if d == 0.0 { 1.5 } else { 1.0 }, c.text_dim);
        p.line_segment([pos2(cx - 4.0 - reach, y), pos2(cx - 4.0, y)], stroke);
        p.line_segment([pos2(cx + 4.0, y), pos2(cx + 4.0 + reach, y)], stroke);
    }
    // The cap, with a shadow under it, grip lines and the line across
    // it that reads against the scale.
    let y = y_of(rect, position_of(db));
    let cap = Rect::from_center_size(pos2(cx, y), vec2(FADER_W - 2.0, CAP_H - 4.0));
    let radius = CornerRadius::same(look.radius.min(4.0) as u8);
    p.rect_filled(cap.translate(vec2(0.0, 2.0)), radius, c.shadow);
    p.rect_filled(cap, radius, c.fader_cap);
    let edge = if lit { c.accent } else { c.text_dim };
    p.rect_stroke(cap, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
    for dy in [-7.0, -4.0, 4.0, 7.0] {
        p.line_segment(
            [
                pos2(cap.left() + 5.0, y + dy),
                pos2(cap.right() - 5.0, y + dy),
            ],
            Stroke::new(1.0, c.line),
        );
    }
    p.line_segment(
        [pos2(cap.left() + 2.0, y), pos2(cap.right() - 2.0, y)],
        Stroke::new(2.0, c.fader_line),
    );
}

/// The dB scale to the left of a fader `travel` long: every mark, or
/// with `narrow` only unity.
pub(crate) fn scale(ui: &mut Ui, travel: f32, narrow: bool) {
    let w = if narrow { 10.0 } else { SCALE_W };
    let (_, rect) = ui.allocate_space(vec2(w, travel + CAP_H));
    let look = theme::look(ui.ctx());
    let c = look.c();
    let font = FontId::proportional(look.small.min(11.0));
    // Top down, leaving out a label too near the one above it (the low
    // end of a short fader).
    let mut last = f32::NEG_INFINITY;
    for &d in &SCALE {
        if narrow && d != 0.0 {
            continue;
        }
        let y = y_of(rect, position_of(d));
        if y - last < 11.0 && d != MIN_DB {
            continue;
        }
        last = y;
        let text = if narrow { "0".into() } else { scale_text(d) };
        ui.painter().text(
            pos2(rect.right() - 1.0, y),
            egui::Align2::RIGHT_CENTER,
            text,
            font.clone(),
            if d == 0.0 { c.text } else { c.text_dim },
        );
    }
}

/// What a meter beside a fader shows: the peak and RMS of each side, as
/// linear levels (1.0 is 0 dBFS).
#[derive(Clone, Copy)]
pub(crate) struct Reading {
    pub(crate) peak: (f32, f32),
    pub(crate) rms: (f32, f32),
}

/// What a meter keeps from frame to frame.
#[derive(Clone, Copy, Default)]
struct Held {
    /// Each side's held peak, and when it was reached.
    peak: [(f32, f64); 2],
    clipped: bool,
}

/// What was clicked on a meter.
pub(crate) struct Clicked {
    /// The meter itself: open the meters window on it.
    pub(crate) meter: Response,
}

/// A stereo meter `travel` long beside a fader, on the fader's scale so
/// the two read together: RMS solid, peak paler above it, a peak line
/// that holds a moment, and a clip light at the top that stays lit until
/// clicked. `name` is the strip's, for the clip light.
pub(crate) fn meter(ui: &mut Ui, id: Id, travel: f32, m: Reading, name: &str) -> Clicked {
    let (_, rect) = ui.allocate_space(vec2(METER_W, travel + CAP_H));
    let now = ui.input(|i| i.time);
    let mut held: Held = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let look = theme::look(ui.ctx());
    let c = look.c().clone();
    let level = |v: f32| 20.0 * (v.max(1e-6) as f64).log10();
    let colour = |db: f64| {
        if db > -0.5 {
            c.meter_high
        } else if db > -12.0 {
            c.meter_mid
        } else {
            c.meter_low
        }
    };
    let body = Rect::from_min_max(
        pos2(rect.left(), y_of(rect, 1.0)),
        pos2(rect.right(), y_of(rect, 0.0)),
    );
    let meter = ui
        .interact(body, id.with("meter"), Sense::click())
        .on_hover_text("Peak and RMS. Click for the meters");
    let meter_name = format!("{name} meter");
    meter.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &meter_name));
    let meter = meter.accessible_name(meter_name.clone());
    let p = ui.painter();
    p.rect_filled(body, 2.0, c.meter_bg);
    let bar_w = (METER_W - 3.0) / 2.0;
    let bottom = body.bottom();
    for (side, (peak, rms)) in [(m.peak.0, m.rms.0), (m.peak.1, m.rms.1)]
        .into_iter()
        .enumerate()
    {
        let x = body.left() + 1.0 + side as f32 * (bar_w + 1.0);
        let top = |v: f32| y_of(rect, position_of(level(v)));
        let bar = |v: f32| Rect::from_min_max(pos2(x, top(v)), pos2(x + bar_w, bottom));
        let pk = level(peak);
        if pk > MIN_DB {
            p.rect_filled(bar(peak), 1.0, colour(pk).gamma_multiply(0.45));
            p.rect_filled(bar(rms), 1.0, colour(pk));
        }
        let (h, at) = &mut held.peak[side];
        if peak >= *h || now - *at > PEAK_HOLD {
            *h = peak;
            *at = now;
        }
        let hd = level(*h);
        if hd > MIN_DB {
            let y = top(*h);
            p.line_segment(
                [pos2(x, y), pos2(x + bar_w, y)],
                Stroke::new(2.0, colour(hd)),
            );
        }
        if peak >= 1.0 {
            held.clipped = true;
        }
    }
    // The clip light, in the space above the scale.
    let light = Rect::from_min_size(
        pos2(rect.left(), rect.top() + 1.0),
        vec2(METER_W, CAP_H / 2.0 - 4.0),
    );
    let clip = ui.interact(light, id.with("clip"), Sense::click());
    let clip_name = format!("Clear the clip light on {name}");
    let lit = held.clipped;
    clip.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, lit, &clip_name));
    let clip = clip
        .accessible_name(clip_name.clone())
        .on_hover_text(if lit {
            "It went over 0 dB. Click to clear"
        } else {
            "Lights if it goes over 0 dB"
        });
    if clip.clicked() {
        held.clipped = false;
    }
    let p = ui.painter();
    p.rect_filled(
        light,
        2.0,
        if held.clipped {
            c.meter_high
        } else {
            c.meter_bg
        },
    );
    p.rect_stroke(light, 2.0, Stroke::new(1.0, c.line), StrokeKind::Inside);
    ui.data_mut(|d| d.insert_temp(id, held));
    if m.peak.0 > 0.0 || m.peak.1 > 0.0 {
        // Let the held peak fall when the music stops.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    Clicked { meter }
}

/// A pan pot: drag up or right to pan right, Shift for fine steps, a
/// double-click (or a pass through the middle) for centre.
pub(crate) fn pan_pot(ui: &mut Ui, id: Id, pan: &mut f32, name: &str, size: f32) -> Response {
    let (_, rect) = ui.allocate_space(vec2(size, size));
    let r = ui.interact(rect, id, Sense::click_and_drag());
    let before = *pan;
    let fine = ui.input(|i| i.modifiers.shift);
    if r.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, *pan));
    }
    if r.dragged() {
        let d = r.drag_delta();
        let mut v = ui.data(|m| m.get_temp::<f32>(id)).unwrap_or(*pan);
        v = (v + (d.x - d.y) / 120.0 * if fine { 0.1 } else { 1.0 }).clamp(-1.0, 1.0);
        ui.data_mut(|m| m.insert_temp(id, v));
        // The centre detent.
        *pan = if v.abs() < 0.03 { 0.0 } else { v };
    }
    if r.double_clicked() {
        *pan = 0.0;
    }
    let step = if fine { 0.01 } else { 0.05 };
    let steps = key_steps(ui, &r, true) + wheel_steps(ui, &r);
    if steps != 0.0 {
        *pan = ((*pan as f64 + steps * step) * 100.0).round() as f32 / 100.0;
    }
    if let Some(v) = asked_value(ui, id) {
        *pan = v as f32;
    }
    *pan = pan.clamp(-1.0, 1.0);
    let mut r = r;
    if *pan != before {
        r.mark_changed();
    }
    let value = *pan as f64;
    r.widget_info(|| egui::WidgetInfo::slider(true, value, name));
    let r = r
        .accessible_name(name)
        .on_hover_text("Pan. Drag up or right; double-click for centre");
    paint_pot(ui, rect, *pan, r.has_focus() || r.hovered());
    r
}

fn paint_pot(ui: &Ui, rect: Rect, pan: f32, lit: bool) {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let p = ui.painter();
    let centre = rect.center();
    let radius = rect.width() / 2.0 - 3.0;
    // The knob turns through 270°, from bottom left to bottom right.
    let angle = |v: f32| std::f32::consts::PI * (1.5 + 0.75 * v);
    let at = |a: f32, r: f32| centre + r * Vec2::angled(a - std::f32::consts::FRAC_PI_2 * 2.0);
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = 16;
        (0..=n)
            .map(|i| at(from + (to - from) * i as f32 / n as f32, radius + 2.0))
            .collect()
    };
    p.add(egui::Shape::line(
        arc(angle(-1.0), angle(1.0)),
        Stroke::new(2.0, c.fader_slot),
    ));
    if pan != 0.0 {
        p.add(egui::Shape::line(
            arc(angle(0.0), angle(pan)),
            Stroke::new(2.0, c.accent),
        ));
    }
    p.circle_filled(centre + vec2(0.0, 1.5), radius - 1.0, c.shadow);
    p.circle_filled(centre, radius - 1.0, c.fader_cap);
    let edge = if lit { c.accent } else { c.text_dim };
    p.circle_stroke(centre, radius - 1.0, Stroke::new(1.0, edge));
    p.line_segment(
        [at(angle(pan), 2.0), at(angle(pan), radius - 2.0)],
        Stroke::new(2.0, c.fader_line),
    );
    // The centre mark above the knob.
    p.circle_filled(at(angle(0.0), radius + 5.0), 1.2, c.text_dim);
}

/// A pan in a few letters: "C", "L30", "R100".
pub(crate) fn pan_short(p: f32) -> String {
    let pct = (p.abs() * 100.0).round();
    if pct < 1.0 {
        "C".into()
    } else if p < 0.0 {
        format!("L{pct:.0}")
    } else {
        format!("R{pct:.0}")
    }
}

/// The name tag at the foot of a strip: the name on the theme's
/// scribble colour, under a band of the track's colour. Click to select
/// the strip (Ctrl or Shift adds to the selection), so its fader moves
/// with the others selected.
pub(crate) fn scribble(
    ui: &mut Ui,
    id: Id,
    width: f32,
    name: &str,
    colour: Color32,
    selected: Option<bool>,
) -> Response {
    let (_, rect) = ui.allocate_space(vec2(width, 24.0));
    let r = match selected {
        Some(on) => {
            let r = ui.interact(rect, id, Sense::click());
            let label = format!("Select {name}");
            r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &label));
            r.accessible_name(label.clone())
                .on_hover_text("Click to select; Ctrl-click to add. Selected faders move together")
        }
        // A tag only, for a strip that can't be selected.
        None => ui.interact(rect, id, Sense::hover()),
    };
    let selected = selected.unwrap_or(false);
    let look = theme::look(ui.ctx());
    let c = look.c();
    let p = ui.painter();
    let radius = CornerRadius::same(look.radius.min(3.0) as u8);
    p.rect_filled(rect, radius, c.scribble);
    let band = Rect::from_min_size(rect.min, vec2(rect.width(), 3.0));
    p.rect_filled(
        band,
        CornerRadius {
            nw: radius.nw,
            ne: radius.ne,
            sw: 0,
            se: 0,
        },
        colour,
    );
    let font = FontId::proportional(look.small.max(11.0));
    let shown = elide(ui, name, &font, rect.width() - 6.0);
    p.text(
        rect.center() + vec2(0.0, 1.5),
        egui::Align2::CENTER_CENTER,
        shown,
        font,
        c.scribble_text,
    );
    if selected || r.has_focus() {
        p.rect_stroke(rect, radius, Stroke::new(2.0, c.accent), StrokeKind::Inside);
    }
    r
}

/// `text` cut to fit `width`, with an ellipsis if cut.
fn elide(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    let fits = |s: &str| {
        ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE))
            .size()
            .x
            <= width
    };
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for n in (1..chars.len()).rev() {
        let s: String = chars[..n].iter().collect::<String>() + "…";
        if fits(&s) {
            return s;
        }
    }
    "…".into()
}

/// The strip's body behind its controls.
pub(crate) fn paint_strip(ui: &Ui, rect: Rect) {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let radius = CornerRadius::same(look.radius.min(6.0) as u8);
    ui.painter().rect_filled(rect, radius, c.desk);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(look.line, c.line),
        StrokeKind::Inside,
    );
}

/// The shortest fader travel before a strip's routing goes compact.
pub(crate) const MIN_TRAVEL: f32 = 110.0;

/// The fader's travel on a strip `height` tall whose other parts take
/// `fixed`: what is left, so a full-size desk has long faders, within
/// reason. Less than that, and the desk scrolls.
pub(crate) fn travel(height: f32, fixed: f32) -> f32 {
    (height - fixed).clamp(80.0, 760.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_law_puts_most_of_the_travel_where_it_matters() {
        assert_eq!(db_at(0.0), MIN_DB);
        assert_eq!(db_at(1.0), MAX_DB);
        assert_eq!(db_at(0.75), 0.0, "unity three quarters up");
        let span = position_of(6.0) - position_of(-20.0);
        assert!(span > 0.6, "-20 dB to +6 dB is most of the throw: {span}");
        assert!(position_of(-20.0) <= 0.2, "the bottom fifth goes to off");
        for i in 0..=100 {
            let p = i as f64 / 100.0;
            assert!(
                (position_of(db_at(p)) - p).abs() < 1e-9,
                "round trip at {p}"
            );
        }
        for w in (0..=100).collect::<Vec<_>>().windows(2) {
            assert!(db_at(w[1] as f64 / 100.0) > db_at(w[0] as f64 / 100.0));
        }
    }

    #[test]
    fn a_drag_near_unity_stops_on_it() {
        let u = position_of(0.0);
        assert_eq!(dragged_to(u + 0.005), 0.0);
        assert_eq!(dragged_to(u - 0.005), 0.0);
        assert!(dragged_to(u + 0.05) > 0.5);
        assert_eq!(dragged_to(0.0), MIN_DB);
    }

    #[test]
    fn the_scale_and_pan_read_as_a_desk_prints_them() {
        assert_eq!(scale_text(6.0), "+6");
        assert_eq!(scale_text(0.0), "0");
        assert_eq!(scale_text(-20.0), "-20");
        assert_eq!(scale_text(-60.0), "off");
        assert_eq!(pan_short(0.0), "C");
        assert_eq!(pan_short(-0.3), "L30");
        assert_eq!(pan_short(1.0), "R100");
    }
}
