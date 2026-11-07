// SPDX-License-Identifier: GPL-3.0-or-later
//! Pedalboard mode (roadmap 2.3; `docs/research/MIXER_DESK.md`,
//! "Pedalboard mode"): one channel's effect chain takes over the window
//! at a larger scale, every unit open, laid out in signal order left to
//! right and wrapping into rows like pedals on a board, with the
//! channel's input at the start and its level, sends and output at the
//! end.
//!
//! - Getting there: the pedal icon on a mixer strip, the Effects pane's
//!   Pedalboard button, or command `pedalboard` (on the selected track,
//!   or the one the Effects pane shows). `pedalboard-next-track` and
//!   `pedalboard-previous-track` board the next channel; Escape, Done or
//!   `pedalboard` again come back.
//! - The scale: the whole window is zoomed while the board shows, so
//!   text stays sharp; leaving takes the zoom out again.
//! - Moving: a pedal is picked up by its name plate and dropped between
//!   others, into or out of a container's chain (`fx_board.rs`); the
//!   drop is one undo step. Assistants move effects with edit_effect.
//! - `get_selection.pedalboard` says whether it shows, the channel, the
//!   scale and the effects in board order.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::App;
use crate::commands::Cmd;
use crate::icons::{Icon, IconButton};
use crate::panel_kit::{self, Flow};

/// How much larger the board draws everything, at most. A small window
/// gets less, so a row still holds a pedal or two.
pub(crate) const SCALE: f32 = 1.5;

/// The window width, in points before zooming, that gets the full scale.
const FULL_SCALE_WIDTH: f32 = 1100.0;

/// Pedalboard mode's state.
#[derive(Default)]
pub(crate) struct Pedalboard {
    /// The channel on the board: a track's key, or the master's. None
    /// while the mode is off.
    pub(crate) on: Option<String>,
    /// The zoom the board multiplied in, taken out on leaving.
    scale: f32,
    /// A channel asked for from a strip or the Effects pane, to board
    /// once the frame can.
    pub(crate) ask: Option<String>,
}

impl Pedalboard {
    pub(crate) fn shows(&self) -> bool {
        self.on.is_some()
    }
}

/// The scale for a window `width` points wide.
fn scale_for(width: f32) -> f32 {
    (width / FULL_SCALE_WIDTH).clamp(1.0, SCALE)
}

impl App {
    /// Whether `key` names a channel that can go on the board.
    fn boardable(&self, key: &str) -> bool {
        key == daw_project::Master::KEY || self.track_index(key).is_some()
    }

    /// Puts channel `key` (a track's, or the master's) on the board,
    /// entering the mode if it is off.
    pub(crate) fn enter_pedalboard(&mut self, ctx: &egui::Context, key: String) {
        if !self.boardable(&key) {
            self.message = Some("There is no such track to put on the pedalboard.".into());
            return;
        }
        if self.board.on.is_none() {
            let s = scale_for(ctx.content_rect().width());
            ctx.set_zoom_factor(ctx.zoom_factor() * s);
            self.board.scale = s;
        }
        self.board.on = Some(key);
        ctx.request_repaint();
    }

    /// Leaves the mode, taking its zoom out.
    pub(crate) fn leave_pedalboard(&mut self, ctx: &egui::Context) {
        if self.board.on.take().is_some() {
            let s = std::mem::replace(&mut self.board.scale, 1.0);
            if s > 0.0 {
                ctx.set_zoom_factor(ctx.zoom_factor() / s);
            }
            ctx.request_repaint();
        }
    }

    /// The channel command `pedalboard` boards: the one the Effects pane
    /// shows, else the selected track.
    fn board_pick(&mut self) -> Option<String> {
        if self.effects.open_master {
            return Some(daw_project::Master::KEY.into());
        }
        let shown = self.effects.open_track.filter(|&i| i < self.tracks.len());
        shown
            .or_else(|| self.inspected())
            .map(|i| self.tracks[i].key.clone())
    }

    /// Command `pedalboard`: in, or out again.
    pub(crate) fn toggle_pedalboard(&mut self, ctx: &egui::Context) {
        if self.board.shows() {
            self.leave_pedalboard(ctx);
            return;
        }
        match self.board_pick() {
            Some(key) => self.enter_pedalboard(ctx, key),
            None => self.message = Some("There is no track yet.".into()),
        }
    }

    /// The channels the board steps through: every track in order, then
    /// the master.
    fn board_order(&self) -> Vec<String> {
        (self.tracks.iter())
            .map(|t| t.key.clone())
            .chain([daw_project::Master::KEY.to_string()])
            .collect()
    }

