// SPDX-License-Identifier: GPL-3.0-or-later
//! The sampler's front panel (UI polish step 5), on the panel kit.
//!
//! On its own sound files the modules run left to right in signal
//! order: the key map (every zone drawn over the keys and how hard they
//! are struck, clicked to choose one), the chosen zone's settings, the
//! filter with its response, the envelope with its shape, and playing
//! and output. With an SFZ library loaded the panel shows the library,
//! its articulations, its controls (mic levels and the like) and the
//! output instead, as those are what play it then.
//!
//! Every control is a setting, a command or an `edit_instrument` /
//! `set_sound` argument, so an assistant reaches each one; choosing the
//! zone shown changes only the view.

use daw_instruments::params::{Params, note_name};
use daw_instruments::sampler::{at, zone};
use daw_instruments::{Builtin, Library, zone_at};
use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::commands::Cmd;
use crate::icons::{Icon, IconButton};
use crate::instrument::{Ctx, Shown, Slot, pick_sounds, read_sound, sound_menu, sound_name};
use crate::panel_kit::{
    FilterShape, Size, envelope_display, filter_display, module, param_knob, param_switch, screen,
};
use crate::theme;

/// The panel's modules, `height` tall. True if the instrument has to be
/// made again (its zones' sounds changed).
pub(crate) fn sampler(
    ui: &mut Ui,
    slot: &mut Slot,
    cx: &Ctx,
    shown: &mut Shown,
    height: f32,
) -> bool {
    let salt = slot.id;
    let mut chosen = slot.zone;
    let mut renew = false;
    let Some(b) = slot.builtin_mut() else {
        return false;
    };
    let p = b.params().clone();
    egui::ScrollArea::horizontal()
        .id_salt(("sampler-panel", salt))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(lib) = b.library().cloned() {
                    library(ui, &p, salt, height, &lib, shown);
                    articulations(ui, &p, salt, height, &lib);
                    controls(ui, &p, salt, height);
                    library_output(ui, &p, salt, height);
                } else {
                    renew |= key_map(ui, b, &p, salt, height, &mut chosen, cx, shown);
                    renew |= zone_module(ui, b, &p, salt, height, &mut chosen, cx, shown);
                    filter(ui, &p, salt, height);
                    envelope(ui, &p, salt, height);
                    playing(ui, &p, salt, height);
                }
            });
        });
    slot.zone = chosen;
    renew
}

fn dim(ui: &Ui, text: impl Into<String>) -> RichText {
    let look = theme::look(ui.ctx());
    RichText::new(text.into())
        .size(look.small)
        .color(look.c().text_dim)
}

/// A zone's name: its sound's, or its number.
fn zone_name(b: &Builtin, z: usize) -> String {
    b.sample(&format!("zone{z}"))
        .map_or_else(|| format!("Zone {}", z + 1), |s| sound_name(&s.reference))
}

/// The zones as get_instrument gives them, with the one the panel shows:
/// what the key map draws, for an assistant to read.
pub(crate) fn zones_value(b: &Builtin, shown: usize) -> serde_json::Value {
    let p = b.params();
    let list: Vec<serde_json::Value> = (0..b.zones())
        .map(|z| {
            let g = |k| p.get(zone_at(z, k));
            serde_json::json!({
                "zone": z,
                "sound": b.sample(&format!("zone{z}")).map(|s| s.reference.clone()),
                "name": zone_name(b, z),
                "root": note_name(g(zone::ROOT) as i32),
                "lowest": note_name(g(zone::LO) as i32),
                "highest": note_name(g(zone::HI) as i32),
                "softest": g(zone::VEL_LO),
                "hardest": g(zone::VEL_HI),
                "loop": g(zone::LOOP) >= 0.5,
                "round_robin_turn": g(zone::ROBIN),
            })
        })
        .collect();
    serde_json::json!({
        "list": list,
        "shown": (b.zones() > 0).then(|| shown.min(b.zones() - 1)),
    })
}

