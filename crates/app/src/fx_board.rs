// SPDX-License-Identifier: GPL-3.0-or-later
//! The chain on the pedalboard (roadmap 2.3, "Pedalboard mode";
//! `docs/research/MIXER_DESK.md`): one track's effects as pedals in
//! signal order, left to right and on down in rows, every one open, each
//! with a footswitch, and picked up by its name plate to move it.
//!
//! The rest of the board (the ends of the channel, the bar along the
//! top, entering and leaving) is in `pedalboard.rs`; this part is the
//! chain, drawn from the same pieces as the Effects pane's rack.

use daw_project::{RackUnit, RackView};
use eframe::egui::{self, Rect, RichText, Stroke, pos2, vec2};

use super::{
    Effects, FxSlot, Row, SlotKind, add_list, outputs_picker, params, presets_menu,
    remove_with_contents, siblings, sidechain_picker, tree, with_contents,
};
use crate::icons::{Icon, IconButton};
use crate::panel_kit::{self, EAR, Flow, STOMP, UNIT};

/// An effect picked up on the board, by its slot's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lifted(pub u64);

/// Where an effect dropped on the board goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Spot {
    /// The chain: a layer's or a container's (its key and chain number);
    /// none for the track's own.
    pub place: Option<(String, u32)>,
    /// Before this effect (its slot's id), or at the chain's end.
    pub before: Option<u64>,
}

/// What drawing the chain did.
#[derive(Default)]
pub(crate) struct Drawn {
    /// The chain changed: the graph needs building again.
    pub changed: bool,
    /// An effect was moved by dragging: the undo step's name.
    pub moved: Option<String>,
}

/// A width for a tile between pedals: a chain's heading.
const TILE: f32 = 150.0;

/// Moves the effect whose slot id is `id` to `to`. False if nothing
/// changed: it was there already, or `to` is inside it.
pub(crate) fn move_to(slots: &mut Vec<FxSlot>, id: u64, to: &Spot) -> bool {
    let Some(from) = slots.iter().position(|s| s.id == id) else {
        return false;
    };
    let inside: Vec<&str> = (with_contents(slots, from).into_iter())
        .map(|j| slots[j].key.as_str())
        .collect();
    if to
        .place
        .as_ref()
        .is_some_and(|(k, _)| inside.contains(&k.as_str()))
    {
        return false;
    }
    if to.before == Some(id) {
        return false;
    }
    let order = |slots: &[FxSlot]| -> Vec<(u64, Option<(String, u32)>)> {
        slots.iter().map(|s| (s.id, s.place.clone())).collect()
    };
    let was = order(slots);
    let mut s = slots.remove(from);
    s.place = to.place.clone();
    let at = (to.before)
        .and_then(|b| slots.iter().position(|x| x.id == b))
        .unwrap_or(slots.len());
    slots.insert(at, s);
    order(slots) != was
}

/// The spot just before `i`, in its chain.
fn before(slots: &[FxSlot], i: usize) -> Spot {
    Spot {
        place: slots[i].place.clone(),
        before: Some(slots[i].id),
    }
}

/// The spot just after `i`, in its chain.
fn after(slots: &[FxSlot], layers: &[(String, String)], i: usize) -> Spot {
    let sib = siblings(slots, layers, i);
    let at = sib.iter().position(|&j| j == i).unwrap_or(0);
    Spot {
        place: slots[i].place.clone(),
        before: sib.get(at + 1).map(|&j| slots[j].id),
    }
}

/// The start of the chain `place` names.
fn start_of(slots: &[FxSlot], layers: &[(String, String)], place: Option<(String, u32)>) -> Spot {
    let first = (0..slots.len()).find(|&j| match &place {
        None => super::on_top(slots, layers, j),
        Some(p) => slots[j].place.as_ref() == Some(p),
    });
    Spot {
        before: first.map(|j| slots[j].id),
        place,
    }
}

/// A place on the board an effect can be dropped: the left half of
/// `rect` drops at `left`, the right half at `right`.
struct Target {
    rect: Rect,
    left: Spot,
    right: Spot,
}

