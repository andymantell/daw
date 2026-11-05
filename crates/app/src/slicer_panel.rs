// SPDX-License-Identifier: GPL-3.0-or-later
//! The loop slicer's front panel (UI polish step 5), on the panel kit.
//!
//! Three modules, left to right in the order a loop is worked on: the
//! loop itself (its file, its shape drawn with a line where each slice
//! starts, and cutting it again at the hits or into equal parts), the
//! slice shown (its pitch, pan, filter and whether it plays backwards),
//! and how the slices play (the note mode, the shared envelope and
//! level, the loop's length in beats and the notes that play it).
//!
//! The settings are the slicer's, so an assistant reaches each through
//! `set_parameter`; the loop and the cutting through `edit_instrument`
//! (`loop`, `slice`), and the notes through the `slice-notes` command.

use daw_instruments::params::note_name;
use daw_instruments::slicer::{self, at, slice};
use eframe::egui::{self, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::commands::Cmd;
use crate::icons::{Icon, IconButton};
use crate::instrument::{Ctx, Shown, Slot, read_sound, sound_menu};
use crate::panel_kit::{HEADER, Knob, Size, UNIT, module, param_knob, param_switch, screen};
use crate::theme;

/// The modules' height in a three-unit plate, below its header.
pub(crate) const BODY: f32 = 3.0 * UNIT - 12.0 - HEADER - 2.0;

/// The width the slice and playing modules take; the loop has the rest.
const SLICE_W: f32 = 236.0;
const PLAYING_W: f32 = 300.0;
/// The loop module's narrowest; the panel scrolls sideways below it.
const LOOP_MIN: f32 = 420.0;

/// The slicer's panel in `slot`, `height` tall. True if the loop was cut
/// again, so the instrument needs making anew.
pub(crate) fn panel(
    ui: &mut Ui,
    slot: &mut Slot,
    cx: &Ctx,
    shown: &mut Shown,
    height: f32,
) -> bool {
    let id = slot.id;
    let mut renew = false;
    let mut sensitivity = slot.sensitivity;
    let mut parts = slot.parts;
    let mut chosen = slot.zone;
    let Some(b) = slot.builtin_mut() else {
        return false;
    };
    let p = b.params().clone();
    let spacing = 8.0;
    let loop_w = (ui.available_width() - SLICE_W - PLAYING_W - 2.0 * spacing).max(LOOP_MIN);
    egui::ScrollArea::horizontal()
        .id_salt(("slicer-panel", id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = spacing;
                module(ui, "Loop", vec2(loop_w, height), |ui| {
                    ui.horizontal(|ui| {
                        let current = b
                            .sample("loop")
                            .map(|s| s.reference.clone())
                            .unwrap_or_default();
                        if let Some(r) = sound_menu(ui, ("slicer-loop", id), &current)
                            && let Some(l) = read_sound(&r, cx, shown)
                        {
                            b.set_loop(std::sync::Arc::new(l), sensitivity);
                            renew = true;
                        }
                        let n = b.slices().len();
                        if b.loop_sample().is_some() {
                            ui.label(dim(
                                ui,
                                &format!(
                                    "{n} slice{}, on the keys from {} up",
                                    if n == 1 { "" } else { "s" },
                                    note_name(slicer::FIRST_NOTE as i32)
                                ),
                            ));
                        }
                    });
                    let knobs = Size::Small.height();
                    let wave = (ui.available_height() - knobs - 8.0).max(40.0);
                    if let Some(k) = loop_display(ui, b, vec2(ui.available_width(), wave), chosen) {
                        chosen = k;
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        renew |= cut(ui, id, &mut sensitivity, &mut parts, b);
                    });
                });
                let n = b.slices().len().max(1);
                chosen = chosen.min(n - 1);
                module(ui, "Slice", vec2(SLICE_W, height), |ui| {
                    slice_picker(ui, id, &mut chosen, n);
                    let k = |s| slicer::slice_at(chosen, s);
                    ui.horizontal(|ui| {
                        for (i, label) in [(slice::TUNE, "Tune"), (slice::PAN, "Pan")] {
                            param_knob(ui, ("slice", id), &p, k(i), Some(label), Size::Small);
                        }
                        param_knob(
                            ui,
                            ("slice", id),
                            &p,
                            k(slice::CUTOFF),
                            Some("Low-pass"),
                            Size::Normal,
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(dim(ui, "Backwards"));
                        param_switch(ui, ("slice", id), &p, k(slice::REVERSE));
                    });
                });
                module(ui, "Playing", vec2(PLAYING_W, height), |ui| {
                    param_switch(ui, ("slicer", id), &p, at::MODE);
                    ui.horizontal(|ui| {
                        for (i, label) in [
                            (at::TUNE, "Tune"),
                            (at::ATTACK, "Attack"),
                            (at::RELEASE, "Release"),
                            (at::VELOCITY, "Velocity"),
                            (at::LEVEL, "Level"),
                        ] {
                            param_knob(ui, ("slicer", id), &p, i, Some(label), Size::Small);
                        }
                    });
                    ui.horizontal(|ui| {
                        param_knob(
                            ui,
                            ("slicer", id),
                            &p,
                            at::BEATS,
                            Some("Beats"),
                            Size::Small,
                        );
                        ui.vertical(|ui| {
                            ui.add_space(6.0);
                            if ui
                                // ai: slice-notes
                                .button("Make notes that play the loop")
                                .on_hover_text(
                                    "A note clip playing the slices in order on their beats, \
                                     so the loop follows the song's tempo",
                                )
                                .clicked()
                            {
                                shown.cmd = Some(Cmd::SliceNotes);
                            }
                            ui.label(dim(ui, "The loop is this many beats long"));
                        });
                    });
                });
            });
        });
    slot.sensitivity = sensitivity;
    slot.parts = parts;
    slot.zone = chosen;
    renew
}