/// Where a zone sits on the key map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placed {
    pub lo: f64,
    pub hi: f64,
    pub vel_lo: f64,
    pub vel_hi: f64,
    pub root: f64,
}

fn placed(p: &Params, z: usize) -> Placed {
    let g = |k| p.get(zone_at(z, k));
    Placed {
        lo: g(zone::LO).min(g(zone::HI)),
        hi: g(zone::HI).max(g(zone::LO)),
        vel_lo: g(zone::VEL_LO).min(g(zone::VEL_HI)),
        vel_hi: g(zone::VEL_HI).max(g(zone::VEL_LO)),
        root: g(zone::ROOT),
    }
}

/// The rect a zone takes in the map's `area`: keys 0 to 127 across,
/// velocity 1 to 127 up.
fn zone_rect(area: Rect, z: &Placed) -> Rect {
    let x = |k: f64| area.left() + area.width() * (k / 128.0) as f32;
    let y = |v: f64| area.bottom() - area.height() * ((v - 1.0) / 127.0) as f32;
    Rect::from_min_max(
        pos2(x(z.lo), y(z.vel_hi + 1.0)),
        pos2(x(z.hi + 1.0), y(z.vel_lo)),
    )
}

/// The zone under `at`: the smallest of those it is in, so one inside
/// another can still be picked.
pub(crate) fn zone_at_point(area: Rect, zones: &[Placed], at: Pos2) -> Option<usize> {
    (zones.iter().enumerate())
        .filter(|(_, z)| zone_rect(area, z).expand(1.0).contains(at))
        .min_by(|(_, a), (_, b)| {
            let (a, b) = (zone_rect(area, a), zone_rect(area, b));
            a.area().total_cmp(&b.area())
        })
        .map(|(i, _)| i)
}

/// The map: each zone a box over its keys and velocities, the chosen
/// one lit, its root note marked, a keyboard along the bottom. A click
/// chooses the zone under it.
fn map_display(ui: &mut Ui, size: egui::Vec2, zones: &[Placed], chosen: usize) -> Option<usize> {
    let (rect, look) = screen(ui, size, 8, 2);
    let c = look.c();
    let keys_h = 9.0;
    let area = Rect::from_min_max(
        rect.min + vec2(2.0, 2.0),
        pos2(rect.right() - 2.0, rect.bottom() - keys_h - 2.0),
    );
    let keys = Rect::from_min_max(
        pos2(area.left(), area.bottom() + 1.0),
        rect.max - vec2(2.0, 2.0),
    );
    let id = ui.scope_id().with("key-map");
    // ai: view
    let r = ui
        .interact(rect, id, Sense::click())
        .on_hover_text("Each zone over the keys it plays (across) and how hard they are struck (up); click one to show it");
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, "Key map"));
    let p = ui.painter_at(rect);
    // The keyboard: the black keys darker, a line at each C.
    let w = keys.width() / 128.0;
    for k in 0..128u8 {
        let black = matches!(k % 12, 1 | 3 | 6 | 8 | 10);
        let x = keys.left() + w * f32::from(k);
        let key = Rect::from_min_size(pos2(x, keys.top()), vec2(w.max(1.0), keys.height()));
        let fill = if black {
            theme::mix(c.meter_bg, c.text, 0.1)
        } else {
            theme::mix(c.meter_bg, c.text, 0.45)
        };
        p.rect_filled(key, 0.0, fill);
        if k % 12 == 0 {
            p.line_segment(
                [pos2(x, area.top()), pos2(x, keys.bottom())],
                Stroke::new(1.0, theme::mix(c.line, c.meter_bg, 0.4)),
            );
            if k >= 24 && k % 24 == 0 {
                p.text(
                    pos2(x + 2.0, area.top() + 1.0),
                    Align2::LEFT_TOP,
                    note_name(i32::from(k)),
                    look.font(look.small * 0.85),
                    c.text_dim,
                );
            }
        }
    }
    for (i, z) in zones.iter().enumerate() {
        let on = i == chosen;
        let zr = zone_rect(area, z);
        let fill = c.accent.gamma_multiply(if on { 0.45 } else { 0.16 });
        let edge = if on {
            c.accent
        } else {
            c.accent.gamma_multiply(0.55)
        };
        p.rect(
            zr,
            1.0,
            fill,
            Stroke::new(1.0, edge),
            egui::StrokeKind::Inside,
        );
        let x = area.left() + area.width() * ((z.root + 0.5) / 128.0) as f32;
        if on || zr.width() > 4.0 {
            p.line_segment(
                [pos2(x, zr.top() + 1.0), pos2(x, zr.bottom() - 1.0)],
                Stroke::new(if on { 2.0 } else { 1.0 }, edge),
            );
        }
    }
    if zones.is_empty() {
        p.text(
            area.center(),
            Align2::CENTER_CENTER,
            "No zones yet",
            look.font(look.small),
            c.text_dim,
        );
    }
    let picked = (r.clicked())
        .then(|| r.interact_pointer_pos())
        .flatten()
        .and_then(|at| zone_at_point(area, zones, at));
    if let Some(at) = r.hover_pos()
        && let Some(i) = zone_at_point(area, zones, at)
    {
        let z = zones[i];
        r.clone().on_hover_ui_at_pointer(|ui| {
            ui.label(format!(
                "Zone {}: {} to {}, root {}, velocity {} to {}",
                i + 1,
                note_name(z.lo as i32),
                note_name(z.hi as i32),
                note_name(z.root as i32),
                z.vel_lo,
                z.vel_hi
            ));
        });
    }
    picked
}

