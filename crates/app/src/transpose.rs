// SPDX-License-Identifier: GPL-3.0-or-later
//! Transposing audio clips (the first step of plan B2, pitch editing):
//! each clip has its own transpose in semitones and cents, played
//! through the engine's stretcher, so it works with clips that follow
//! tempo and with the practice transpose on top. By default the
//! formants stay where they were, so a voice moved into another key
//! still sounds like the same singer; they can move with the pitch
//! instead, as tape sped up does.
//!
//! The commands act on the selected audio clips and those grouped with
//! them, as the stretch commands do.

use eframe::egui::{self, RichText};

use daw_project::MAX_TRANSPOSE_CENTS;

use crate::App;
use crate::commands::Cmd;
use crate::timeline::Clip;

/// The Transpose window's state.
#[derive(Default)]
pub(crate) struct Transpose {
    pub open: bool,
    /// What the window would set, in semitones and cents.
    pub semitones: i32,
    pub cents: i32,
    pub move_formants: bool,
    /// The window has been filled in from the selection.
    pub filled: bool,
}

/// A transpose as people read it: "+3 st", "−6 st 20 ct", "+15 ct".
pub(crate) fn label(cents: i32) -> String {
    if cents == 0 {
        return "0 st".into();
    }
    let sign = if cents < 0 { "−" } else { "+" };
    let a = cents.unsigned_abs();
    match (a / 100, a % 100) {
        (s, 0) => format!("{sign}{s} st"),
        (0, c) => format!("{sign}{c} ct"),
        (s, c) => format!("{sign}{s} st {c} ct"),
    }
}

/// Holds a transpose within what a clip takes.
pub(crate) fn clamp(cents: i64) -> i32 {
    cents.clamp(-MAX_TRANSPOSE_CENTS, MAX_TRANSPOSE_CENTS) as i32
}

impl Clip {
    /// The clip's transpose in semitones, as the engine takes it.
    pub fn semitones(&self) -> f64 {
        self.cents as f64 / 100.0
    }

    /// Sets the transpose, held within an octave either way; true if it
    /// changed.
    pub fn set_transpose(&mut self, cents: i64) -> bool {
        let c = clamp(cents);
        let changed = c != self.cents;
        self.cents = c;
        changed
    }
}

impl App {
    /// Does `f` to each selected audio clip and its group partners, then
    /// rebuilds if any changed.
    fn each_transposed(&mut self, mut f: impl FnMut(&mut Clip) -> bool) {
        if !self.may_edit_song() {
            return;
        }
        let chosen = self.stretch_clips();
        let mut changed = false;
        for (l, id) in chosen {
            if let Some(c) = self.tracks[l].clips.iter_mut().find(|c| c.id == id) {
                changed |= f(c);
            }
        }
        if changed {
            self.rebuild();
        }
    }

    /// Sets the chosen clips' transpose, and whether it moves their
    /// formants.
    pub(crate) fn set_transpose(&mut self, cents: i64, move_formants: bool) {
        self.each_transposed(|c| {
            let moved = c.move_formants != move_formants;
            c.move_formants = move_formants;
            c.set_transpose(cents) | moved
        });
    }

    /// Moves the chosen clips' transpose by `cents` each.
    pub(crate) fn shift_transpose(&mut self, cents: i64) {
        self.each_transposed(|c| c.set_transpose(c.cents as i64 + cents));
    }

    /// The first selected audio clip's transpose and formant choice.
    fn selected_transpose(&self) -> Option<(i32, bool)> {
        let selected = &self.timeline.selected;
        (self.tracks.iter())
            .flat_map(|t| &t.clips)
            .find(|c| selected.contains(&c.id))
            .map(|c| (c.cents, c.move_formants))
    }