    /// Boards the next channel (`by` 1) or the one before (-1), round
    /// the end.
    pub(crate) fn pedalboard_step(&mut self, ctx: &egui::Context, by: isize) {
        let Some(on) = self.board.on.clone() else {
            return;
        };
        let order = self.board_order();
        let at = order.iter().position(|k| *k == on).unwrap_or(0) as isize;
        let n = order.len() as isize;
        let next = order[(at + by).rem_euclid(n) as usize].clone();
        self.enter_pedalboard(ctx, next);
    }

    /// What `get_selection` says of the board.
    pub(crate) fn pedalboard_value(&self) -> Value {
        let Some(key) = &self.board.on else {
            return json!({"open": false, "track": null});
        };
        let t = if key == daw_project::Master::KEY {
            &self.master
        } else {
            match self.track_index(key) {
                Some(i) => &self.tracks[i],
                None => return json!({"open": false, "track": null}),
            }
        };
        let layers = crate::layers::chains(&t.layers);
        let order: Vec<&str> = (crate::fx::tree(&t.fx, &layers).into_iter())
            .filter_map(|r| match r {
                crate::fx::Row::Fx { i, .. } => Some(t.fx[i].key.as_str()),
                _ => None,
            })
            .collect();
        json!({
            "open": true,
            "track": key,
            "name": t.name,
            "scale": self.board.scale,
            "effects": order,
        })
    }