/// The key map, with the buttons that add, spread and level the zones
/// or load a library in their place.
#[allow(clippy::too_many_arguments)]
fn key_map(
    ui: &mut Ui,
    b: &mut Builtin,
    p: &Params,
    salt: u64,
    height: f32,
    chosen: &mut usize,
    cx: &Ctx,
    shown: &mut Shown,
) -> bool {
    let mut renew = false;
    let zones: Vec<Placed> = (0..b.zones()).map(|z| placed(p, z)).collect();
    module(ui, "Key map", vec2(300.0, height), |ui| {
        if let Some(path) = b.sfz_path() {
            ui.label(dim(ui, "Its SFZ library couldn't be read"))
                .on_hover_text(path.to_string());
        }
        let below = 2.0 * 22.0 + 10.0;
        let h = (ui.available_height() - below).clamp(40.0, 120.0);
        if let Some(z) = map_display(ui, vec2(ui.available_width(), h), &zones, *chosen) {
            *chosen = z;
        }
        ui.horizontal(|ui| {
            // ai: set_sound.slot
            if ui
                .small_button("Add sound files…")
                .on_hover_text(
                    "Each file becomes a zone, at the note its name gives (\"Piano C4\") or on \
                     the next key, spread over the keyboard",
                )
                .clicked()
            {
                let mut add = Vec::new();
                for f in pick_sounds("Add sound files", true) {
                    let r = f.to_string_lossy().into_owned();
                    if let Some(l) = read_sound(&r, cx, shown) {
                        add.push((sound_name(&r), l));
                    }
                }
                if !add.is_empty() {
                    b.add_zones(add);
                    renew = true;
                }
            }
            // ai: edit_instrument.spread_zones
            if ui
                .small_button("Spread")
                .on_hover_text(
                    "Spread the zones over the keys: each from halfway to the one below to \
                     halfway to the one above",
                )
                .clicked()
            {
                b.spread_zones();
            }
            // ai: normalise-zones
            if ui
                .small_button("Normalise")
                .on_hover_text("Each zone's level so its loudest moment peaks at -6 dB")
                .clicked()
            {
                shown.cmd = Some(Cmd::NormaliseZones);
            }
        });
        // ai: load-sfz
        if ui
            .small_button("Load an SFZ library…")
            .on_hover_text(
                "Plays a sample library in the SFZ format, with its layers, round robins, \
                 mics and articulations, in place of these zones",
            )
            .clicked()
        {
            shown.cmd = Some(Cmd::LoadSfz);
        }
    });
    let _ = salt;
    renew
}