fn dim(ui: &Ui, text: &str) -> RichText {
    let look = theme::look(ui.ctx());
    RichText::new(text)
        .size(look.small)
        .color(look.c().text_dim)
}

/// The loop's shape on a screen, with a line and its key where each
/// slice starts, the slice shown picked out. The slice clicked in, if
/// one was.
fn loop_display(
    ui: &mut Ui,
    b: &daw_instruments::Builtin,
    size: egui::Vec2,
    chosen: usize,
) -> Option<usize> {
    let (rect, look) = screen(ui, size, 8, 2);
    let c = look.c();
    let Some(s) = b.loop_sample() else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No loop yet: choose a sound file above",
            look.font(look.small),
            c.text_dim,
        );
        return None;
    };
    let slices = b.slices();
    let n = s.len().max(1);
    let inner = rect.shrink(2.0);
    let x_of = |f: usize| inner.left() + f as f32 / n as f32 * inner.width();
    let p = ui.painter_at(rect);
    // The slice shown, shaded.
    let start = slices.get(chosen).copied().unwrap_or(0);
    let end = slices.get(chosen + 1).copied().unwrap_or(n);
    p.rect_filled(
        Rect::from_x_y_ranges(x_of(start)..=x_of(end), inner.y_range()),
        0.0,
        c.accent.gamma_multiply(0.14),
    );
    let w = inner.width().max(1.0) as usize;
    let mid = inner.center().y;
    for x in 0..w {
        let a = x * n / w;
        let z = ((x + 1) * n / w).max(a + 1).min(n);
        let peak = (a..z).fold(0.0f32, |m, i| m.max(s.left[i].abs()).max(s.right[i].abs()));
        let h = peak.min(1.0) * inner.height() * 0.46;
        let px = inner.left() + x as f32 + 0.5;
        p.line_segment(
            [pos2(px, mid - h), pos2(px, mid + h)],
            Stroke::new(1.0, c.accent.gamma_multiply(0.75)),
        );
    }
    for (k, &f) in slices.iter().enumerate() {
        let x = x_of(f);
        let on = k == chosen;
        p.line_segment(
            [pos2(x, inner.top()), pos2(x, inner.bottom())],
            Stroke::new(
                if on { 2.0 } else { 1.0 },
                if on { c.text } else { c.text_dim },
            ),
        );
        // Keys only where there's room for them.
        let next = slices.get(k + 1).map_or(inner.right(), |&g| x_of(g));
        if next - x > 22.0 || on {
            p.text(
                pos2(x + 3.0, inner.top() + 1.0),
                egui::Align2::LEFT_TOP,
                note_name(slicer::FIRST_NOTE as i32 + k as i32),
                look.numerals_font_at(look.small * 0.9),
                if on { c.text } else { c.text_dim },
            );
        }
    }
    // Clicking shows the slice clicked in.
    let r = ui
        .interact(rect, ui.scope_id().with("slicer-wave"), Sense::click())
        .on_hover_text("Click a slice to show its settings");
    if r.clicked()
        && let Some(at) = r.interact_pointer_pos()
    {
        let f = ((at.x - inner.left()) / inner.width().max(1.0) * n as f32).max(0.0) as usize;
        return Some(slices.iter().rposition(|&x| x <= f).unwrap_or(0));
    }
    None
}