impl Effects {
    /// One track's chain on the pedalboard, laid out by `flow`. `layers`
    /// are its layered instrument's layers (key and heading), whose
    /// chains come first; `others` the tracks a key input can come from.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn board(
        &mut self,
        ui: &mut egui::Ui,
        flow: &mut Flow,
        slots: &mut Vec<FxSlot>,
        layers: &[(String, String)],
        rate: u32,
        others: &[(String, String)],
        rack: &mut Vec<RackUnit>,
        message: &mut Option<String>,
    ) -> Drawn {
        let mut out = Drawn::default();
        let effects: Vec<_> = (self.list.plugins().into_iter())
            .filter(|p| p.enabled && !p.class.is_instrument())
            .collect();
        let chains_before: Vec<usize> = slots.iter().map(FxSlot::chains).collect();
        let mut add = None;
        let mut remove = None;
        let mut swap = None;
        let mut restart = None;
        let mut relayout = Vec::new();
        let mut picks = Vec::new();
        let mut targets: Vec<Target> = Vec::new();
        let mut hidden = Vec::new();
        // Containers whose pedals are being drawn, with their depth: an
        // end tile closes each.
        let mut open: Vec<(usize, usize)> = Vec::new();
        let look = crate::theme::look(ui.ctx());
        let side = crate::character::current(ui.ctx()).side(EAR);
        let rows = tree(slots, layers);
        let tile_height = UNIT;
        let close = |ui: &mut egui::Ui,
                     flow: &mut Flow,
                     slots: &[FxSlot],
                     targets: &mut Vec<Target>,
                     c: usize| {
            let name = slots[c].name.clone();
            let rect = flow.tile(
                ui,
                egui::Id::unique(("end", slots[c].id)),
                90.0,
                tile_height,
                |ui| {
                    ui.label(
                        RichText::new(format!("END OF {}", name.to_uppercase()))
                            .size(look.small)
                            .color(look.c().text_dim),
                    );
                },
            );
            let spot = after(slots, layers, c);
            targets.push(Target {
                rect,
                left: spot.clone(),
                right: spot,
            });
        };
        for row in rows {
            let depth = match row {
                Row::Layer { .. } | Row::Own => 0,
                Row::Chain { depth, .. } | Row::Fx { depth, .. } => depth,
            };
            while let Some(&(c, d)) = open.last()
                && (d >= depth || matches!(row, Row::Layer { .. } | Row::Own))
            {
                open.pop();
                close(ui, flow, slots, &mut targets, c);
            }
            let (i, idle) = match row {
                Row::Layer { n } => {
                    let (key, name) = &layers[n];
                    let rect = flow.tile(
                        ui,
                        egui::Id::unique(("layer", key)),
                        TILE,
                        tile_height,
                        |ui| {
                            ui.label(
                                RichText::new(name.to_uppercase())
                                    .size(look.small)
                                    .color(look.c().text_dim),
                            );
                            ui.menu_button("Add…", |ui| {
                                if let Some(p) = add_list(ui, &mut self.filter, &effects) {
                                    add = Some((p, Some((key.clone(), 0))));
                                    ui.close();
                                }
                            })
                            .response
                            .accessible_name(format!("Add an effect to {name}"));
                        },
                    );
                    let spot = start_of(slots, layers, Some((key.clone(), 0)));
                    targets.push(Target {
                        rect,
                        left: spot.clone(),
                        right: spot,
                    });
                    continue;
                }
                Row::Own => {
                    let rect =
                        flow.tile(ui, egui::Id::unique("own chain"), TILE, tile_height, |ui| {
                            ui.label(
                                RichText::new("THE TRACK'S OWN CHAIN")
                                    .size(look.small)
                                    .color(look.c().text_dim),
                            );
                        });
                    let spot = start_of(slots, layers, None);
                    targets.push(Target {
                        rect,
                        left: spot.clone(),
                        right: spot,
                    });
                    continue;
                }
                Row::Chain { of, branch, .. } => {
                    let chain = slots[of].chain_name(branch);
                    let name = format!("{}: {chain}", slots[of].name);
                    let key = slots[of].key.clone();
                    let heard = slots[of].builtin().and_then(|b| b.heard());
                    let id = egui::Id::unique(("chain", slots[of].id, branch));
                    let rect = flow.tile(ui, id, TILE, tile_height, |ui| {
                        ui.label(
                            RichText::new(name.to_uppercase())
                                .size(look.small)
                                .color(look.c().text_dim),
                        );
                        match heard {
                            Some(h) if h == branch => {
                                ui.label(RichText::new("heard").strong());
                            }
                            Some(_) => {
                                ui.label(RichText::new("not heard now").weak());
                            }
                            None => {}
                        }
                        ui.menu_button("Add…", |ui| {
                            if let Some(p) = add_list(ui, &mut self.filter, &effects) {
                                add = Some((p, Some((key.clone(), branch as u32))));
                                ui.close();
                            }
                        })
                        .response
                        .accessible_name(format!("Add an effect to {chain}"));
                    });
                    let spot = start_of(slots, layers, Some((key, branch as u32)));
                    targets.push(Target {
                        rect,
                        left: spot.clone(),
                        right: spot,
                    });
                    continue;
                }
                Row::Fx { i, idle, .. } => (i, idle),
            };
            if crate::instrument::rack_view(rack, &slots[i].key) == RackView::Hidden {
                hidden.push(i);
                continue;
            }
            if slots[i].chains() > 0 {
                open.push((i, depth));
            }
            let sib = siblings(slots, layers, i);
            let at = sib.iter().position(|&j| j == i).unwrap_or(0);
            let up = at.checked_sub(1).map(|p| sib[p]);
            let down = sib.get(at + 1).copied();
            let (left, right) = (before(slots, i), after(slots, layers, i));
            let s = &mut slots[i];
            let kind = s.builtin().map(daw_effects::Effect::kind);
            let face = kind.filter(|_| s.crashed.is_none());
            let width = match face {
                Some(k) => crate::fx_panel::width(ui, k) + 2.0 * side + 8.0,
                None => 420.0,
            }
            .max(300.0);
            let height =
                face.map(|k| UNIT * f32::from(crate::fx_panel::face(k).units) + STOMP + 8.0);
            let id = egui::Id::unique(("pedal", s.id));
            let ((), rect) = flow.plate(ui, id, width, height, |ui| {
                ui.horizontal(|ui| {
                    ui.set_height(26.0);
                    let name = s.name.clone();
                    // ai: edit_effect.index
                    let lift =
                        ui.dnd_drag_source(egui::Id::unique(("lift", s.id)), Lifted(s.id), |ui| {
                            panel_kit::name_plate(ui, &name, &look, look.heading * 0.8);
                        });
                    lift.response
                        .on_hover_cursor(egui::CursorIcon::Grab)
                        .on_hover_text(match kind {
                            Some(k) => format!(
                                "{}. Drag it by its name to move it along the board.",
                                k.about()
                            ),
                            None => "Drag it by its name to move it along the board.".into(),
                        });
                    if idle {
                        ui.label(RichText::new("not playing").weak()).on_hover_text(
                            "Its container has no chain of this number now; move it or add \
                             the chain back",
                        );
                    }
                    if s.latency > 0 {
                        ui.label(RichText::new(format!("{} smp", s.latency)).weak());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // ai: remove_effect
                        if ui
                            .add(
                                IconButton::new(Icon::Close, format!("Remove {name}"))
                                    .small()
                                    .tip("Remove"),
                            )
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        // The arrows stay, for the keyboard and screen
                        // readers: later goes right, earlier left.
                        if ui
                            // ai: edit_effect.index
                            .add_enabled(
                                down.is_some(),
                                IconButton::new(Icon::Next, format!("Move {name} later"))
                                    .small()
                                    .tip("Later in the chain"),
                            )
                            .clicked()
                        {
                            swap = down.map(|d| (i, d));
                        }
                        if ui
                            // ai: edit_effect.index
                            .add_enabled(
                                up.is_some(),
                                IconButton::new(Icon::Previous, format!("Move {name} earlier"))
                                    .small()
                                    .tip("Earlier in the chain"),
                            )
                            .clicked()
                        {
                            swap = up.map(|u| (u, i));
                        }
                        if s.is_plugin() {
                            ui.menu_button("Presets", |ui| {
                                if let Some(e) = presets_menu(ui, s, &self.presets_dir) {
                                    *message = Some(format!("{name}: {e}"));
                                }
                            })
                            .response
                            .accessible_name(format!("Presets for {name}"));
                        }
                        if s.takes_key() {
                            let had = s.sidechain.is_some();
                            if sidechain_picker(ui, s, others) {
                                out.changed = true;
                                if s.is_plugin() && had != s.sidechain.is_some() {
                                    relayout.push(i);
                                }
                            }
                        }
                        if s.is_plugin() {
                            // ai: view
                            ui.toggle_value(&mut s.show_params, "Controls")
                                .on_hover_text("Show the plugin's settings here");
                            if ui
                                // ai: view
                                .button("Edit")
                                .on_hover_text("Open the plugin's own window")
                                .clicked()
                                && let Err(e) = s.open_editor()
                            {
                                *message = Some(format!("{name}: {e}"));
                            }
                        }
                    });
                });
                if outputs_picker(ui, s) {
                    relayout.push(i);
                }
                if let Some(note) = &s.note {
                    ui.label(RichText::new(note).small().weak());
                }
                if let Some(why) = &s.crashed {
                    ui.label(
                        RichText::new(format!("Stopped: {why}. Its audio passes through."))
                            .color(ui.visuals().error_fg_color),
                    );
                    // ai: edit_effect.restart
                    if ui
                        .button("Restart")
                        .on_hover_text("Start the plugin again with its last saved settings")
                        .clicked()
                    {
                        restart = Some(i);
                    }
                } else {
                    let pick = match &s.kind {
                        SlotKind::Builtin(b) => {
                            let body = ui.available_height() - STOMP - 10.0;
                            crate::fx_panel::panel(ui, s.id, b, body)
                        }
                        SlotKind::Plugin { .. } if s.show_params => params(ui, s),
                        SlotKind::Plugin { .. } => None,
                    };
                    if let Some(pick) = pick {
                        picks.push((i, pick));
                    }
                }
                // The footswitch, along the foot of the pedal.
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    let on = !s.bypass;
                    // ai: edit_effect.bypass
                    let r = panel_kit::stomp(ui, on, &format!("{} on", s.name));
                    if r.on_hover_text(if on {
                        "On: stomp to switch it off (bypass)"
                    } else {
                        "Off (bypassed): stomp to switch it on"
                    })
                    .clicked()
                    {
                        s.bypass = on;
                        out.changed = true;
                    }
                });
            });
            targets.push(Target { rect, left, right });
        }
        while let Some((c, _)) = open.pop() {
            close(ui, flow, slots, &mut targets, c);
        }
        // The end of the chain: add one more.
        let rect = flow.tile(ui, egui::Id::unique("board add"), TILE, tile_height, |ui| {
            egui::ComboBox::from_id_salt("board-add-fx")
                .selected_text("Add effect…")
                .width(TILE - 16.0)
                .show_ui(ui, |ui| {
                    if let Some(p) = add_list(ui, &mut self.filter, &effects) {
                        add = Some((p, None));
                    }
                })
                .response
                .accessible_name("Add effect");
        });
        let end = Spot {
            place: None,
            before: None,
        };
        targets.push(Target {
            rect,
            left: end.clone(),
            right: end,
        });
        // Hidden from the rack: still playing, shown faded with a way to
        // bring them back.
        for i in hidden {
            let s = &mut slots[i];
            let id = egui::Id::unique(("hidden pedal", s.id));
            flow.plate(ui, id, 240.0, None, |ui| {
                ui.set_opacity(0.6);
                panel_kit::name_plate(ui, &s.name, &look, look.heading * 0.7);
                ui.label(RichText::new("Hidden from the rack; it still plays.").weak());
                if ui
                    // ai: set_rack.view
                    .button("Show")
                    .on_hover_text("Show it in the rack and on the board again")
                    .clicked()
                {
                    crate::instrument::set_rack_view(rack, &s.key, RackView::Shown);
                }
            });
        }
        // A pedal being dragged: where it would land, and the drop.
        let ctx = ui.ctx().clone();
        if let Some(lifted) = egui::DragAndDrop::payload::<Lifted>(&ctx) {
            let at = ctx.pointer_latest_pos();
            let spot = at.and_then(|p| {
                let t = targets.iter().find(|t| t.rect.contains(p))?;
                let left = p.x < t.rect.center().x;
                let edge = if left { t.rect.left() } else { t.rect.right() };
                Some((
                    if left {
                        t.left.clone()
                    } else {
                        t.right.clone()
                    },
                    edge,
                    t.rect,
                ))
            });
            if let Some((_, x, rect)) = &spot {
                let p = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::unique("board drop"),
                ));
                let x = if *x == rect.left() {
                    x - panel_kit::PEDAL_GAP / 2.0
                } else {
                    x + panel_kit::PEDAL_GAP / 2.0
                };
                p.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(4.0, look.c().accent),
                );
            }
            if ctx.input(|i| i.pointer.any_released()) {
                egui::DragAndDrop::clear_payload(&ctx);
                if let Some((spot, ..)) = spot {
                    let name = (slots.iter().find(|s| s.id == lifted.0)).map(|s| s.name.clone());
                    if move_to(slots, lifted.0, &spot) {
                        out.changed = true;
                        out.moved = name.map(|n| format!("Move {n}"));
                    }
                }
            }
        }
        let _ = vec2;
        out.changed |= self.use_picks(slots, picks, rate, message);
        if let Some((a, b)) = swap {
            slots.swap(a, b);
            out.changed = true;
        }
        if let Some((pick, place)) = add {
            out.changed |= self.add_picked(slots, pick, place, rate, message);
        }
        if (slots.iter().map(FxSlot::chains)).ne(chains_before.iter().copied()) {
            out.changed = true;
        }
        relayout.extend(restart);
        out.changed |= self.reload_each(slots, relayout, rate, message);
        if let Some(i) = remove {
            remove_with_contents(slots, i);
            out.changed = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects() -> Effects {
        let dir = std::env::temp_dir().join(format!("daw-board-{}", std::process::id()));
        Effects::new(dir.join("plugins.json"))
    }

    fn ids(slots: &[FxSlot]) -> Vec<String> {
        slots.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn a_dropped_effect_goes_before_after_and_into_a_container() {
        let mut fx = effects();
        let mut slots: Vec<FxSlot> = [
            daw_effects::Kind::Eq,
            daw_effects::Kind::Delay,
            daw_effects::Kind::Parallel,
            daw_effects::Kind::Reverb,
        ]
        .into_iter()
        .map(|k| fx.create_builtin(k, 48_000))
        .collect();
        let names = ids(&slots);
        let (eq, delay, par, reverb) = (slots[0].id, slots[1].id, slots[2].id, slots[3].id);
        // The reverb before the EQ.
        let to = Spot {
            place: None,
            before: Some(eq),
        };
        assert!(move_to(&mut slots, reverb, &to));
        assert_eq!(
            ids(&slots),
            [&names[3], &names[0], &names[1], &names[2]].map(String::clone)
        );
        // Where it is already: nothing changes.
        assert!(!move_to(&mut slots, reverb, &to));
        // The delay into the container's first chain.
        let key = slots.iter().find(|s| s.id == par).unwrap().key.clone();
        let into = Spot {
            place: Some((key.clone(), 0)),
            before: None,
        };
        assert!(move_to(&mut slots, delay, &into));
        let d = slots.iter().find(|s| s.id == delay).unwrap();
        assert_eq!(d.place, Some((key.clone(), 0)));
        // The container can't go inside itself.
        assert!(!move_to(&mut slots, par, &into));
        // Out again, to the end of the track's own chain.
        let end = Spot {
            place: None,
            before: None,
        };
        assert!(move_to(&mut slots, delay, &end));
        assert_eq!(slots.last().unwrap().id, delay);
        assert_eq!(slots.last().unwrap().place, None);
    }
}