/// The chosen zone: which it is, its sound, and its settings.
#[allow(clippy::too_many_arguments)]
fn zone_module(
    ui: &mut Ui,
    b: &mut Builtin,
    p: &Params,
    salt: u64,
    height: f32,
    chosen: &mut usize,
    cx: &Ctx,
    shown: &mut Shown,
) -> bool {
    let mut renew = false;
    let zones = b.zones();
    module(ui, "Zone", vec2(440.0, height), |ui| {
        if zones == 0 {
            ui.label(dim(
                ui,
                "Add sound files to make zones: each plays a sound over a range of keys.",
            ));
            return;
        }
        *chosen = (*chosen).min(zones - 1);
        let mut remove = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // ai: view
            if ui
                .add_enabled(
                    *chosen > 0,
                    // ai: view
                    IconButton::new(Icon::Previous, "Previous zone").small(),
                )
                .clicked()
            {
                *chosen -= 1;
            }
            egui::ComboBox::from_id_salt(("sampler-zone", salt))
                .selected_text(format!("{}. {}", *chosen + 1, zone_name(b, *chosen)))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for z in 0..zones {
                        let root = note_name(p.get(zone_at(z, zone::ROOT)) as i32);
                        let text = format!("{}. {} ({root})", z + 1, zone_name(b, z));
                        // ai: view
                        if ui.selectable_label(*chosen == z, text).clicked() {
                            *chosen = z;
                        }
                    }
                })
                .response
                .accessible_name("Zone shown");
            // ai: view
            if ui
                .add_enabled(
                    *chosen + 1 < zones,
                    // ai: view
                    IconButton::new(Icon::Next, "Next zone").small(),
                )
                .clicked()
            {
                *chosen += 1;
            }
            let current = b
                .sample(&format!("zone{chosen}"))
                .map(|s| s.reference.clone())
                .unwrap_or_default();
            if let Some(r) = sound_menu(ui, ("zone-sound", salt, *chosen), &current)
                && let Some(l) = read_sound(&r, cx, shown)
            {
                b.set_sample(&format!("zone{chosen}"), Some(l));
                renew = true;
            }
            // ai: edit_instrument.remove_zone
            if ui
                .add(IconButton::new(Icon::Delete, "Remove zone").small())
                .clicked()
            {
                remove = true;
            }
        });
        let z = *chosen;
        let k = |i| zone_at(z, i);
        let salt = ("zone", salt, z);
        ui.horizontal(|ui| {
            for (i, label) in [
                (zone::ROOT, "Root"),
                (zone::LO, "Lowest"),
                (zone::HI, "Highest"),
                (zone::VEL_LO, "Softest"),
                (zone::VEL_HI, "Hardest"),
                (zone::ROBIN, "Turn"),
            ] {
                param_knob(ui, salt, p, k(i), Some(label), Size::Small);
            }
        });
        ui.horizontal(|ui| {
            for (i, label) in [
                (zone::TUNE, "Tune"),
                (zone::LEVEL, "Level"),
                (zone::PAN, "Pan"),
            ] {
                param_knob(ui, salt, p, k(i), Some(label), Size::Small);
            }
            ui.vertical(|ui| {
                ui.label(dim(ui, "Loop"));
                param_switch(ui, salt, p, k(zone::LOOP));
            });
            for (i, label) in [(zone::LOOP_START, "Start"), (zone::LOOP_END, "End")] {
                param_knob(ui, salt, p, k(i), Some(label), Size::Small);
            }
        });
        if remove {
            b.remove_zone(z);
            *chosen = z.saturating_sub(usize::from(z + 1 >= zones));
            renew = true;
        }
    });
    renew
}

fn shape(choice: f64) -> FilterShape {
    match choice as u32 {
        1 => FilterShape::Band,
        2 => FilterShape::High,
        _ => FilterShape::Low { poles: 2 },
    }
}