/// Cutting the loop again: at its hits, how keenly; or into so many
/// equal parts. True if it was cut.
fn cut(
    ui: &mut Ui,
    id: u64,
    sensitivity: &mut f32,
    parts: &mut usize,
    b: &mut daw_instruments::Builtin,
) -> bool {
    let mut cut = false;
    let mut keen = f64::from(*sensitivity);
    let shown = format!("{:.0}%", keen * 100.0);
    // ai: edit_instrument.slice.at_hits
    Knob::new(&mut keen, 0.0, 1.0, "Sensitivity")
        .default(0.5)
        .readout(shown)
        .size(Size::Small)
        .show(ui, ("slicer-sensitivity", id))
        .on_hover_text("Higher finds quieter hits too");
    *sensitivity = keen as f32;
    ui.vertical(|ui| {
        ui.add_space(8.0);
        // ai: edit_instrument.slice.at_hits
        if ui
            .button("Cut at the hits")
            .on_hover_text("A slice where each hit starts")
            .clicked()
        {
            b.slice_at_hits(*sensitivity);
            cut = true;
        }
    });
    ui.add_space(10.0);
    let mut count = *parts as f64;
    let shown = format!("{count:.0}");
    // ai: edit_instrument.slice.parts
    Knob::new(&mut count, 1.0, slicer::MAX_SLICES as f64, "Parts")
        .stepped(true)
        .default(8.0)
        .readout(shown)
        .size(Size::Small)
        .show(ui, ("slicer-parts", id))
        .on_hover_text("How many equal parts to cut the loop into");
    *parts = (count.round() as usize).clamp(1, slicer::MAX_SLICES);
    ui.vertical(|ui| {
        ui.add_space(8.0);
        // ai: edit_instrument.slice.parts
        if ui.button("Cut into equal parts").clicked() {
            b.slice_equally(*parts);
            cut = true;
        }
    });
    cut
}

/// Which slice the module shows: previous and next, and the list.
fn slice_picker(ui: &mut Ui, id: u64, chosen: &mut usize, n: usize) {
    let key = |k: usize| note_name(slicer::FIRST_NOTE as i32 + k as i32);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        // ai: view
        let back = IconButton::new(Icon::Previous, "Previous slice").small();
        if ui.add_enabled(*chosen > 0, back).clicked() {
            *chosen -= 1;
        }
        egui::ComboBox::from_id_salt(("slicer-slice", id))
            .selected_text(format!("Slice {} ({})", *chosen + 1, key(*chosen)))
            .width(120.0)
            .show_ui(ui, |ui| {
                for k in 0..n {
                    // ai: view
                    ui.selectable_value(chosen, k, format!("Slice {} ({})", k + 1, key(k)));
                }
            })
            .response
            .accessible_name("Slice");
        // ai: view
        let on = IconButton::new(Icon::Next, "Next slice").small();
        if ui.add_enabled(*chosen + 1 < n, on).clicked() {
            *chosen += 1;
        }
    });
}
