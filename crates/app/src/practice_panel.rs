// SPDX-License-Identifier: GPL-3.0-or-later
//! The Practice panel (UI polish plan, section 8): a tab of the right
//! panel that gathers what a lesson needs. From the top: the tuner's
//! reading, speed and pitch, the loop and the speed trainer, this
//! week's assignments, the lesson notes, and buttons for the fretboard
//! and the notation lane.
//!
//! Nothing here is new: each control runs the command or changes the
//! setting its own window does, so assistants reach it the same way
//! (`set_practice`, `set_loop`, `edit_assignments`, the commands). The
//! tuner listens while the panel shows, as it does while its own pane
//! is open; `get_selection` gives what the panel shows under
//! `practice_panel`.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::App;
use crate::commands::Cmd;
use crate::workspace::Pane;

/// The lesson notes' lines shown before Open lesson notes.
const NOTE_LINES: usize = 6;

#[derive(Default)]
pub(crate) struct PracticePanel {
    /// The tuner's reading was drawn this frame.
    pub(crate) drawn: bool,
}

impl App {
    /// The Practice panel.
    pub(crate) fn practice_pane(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("practice-pane")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.practice_tuner(ui);
                self.practice_speed(ui);
                self.practice_loop(ui);
                section(ui, "this week", "This week", |ui| self.week_assignments(ui));
                self.practice_notes(ui);
                self.practice_views(ui);
            });
    }

    fn practice_tuner(&mut self, ui: &mut egui::Ui) {
        section(ui, "tuner", "Tuner", |ui| {
            self.practice_ui.drawn = true;
            self.tuner.compact(ui);
            // ai: tuner
            let r = ui.small_button("Input and reference pitch…");
            if r.on_hover_text("Open the tuner's own tab").clicked() {
                self.run_after_panes(ui, Cmd::Tuner);
            }
        });
    }

    fn practice_speed(&mut self, ui: &mut egui::Ui) {
        section(ui, "speed", "Speed and pitch", |ui| {
            let before = (self.speed_pct, self.semitones, self.cents);
            ui.horizontal_wrapped(|ui| {
                ui.label("Speed");
                self.speed_row(ui);
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Transpose");
                self.pitch_row(ui);
            });
            let shown = crate::top_bar::TUNINGS
                .iter()
                .find(|(_, st)| (self.semitones, self.cents) == (*st, 0))
                .map_or("Your own", |(name, _)| *name);
            ui.horizontal(|ui| {
                ui.label("Tuning");
                egui::ComboBox::from_id_salt("practice tuning")
                    .selected_text(shown)
                    .width((ui.available_width() - 24.0).max(60.0))
                    .show_ui(ui, |ui| {
                        for (label, st) in crate::top_bar::TUNINGS {
                            let on = (self.semitones, self.cents) == (st, 0);
                            // ai: set_practice
                            if ui.selectable_label(on, label).clicked() {
                                self.semitones = st;
                                self.cents = 0;
                            }
                        }
                    })
                    .response
                    .accessible_name("Retune a recording to play along");
            });
            self.back_to_normal(ui);
            if (self.speed_pct, self.semitones, self.cents) != before {
                let c = self.practice();
                self.send(c);
            }
        });
    }

    fn practice_loop(&mut self, ui: &mut egui::Ui) {
        section(ui, "loop", "Loop and trainer", |ui| {
            ui.horizontal(|ui| {
                // ai: loop
                let r = ui
                    .add(egui::Button::selectable(self.looping, "Loop"))
                    .accessible_name("Loop, from the practice panel");
                if r.on_hover_text(self.commands.hover(Cmd::Loop)).clicked() {
                    self.run_after_panes(ui, Cmd::Loop);
                }
            });
            self.loop_options(ui);
            ui.horizontal_wrapped(|ui| self.trainer.controls(ui));
            if self.trainer.on && !self.looping {
                ui.label(
                    RichText::new("The trainer speeds up each time round the loop: turn Loop on.")
                        .weak(),
                );
            }
        });
    }

    fn practice_notes(&mut self, ui: &mut egui::Ui) {
        section(ui, "notes", "Lesson notes", |ui| {
            let lines: Vec<&str> = (self.notes.lines())
                .map(str::trim_end)
                .filter(|l| !l.trim().is_empty() && l.trim() != "```")
                .collect();
            if lines.is_empty() {
                ui.label(RichText::new("No lesson notes yet.").weak());
            }
            for l in lines.iter().take(NOTE_LINES) {
                let plain = l.trim_start_matches('#').trim_start().replace("**", "");
                ui.add(egui::Label::new(plain).truncate());
            }
            if lines.len() > NOTE_LINES {
                ui.label(
                    RichText::new(format!("and {} more lines", lines.len() - NOTE_LINES)).weak(),
                );
            }
            let label = if lines.is_empty() {
                "Write lesson notes"
            } else {
                "Open lesson notes"
            };
            // ai: lesson-notes
            if ui.small_button(label).clicked() {
                self.run_after_panes(ui, Cmd::LessonNotes);
            }
            // ai: lessons
            let r = ui.small_button("All assignments and the practice log");
            if r.on_hover_text("Open the Lessons tab").clicked() {
                self.run_after_panes(ui, Cmd::Lessons);
            }
        });
    }

    fn practice_views(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let fretboard = self.workspace.layout.is_open(Pane::Fretboard);
            // ai: fretboard
            let r = ui.add(egui::Button::selectable(fretboard, "Fretboard"));
            if r.on_hover_text(self.commands.hover(Cmd::Fretboard))
                .clicked()
            {
                self.run_after_panes(ui, Cmd::Fretboard);
            }
            let notation = self
                .notation_track()
                .is_some_and(|i| self.tracks[i].notation.is_some());
            // ai: notation-lane
            let r = ui.add(egui::Button::selectable(notation, "Notation"));
            if r.on_hover_text(self.commands.hover(Cmd::NotationLane))
                .clicked()
            {
                self.run_after_panes(ui, Cmd::NotationLane);
            }
        });
    }

    /// Runs `cmd` from the panel; those that change the panes wait
    /// until the panes are drawn (see `App::run`).
    fn run_after_panes(&mut self, ui: &egui::Ui, cmd: Cmd) {
        let ctx = ui.ctx().clone();
        self.run(&ctx, cmd);
    }

    /// What the practice panel shows, for `get_selection`.
    pub(crate) fn practice_panel_value(&mut self) -> Value {
        let tuner = match self.tuner.reading() {
            Some((n, hz)) => json!({
                "listening": self.tuner.listening(),
                "note": format!("{}{}", n.name(), n.octave()),
                "cents": (n.cents * 10.0).round() / 10.0,
                "hz": (hz * 100.0).round() / 100.0,
            }),
            None => json!({"listening": self.tuner.listening(), "note": null}),
        };
        json!({
            "open": self.workspace.layout.is_open(Pane::Practice),
            "tuner": tuner,
            "this_week": self.week_assignment_ids(),
        })
    }
}