fn filter(ui: &mut Ui, p: &Params, salt: u64, height: f32) {
    module(ui, "Filter", vec2(214.0, height), |ui| {
        param_switch(ui, ("sampler", salt), p, at::FILTER);
        filter_display(
            ui,
            vec2(ui.available_width(), 44.0),
            p.get(at::CUTOFF),
            p.get(at::RESONANCE),
            shape(p.get(at::FILTER)),
        );
        ui.horizontal(|ui| {
            param_knob(
                ui,
                ("sampler", salt),
                p,
                at::CUTOFF,
                Some("Cutoff"),
                Size::Large,
            );
            param_knob(
                ui,
                ("sampler", salt),
                p,
                at::RESONANCE,
                Some("Resonance"),
                Size::Small,
            );
        });
    });
}

fn envelope(ui: &mut Ui, p: &Params, salt: u64, height: f32) {
    module(ui, "Envelope", vec2(240.0, height), |ui| {
        envelope_display(
            ui,
            vec2(ui.available_width(), 52.0),
            Some("Level"),
            p.get(at::ATTACK),
            p.get(at::DECAY),
            p.get(at::SUSTAIN),
            p.get(at::RELEASE),
        );
        ui.horizontal(|ui| {
            for (i, label) in [
                (at::ATTACK, "Attack"),
                (at::DECAY, "Decay"),
                (at::SUSTAIN, "Sustain"),
                (at::RELEASE, "Release"),
            ] {
                param_knob(ui, ("sampler", salt), p, i, Some(label), Size::Small);
            }
        });
    });
}

fn playing(ui: &mut Ui, p: &Params, salt: u64, height: f32) {
    module(ui, "Playing", vec2(176.0, height), |ui| {
        ui.horizontal(|ui| {
            for (i, label) in [(at::VELOCITY, "Velocity"), (at::BEND, "Bend")] {
                param_knob(ui, ("sampler", salt), p, i, Some(label), Size::Small);
            }
        });
        param_knob(
            ui,
            ("sampler", salt),
            p,
            at::LEVEL,
            Some("Level"),
            Size::Normal,
        );
    });
}

// ----------------------------------------------------------------------
// With an SFZ library

/// What the library is, what it doesn't play, and the way back to zones.
fn library(ui: &mut Ui, p: &Params, salt: u64, height: f32, lib: &Library, shown: &mut Shown) {
    let _ = p;
    module(ui, "Library", vec2(300.0, height), |ui| {
        let look = theme::look(ui.ctx());
        ui.label(
            RichText::new(lib.name())
                .font(look.font(look.heading * 0.8))
                .color(look.c().badge),
        )
        .on_hover_text(lib.path.display().to_string());
        ui.label(dim(
            ui,
            format!(
                "{} regions, {} sounds, {} MB in memory{}",
                lib.sfz.regions.len(),
                lib.sources.len(),
                lib.bytes >> 20,
                if lib.streamed {
                    ", the rest from disk"
                } else {
                    ""
                }
            ),
        ));
        if !lib.problems.is_empty() {
            let list: Vec<&str> = lib.problems.iter().take(20).map(String::as_str).collect();
            ui.label(dim(
                ui,
                format!("{} problems reading it", lib.problems.len()),
            ))
            .on_hover_text(list.join("\n"));
        }
        if !lib.sfz.unsupported.is_empty() {
            let list: Vec<String> = (lib.sfz.unsupported.iter())
                .map(|(k, n)| format!("{k} ({n})"))
                .collect();
            ui.label(dim(ui, "Some of its settings aren't played yet"))
                .on_hover_text(list.join(", "));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // ai: load-sfz
            if ui
                .small_button("Load another…")
                .on_hover_text("Plays a different SFZ library")
                .clicked()
            {
                shown.cmd = Some(Cmd::LoadSfz);
            }
            // ai: unload-sfz
            if ui
                .small_button("Back to zones")
                .on_hover_text("Stops playing the library; the sampler plays its own zones again")
                .clicked()
            {
                shown.cmd = Some(Cmd::UnloadSfz);
            }
        });
    });
    let _ = salt;
}