    /// Runs a transpose command; false if `cmd` isn't one.
    pub(crate) fn run_transpose(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::TransposeClips => {
                self.transpose.filled = false;
                self.transpose.open = true;
            }
            Cmd::TransposeUp => self.shift_transpose(100),
            Cmd::TransposeDown => self.shift_transpose(-100),
            Cmd::ResetTranspose => self.each_transposed(|c| c.set_transpose(0)),
            Cmd::MoveFormants => {
                // On if any chosen clip keeps its formants yet.
                let selected = &self.timeline.selected;
                let on = (self.tracks.iter())
                    .flat_map(|t| &t.clips)
                    .any(|c| selected.contains(&c.id) && !c.move_formants);
                self.each_transposed(|c| {
                    let was = c.move_formants;
                    c.move_formants = on;
                    was != on
                });
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn transpose_window(&mut self, ctx: &egui::Context) {
        if !self.transpose.open {
            return;
        }
        let may_edit = self.may_edit_song() && self.has_stretch_clips();
        if !self.transpose.filled
            && let Some((cents, formants)) = self.selected_transpose()
        {
            self.transpose.semitones = cents / 100;
            self.transpose.cents = cents % 100;
            self.transpose.move_formants = formants;
            self.transpose.filled = true;
        }
        let mut open = true;
        let mut set = false;
        let mut reset = false;
        let t = &mut self.transpose;
        egui::Window::new("Transpose clips")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Moves the selected clips' pitch without changing their length or \
                     timing; it works on clips that follow tempo too.",
                );
                if !may_edit {
                    ui.label(RichText::new("Select an audio clip first.").weak());
                    return;
                }
                ui.horizontal(|ui| {
                    let st = ui.label("Semitones");
                    // ai: edit_clip
                    ui.add(egui::DragValue::new(&mut t.semitones).range(-12..=12))
                        .labelled_by(st.id);
                    let ct = ui.label("Cents");
                    // ai: edit_clip
                    ui.add(egui::DragValue::new(&mut t.cents).range(-99..=99))
                        .labelled_by(ct.id);
                });
                // ai: move-formants
                ui.checkbox(&mut t.move_formants, "Move formants with the pitch")
                    .on_hover_text(
                        "Off keeps a voice sounding like the same person in the new key. \
                         On changes its character too, like tape played faster or slower.",
                    );
                let total = clamp(t.semitones as i64 * 100 + t.cents as i64);
                ui.label(RichText::new(format!("Transpose: {}", label(total))).weak());
                ui.horizontal(|ui| {
                    // ai: edit_clip
                    set = ui.button("Transpose").clicked();
                    // ai: reset-transpose
                    reset = ui.button("Back to its own pitch").clicked();
                });
            });
        let total = clamp(self.transpose.semitones as i64 * 100 + self.transpose.cents as i64);
        if set {
            self.set_transpose(total as i64, self.transpose.move_formants);
        }
        if reset {
            self.run_transpose(Cmd::ResetTranspose);
            self.transpose.semitones = 0;
            self.transpose.cents = 0;
        }
        if !open {
            self.transpose.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Track;
    use crate::agent::driver::Driver;
    use crate::timeline::tests::clip;

    #[test]
    fn labels_read_as_semitones_and_cents() {
        assert_eq!(label(0), "0 st");
        assert_eq!(label(300), "+3 st");
        assert_eq!(label(-600), "−6 st");
        assert_eq!(label(-620), "−6 st 20 ct");
        assert_eq!(label(15), "+15 ct");
        assert_eq!(clamp(5000), 1200);
        assert_eq!(clamp(-5000), -1200);
    }

    #[test]
    fn an_assistant_transposes_a_clip_and_it_is_saved_and_undone() {
        let mut d = Driver::new();
        let c = clip(0, 96_000);
        let key = c.key.clone();
        let app = d.app();
        app.tracks.push(Track::new("Vocal".into()));
        app.tracks[0].clips.push(c);
        app.rebuild();
        let v = d
            .call(
                "edit_clip",
                json!({"clip": key, "transpose_semitones": -6.2}),
            )
            .unwrap();
        assert_eq!(v["transpose_semitones"], -6.2);
        assert_eq!(v["move_formants"], false);
        assert_eq!(d.app().tracks[0].clips[0].cents, -620);
        assert_eq!(
            d.project()["tracks"][0]["clips"][0]["transpose_semitones"],
            -6.2
        );
        assert_eq!(d.app().project().tracks[0].clips[0].cents, -620);
        // Too far, and not a number.
        assert!(
            d.call("edit_clip", json!({"clip": key, "transpose_semitones": 13}))
                .is_err()
        );
        assert!(
            d.call(
                "edit_clip",
                json!({"clip": key, "transpose_semitones": "up"})
            )
            .is_err()
        );
        d.call("edit_clip", json!({"clip": key, "move_formants": true}))
            .unwrap();
        assert!(d.app().tracks[0].clips[0].move_formants);
        assert_eq!(d.app().tracks[0].clips[0].cents, -620, "kept");
        d.command("undo");
        assert!(!d.app().tracks[0].clips[0].move_formants);
        d.command("undo");
        assert_eq!(d.app().tracks[0].clips[0].cents, 0);
    }

    #[test]
    fn the_commands_act_on_the_selected_clips() {
        let mut d = Driver::new();
        let app = d.app();
        app.tracks.push(Track::new("Vocal".into()));
        app.tracks[0].clips.push(clip(0, 96_000));
        app.tracks[0].clips.push(clip(96_000, 96_000));
        app.rebuild();
        let id = app.tracks[0].clips[0].id;
        app.timeline.selected.insert(id);
        d.command("transpose-up");
        d.command("transpose-up");
        d.command("transpose-down");
        let clips = &d.app().tracks[0].clips;
        assert_eq!(clips[0].cents, 100);
        assert_eq!(clips[1].cents, 0, "not selected");
        for _ in 0..14 {
            d.command("transpose-up");
        }
        assert_eq!(d.app().tracks[0].clips[0].cents, 1200, "an octave at most");
        d.command("move-formants");
        assert!(d.app().tracks[0].clips[0].move_formants);
        d.command("move-formants");
        assert!(!d.app().tracks[0].clips[0].move_formants);
        d.command("reset-transpose");
        assert_eq!(d.app().tracks[0].clips[0].cents, 0);
    }

    #[test]
    fn a_transposed_clip_plays_through_the_stretcher_and_keeps_formants() {
        let mut c = clip(1_000, 96_000);
        let map = daw_engine::time::TempoMap::new(
            48_000,
            daw_engine::time::Tempo::bpm(120),
            daw_engine::time::TimeSignature::COMMON,
        );
        let plain = crate::clip_spec(&c, &map);
        assert_eq!((plain.semitones, plain.tag), (0.0, 0));
        c.set_transpose(-650);
        let spec = crate::clip_spec(&c, &map);
        assert_eq!(spec.semitones, -6.5);
        assert!(spec.formants);
        assert_eq!(spec.tag, c.id, "its stretcher carries on across rebuilds");
        c.move_formants = true;
        assert!(!crate::clip_spec(&c, &map).formants);
    }
}