/// A part of the panel under a heading that folds away.
fn section(ui: &mut egui::Ui, salt: &str, heading: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(heading).strong())
        .id_salt(("practice section", salt))
        .default_open(true)
        .show(ui, add);
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;
    use serde_json::json;

    use crate::agent::driver::Driver;
    use crate::workspace::{Pane, Region};

    #[test]
    fn an_assistant_opens_the_practice_panel_and_reads_it() {
        let mut d = Driver::new();
        let panel =
            |d: &mut Driver| d.call("get_selection", json!({})).unwrap()["practice_panel"].clone();
        assert_eq!(panel(&mut d)["open"], false);
        // Its command opens it as a tab of the right panel, shown.
        d.command("practice-panel");
        d.h.run();
        assert_eq!(panel(&mut d)["open"], true);
        let p = d.call("get_selection", json!({})).unwrap()["panels"].clone();
        assert_eq!(p["right"]["shown"], "practice");
        assert_eq!(
            d.app().workspace.layout.region_of(Pane::Practice),
            Some(Region::Right)
        );
        // The tuner listens while it shows.
        d.h.run();
        assert_eq!(panel(&mut d)["tuner"]["listening"], true);
        // An assignment set through the assistant interface is this
        // week's, and the person sees it in the panel.
        d.call(
            "edit_assignments",
            json!({"add": [{"from_bar": 1, "to_bar": 4, "bpm": 90, "due_days": 3, "note": "Clean changes"}]}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(panel(&mut d)["this_week"].as_array().unwrap().len(), 1);
        d.h.get_by_label("Practise");
        d.h.get_by_label("Clean changes");
        // Speed set by the assistant shows in the panel; the person's
        // click on 75% reaches the assistant.
        d.call("set_practice", json!({"speed_pct": 60})).unwrap();
        d.h.run();
        d.h.get_by_label("75%").click();
        d.h.run();
        let t = d.call("get_transport", json!({})).unwrap();
        assert_eq!(t["speed_pct"], 75.0);
        // Loop from the panel.
        d.h.get_by_label("Loop, from the practice panel").click();
        d.h.run();
        assert_eq!(d.call("get_transport", json!({})).unwrap()["loop"], true);
        // The fretboard button opens its pane.
        d.h.get_by_label("Fretboard").click();
        d.h.run();
        d.h.run();
        assert!(d.app().workspace.layout.is_open(Pane::Fretboard));
        // Closed by the assistant, the tuner stops listening.
        d.call("set_pane", json!({"pane": "practice", "open": false}))
            .unwrap();
        d.h.run();
        d.h.run();
        let v = panel(&mut d);
        assert_eq!(v["open"], false);
        assert_eq!(v["tuner"]["listening"], false);
    }
}