/// The library's articulations, chosen by key switch, as a list.
fn articulations(ui: &mut Ui, p: &Params, salt: u64, height: f32, lib: &Library) {
    let Some(i) = p.index("keyswitch") else {
        return;
    };
    let switches = lib.sfz.key_switches();
    module(ui, "Articulation", vec2(200.0, height), |ui| {
        let now = p.get(i) as u8;
        egui::ScrollArea::vertical()
            .id_salt(("sfz-switches", salt))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (k, name) in &switches {
                    let text = format!("{name} ({})", note_name(i32::from(*k)));
                    // ai: set_parameter
                    if ui.selectable_label(*k == now, text).clicked() {
                        p.set(i, f64::from(*k));
                    }
                }
            });
    });
}

/// A knob for each controller the library names (mic levels, say).
fn controls(ui: &mut Ui, p: &Params, salt: u64, height: f32) {
    let ccs: Vec<usize> = (p.list().iter().enumerate())
        .filter(|(_, x)| x.key.starts_with("cc"))
        .map(|(i, _)| i)
        .collect();
    if ccs.is_empty() {
        return;
    }
    let cell = Size::Small.cell() + 6.0;
    let rows = ((height - 30.0) / (Size::Small.height() + 4.0))
        .floor()
        .max(1.0) as usize;
    let columns = ccs.len().div_ceil(rows).clamp(2, 8);
    let width = columns as f32 * cell + 16.0;
    module(ui, "Controls", vec2(width, height), |ui| {
        egui::ScrollArea::vertical()
            .id_salt(("sfz-ccs", salt))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in ccs.chunks(columns) {
                    ui.horizontal(|ui| {
                        for &i in row {
                            param_knob(ui, ("sfz-cc", salt), p, i, None, Size::Small);
                        }
                    });
                }
            });
    });
}