    /// Escape leaves the board, unless a menu or field has it. True if
    /// it did.
    pub(crate) fn pedalboard_keys(&mut self, ctx: &egui::Context) -> bool {
        if !self.board.shows() || ctx.egui_wants_keyboard_input() {
            return false;
        }
        let popup = egui::Popup::is_any_open(ctx);
        if !popup && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.leave_pedalboard(ctx);
            return true;
        }
        false
    }

    /// Whether a command run now leaves the board first: one that shows
    /// a pane or changes the layout, which the board covers.
    pub(crate) fn command_leaves_board(&self, cmd: Cmd) -> bool {
        self.board.shows()
            && (cmd.on_workspace()
                || matches!(
                    cmd,
                    Cmd::InstrumentWindow | Cmd::EffectsPane | Cmd::NotationSettings
                ))
    }

    /// Before the frame is drawn: a channel asked for goes on the board,
    /// and a board whose channel went is left. True while it shows.
    pub(crate) fn pedalboard_frame(&mut self, ctx: &egui::Context) -> bool {
        if let Some(key) = self.board.ask.take() {
            self.enter_pedalboard(ctx, key);
        }
        if let Some(key) = self.board.on.clone()
            && !self.boardable(&key)
        {
            self.leave_pedalboard(ctx);
        }
        self.board.shows()
    }

    /// The bar along the top while the board shows: the channel, with
    /// the ones either side of it, play, the chain's presets, and Done.
    pub(crate) fn pedalboard_bar(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.board.on.clone() else {
            return;
        };
        let ctx = ui.ctx().clone();
        let ti = self.track_index(&key);
        let name = ti.map_or("Master".to_string(), |i| self.tracks[i].name.clone());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                // ai: pedalboard-previous-track
                .add(IconButton::new(Icon::Previous, "Previous track").tip("The track before"))
                .clicked()
            {
                self.pedalboard_step(&ctx, -1);
            }
            let look = crate::theme::look(ui.ctx());
            ui.label(RichText::new(&name).font(look.font(look.heading)).strong())
                .on_hover_text("The channel on the pedalboard");
            if ui
                // ai: pedalboard-next-track
                .add(IconButton::new(Icon::Next, "Next track").tip("The track after"))
                .clicked()
            {
                self.pedalboard_step(&ctx, 1);
            }
            ui.separator();
            let playing = self.ctl.status().playing();
            let (icon, word) = if playing {
                (Icon::Stop, "Stop")
            } else {
                (Icon::Play, "Play")
            };
            // ai: play
            if ui.add(IconButton::new(icon, word).tip(word)).clicked() {
                self.run(&ctx, Cmd::PlayStop);
            }
            ui.separator();
            if ui
                // ai: add_preset
                .button("Presets…")
                .on_hover_text("Put a saved chain or track preset on this track")
                .clicked()
            {
                self.open_presets();
            }
            if let Some(i) = ti
                && ui
                    // ai: save_preset
                    .button("Save as preset…")
                    .on_hover_text("Keep this chain to put on other tracks and projects")
                    .clicked()
            {
                self.ask_save_preset(Some(i), daw_project::PresetKind::Chain);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    // ai: pedalboard
                    .button("Done")
                    .on_hover_text("Back to the window (Esc)")
                    .clicked()
                {
                    self.leave_pedalboard(&ctx);
                }
                ui.label(RichText::new("PEDALBOARD").weak());
            });
        });
    }

    /// The board itself: the channel's input, its chain as pedals, and
    /// its output, in rows.
    pub(crate) fn pedalboard_body(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.board.on.clone() else {
            return;
        };
        let ti = self.track_index(&key);
        let t = match ti {
            Some(i) => &self.tracks[i],
            None => &self.master,
        };
        let (note, missing) = self.frozen_note(t);
        for (name, _) in &missing {
            ui.label(
                RichText::new(format!("{name}: not playing here; see the Effects pane")).weak(),
            );
        }
        if let Some(line) = note {
            ui.label(RichText::new(line).weak());
        }
        let mut rebuild = false;
        let mut moved = None;
        egui::ScrollArea::vertical()
            .id_salt("pedalboard")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                let mut flow = Flow::new(ui);
                let look = crate::theme::look(ui.ctx());
                let heading = |ui: &mut egui::Ui, text: &str| {
                    panel_kit::name_plate(ui, text, &look, look.heading * 0.8);
                };
                // The start: where the sound comes from.
                if let Some(i) = ti {
                    let was = self.instrument_open;
                    flow.plate(ui, egui::Id::unique("board in"), 200.0, None, |ui| {
                        heading(ui, "In");
                        rebuild |= self.board_input(ui, i);
                    });
                    // Its instrument asked for: the board makes way for
                    // the Instrument pane.
                    if self.instrument_open != was && self.instrument_open.is_some() {
                        let ctx = ui.ctx().clone();
                        self.leave_pedalboard(&ctx);
                        return;
                    }
                }
                // The chain.
                let others: Vec<(String, String)> = (self.tracks.iter().enumerate())
                    .filter(|(j, _)| Some(*j) != ti)
                    .map(|(_, t)| (t.key.clone(), t.name.clone()))
                    .collect();
                let rate = self.rate;
                let t = match ti {
                    Some(i) => &mut self.tracks[i],
                    None => &mut self.master,
                };
                let layers = crate::layers::chains(&t.layers);
                let mut fx = std::mem::take(&mut t.fx);
                let mut rack = std::mem::take(&mut t.rack);
                let drawn = self.effects.board(
                    ui,
                    &mut flow,
                    &mut fx,
                    &layers,
                    rate,
                    &others,
                    &mut rack,
                    &mut self.message,
                );
                let t = match ti {
                    Some(i) => &mut self.tracks[i],
                    None => &mut self.master,
                };
                t.fx = fx;
                t.rack = rack;
                rebuild |= drawn.changed;
                moved = drawn.moved;
                // The end: level, pan and where it goes.
                flow.plate(ui, egui::Id::unique("board out"), 300.0, None, |ui| {
                    heading(ui, "Out");
                    match ti {
                        Some(i) => rebuild |= self.board_output(ui, i),
                        None => self.board_master_output(ui),
                    }
                });
                flow.finish(ui);
                ui.add_space(12.0);
            });
        if rebuild {
            self.rebuild();
        }
        if let Some(label) = moved {
            let ctx = ui.ctx().clone();
            self.proj.label = Some(label);
            self.sync_doc(&ctx, true);
            self.proj.label = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use super::*;
    use crate::agent::driver::Driver;

    fn board(d: &mut Driver) -> Value {
        d.call("get_selection", json!({})).unwrap()["pedalboard"].clone()
    }

    fn bypassed(d: &mut Driver, track: &str, effect: &str) -> bool {
        let fx = d.call("get_effects", json!({"track": track})).unwrap();
        let list = fx["effects"].as_array().cloned().unwrap_or_default();
        let e = list.iter().find(|e| e["id"] == effect).expect("the effect");
        !e["on"].as_bool().unwrap()
    }

    /// An assistant puts a track's chain on the board, reads it, moves
    /// an effect along it and into a container, steps to the next track
    /// and back, and Escape brings the window back at its own scale.
    #[test]
    fn the_pedalboard_works_with_an_assistant() {
        let mut d = Driver::new();
        let id = |v: Value, k: &str| v[k].as_str().unwrap().to_string();
        let guitar = id(
            d.call("add_track", json!({"kind": "audio", "name": "Guitar"}))
                .unwrap(),
            "track",
        );
        let bass = id(
            d.call("add_track", json!({"kind": "audio", "name": "Bass"}))
                .unwrap(),
            "track",
        );
        let mut fx = Vec::new();
        for kind in ["eq", "delay", "parallel", "reverb"] {
            let v = d
                .call("add_effect", json!({"track": guitar, "effect": kind}))
                .unwrap();
            fx.push(id(v, "effect"));
        }
        let [eq, delay, par, reverb] = [&fx[0], &fx[1], &fx[2], &fx[3]];
        d.h.run();
        assert_eq!(board(&mut d)["open"], false);
        let zoom = d.h.ctx.zoom_factor();
        // Not on the board, its next track can't be boarded.
        assert!(
            d.call("run_command", json!({"id": "pedalboard-next-track"}))
                .is_err()
        );

        d.call("run_command", json!({"id": "pedalboard", "track": guitar}))
            .unwrap();
        d.h.run();
        d.h.run();
        let b = board(&mut d);
        assert_eq!(b["open"], true);
        assert_eq!(b["track"], json!(guitar));
        assert_eq!(b["effects"], json!([eq, delay, par, reverb]));
        let scale = b["scale"].as_f64().unwrap() as f32;
        assert!(scale > 1.2, "{scale}");
        assert!((d.h.ctx.zoom_factor() - zoom * scale).abs() < 1e-4);
        // Every unit open on the board, each with its footswitch, and
        // the channel's ends.
        for name in ["EQ on", "Delay on", "Parallel chains on", "Reverb on"] {
            assert!(d.h.query_by_label(name).is_some(), "{name}");
        }
        assert!(d.h.query_by_label("Guitar volume").is_some());
        assert!(d.h.query_by_label("Done").is_some());

        // The footswitch bypasses it, as edit_effect's bypass does.
        d.h.get_by_label("EQ on").click();
        d.h.run();
        assert!(bypassed(&mut d, &guitar, eq));
        d.call(
            "edit_effect",
            json!({"track": guitar, "effect": eq, "bypass": false}),
        )
        .unwrap();
        d.h.run();
        assert!(!bypassed(&mut d, &guitar, eq));

        // The reverb first; then the delay into the container.
        d.call(
            "edit_effect",
            json!({"track": guitar, "effect": reverb, "index": 0}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(board(&mut d)["effects"], json!([reverb, eq, delay, par]));
        d.call(
            "edit_effect",
            json!({"track": guitar, "effect": delay, "container": par, "branch": 0}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(board(&mut d)["effects"], json!([reverb, eq, par, delay]));
        assert!(d.h.query_by_label("END OF PARALLEL CHAINS").is_some());

        // The next track, round past the master, and back.
        d.command("pedalboard-next-track");
        assert_eq!(board(&mut d)["track"], json!(bass));
        d.command("pedalboard-next-track");
        assert_eq!(board(&mut d)["track"], json!("master"));
        d.command("pedalboard-previous-track");
        d.command("pedalboard-previous-track");
        assert_eq!(board(&mut d)["track"], json!(guitar));

        // Escape comes back, at the window's own scale.
        d.h.key_press(egui::Key::Escape);
        d.h.run();
        d.h.run();
        assert_eq!(board(&mut d)["open"], false);
        assert!((d.h.ctx.zoom_factor() - zoom).abs() < 1e-4);
        assert!(d.h.query_by_label("Done").is_none());
    }

    /// A pedal picked up by its name plate and dropped on the left half
    /// of another goes before it, as one undo step.
    #[test]
    fn a_pedal_dragged_by_its_name_moves_and_undoes() {
        let mut d = Driver::new();
        let v = d
            .call("add_track", json!({"kind": "audio", "name": "Guitar"}))
            .unwrap();
        let guitar = v["track"].as_str().unwrap().to_string();
        let mut fx = Vec::new();
        for kind in ["eq", "delay"] {
            let v = d
                .call("add_effect", json!({"track": guitar, "effect": kind}))
                .unwrap();
            fx.push(v["effect"].as_str().unwrap().to_string());
        }
        d.call("run_command", json!({"id": "pedalboard", "track": guitar}))
            .unwrap();
        d.h.run();
        d.h.run();
        let from = d.h.get_by_label("DELAY").rect().center();
        let to = d.h.get_by_label("EQ").rect().center();
        // Onto the EQ's plate, left of its middle.
        let to = egui::pos2(to.x - 4.0, to.y + 40.0);
        d.h.hover_at(from);
        d.h.run();
        d.h.drag_at(from);
        d.h.run();
        for k in 1..=8 {
            let t = k as f32 / 8.0;
            d.h.hover_at(from + (to - from) * t);
            d.h.run();
        }
        d.h.drop_at(to);
        d.h.run();
        d.h.run();
        assert_eq!(board(&mut d)["effects"], json!([fx[1], fx[0]]));
        d.command("undo");
        d.h.run();
        assert_eq!(board(&mut d)["effects"], json!([fx[0], fx[1]]));
    }

    #[test]
    fn a_small_window_gets_less_of_the_scale() {
        assert_eq!(scale_for(1920.0), SCALE);
        assert!((scale_for(1430.0) - 1.3).abs() < 1e-6);
        assert_eq!(scale_for(800.0), 1.0);
    }
}