fn library_output(ui: &mut Ui, p: &Params, salt: u64, height: f32) {
    module(ui, "Output", vec2(150.0, height), |ui| {
        param_knob(ui, ("sfz", salt), p, at::LEVEL, Some("Level"), Size::Normal);
        if let Some(i) = p.index("normalise") {
            ui.label(dim(ui, "Normalise"));
            param_switch(ui, ("sfz", salt), p, i);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_picks_the_smallest_zone_under_it() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(128.0, 127.0));
        let all = Placed {
            lo: 0.0,
            hi: 127.0,
            vel_lo: 1.0,
            vel_hi: 127.0,
            root: 60.0,
        };
        let middle = Placed {
            lo: 60.0,
            hi: 71.0,
            vel_lo: 1.0,
            vel_hi: 127.0,
            root: 60.0,
        };
        let zones = [all, middle];
        assert_eq!(zone_at_point(area, &zones, pos2(64.0, 50.0)), Some(1));
        assert_eq!(zone_at_point(area, &zones, pos2(10.0, 50.0)), Some(0));
        assert_eq!(zone_at_point(area, &[middle], pos2(10.0, 50.0)), None);
        // Velocity runs up: a soft layer sits at the bottom.
        let soft = Placed {
            vel_hi: 64.0,
            ..middle
        };
        let r = zone_rect(area, &soft);
        assert!(r.bottom() >= 126.0 && r.top() > 60.0, "{r:?}");
    }
}

/// The sampler's panel in the rack, driven through the assistant
/// interface and its own controls.
#[cfg(test)]
mod panel_tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    fn param(d: &mut Driver, t: &str, key: &str) -> f64 {
        let p = d.call("list_parameters", json!({"track": t})).unwrap();
        (p[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == key)
            .and_then(|p| p["value"].as_f64())
            .unwrap_or_else(|| panic!("no {key}"))
    }

    fn open(d: &mut Driver) {
        d.app().instrument_open = Some(0);
        d.app().workspace.full = Some(Pane::Instrument);
        d.h.run();
        d.h.run();
    }

    #[test]
    fn the_sampler_panel_shows_its_zones_and_an_assistant_reads_them() {
        let mut d = Driver::new();
        d.command("add-sampler-track");
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        for (z, sound, root) in [(0, "made:keys", 48.0), (1, "made:pluck", 60.0)] {
            d.call(
                "set_sound",
                json!({"track": t, "slot": format!("zone{z}"), "sound": sound}),
            )
            .unwrap();
            d.call(
                "set_parameter",
                json!({"track": t, "param": format!("instrument.zone{z}.root"), "value": root}),
            )
            .unwrap();
        }
        d.call("edit_instrument", json!({"track": t, "spread_zones": true}))
            .unwrap();
        open(&mut d);

        // The rack unit, its modules and its knobs.
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["rack"][0]["view"], "shown", "{info}");
        assert_eq!(info["zones"]["list"].as_array().unwrap().len(), 2, "{info}");
        assert_eq!(info["zones"]["shown"], 0);
        assert_eq!(info["zones"]["list"][1]["root"], "C4", "{info}");
        d.h.get_by_label("Key map");
        d.h.get_by_label("Cutoff");
        d.h.get_by_label("Attack");
        d.h.get_by_label("Show in rack");

        // Next zone shows the second, which get_instrument says.
        d.h.get_by_label("Next zone").click();
        d.h.run();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["zones"]["shown"], 1, "{info}");

        // Its loop switch sets the zone's setting an assistant reads.
        d.h.get_by_label("Loop: On").click();
        d.h.run();
        assert_eq!(param(&mut d, &t, "instrument.zone1.loop"), 1.0);
        // The filter switch is the sampler's own setting.
        d.h.get_by_label("Filter: High-pass").click();
        d.h.run();
        assert_eq!(param(&mut d, &t, "instrument.filter"), 2.0);

        // Remove zone takes the one shown away.
        d.h.get_by_label("Remove zone").click();
        d.h.run();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(info["zones"]["list"].as_array().unwrap().len(), 1, "{info}");
        assert_eq!(info["zones"]["shown"], 0);
        assert_eq!(info["zones"]["list"][0]["name"], "Keys", "{info}");
    }

    #[test]
    fn the_sampler_panel_shows_a_loaded_library_and_its_controls() {
        let dir = std::env::temp_dir().join(format!("daw-sampler-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = hound::WavWriter::create(
            dir.join("hit.wav"),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..4800 {
            w.write_sample(((i as f32 * 0.05).sin() * 8000.0) as i16)
                .unwrap();
        }
        w.finalize().unwrap();
        std::fs::write(
            dir.join("Kit.sfz"),
            "<control> label_cc70=Close mic set_cc70=127\n\
             <region> key=36 sample=hit.wav sw_last=24 sw_label=Hits amplitude_cc70=100\n\
             <region> key=36 sample=hit.wav sw_last=25 sw_label=Rolls\n",
        )
        .unwrap();
        let mut d = Driver::new();
        d.call(
            "file_command",
            json!({"id": "load-sfz", "path": dir.join("Kit.sfz")}),
        )
        .unwrap();
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        open(&mut d);

        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert!(info["zones"].is_null(), "a library, not zones: {info}");
        d.h.get_by_label("Close mic");
        d.h.get_by_label("Back to zones");
        assert!(d.h.query_by_label("Key map").is_none());

        // An articulation chosen on the panel is the setting.
        d.h.get_by_label("Rolls (C#1)").click();
        d.h.run();
        assert_eq!(param(&mut d, &t, "instrument.keyswitch"), 25.0);

        // And back to zones by its button, through the command.
        d.h.get_by_label("Back to zones").click();
        d.h.run();
        d.h.run();
        d.h.run();
        let info = d.call("get_instrument", json!({"track": t})).unwrap();
        assert!(info["library"].is_null(), "{info}");
        d.h.get_by_label("Key map");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
