// SPDX-License-Identifier: GPL-3.0-or-later
//! Edit → Undo list… and Edit → Activity… (roadmap Phase 4): one window
//! with two views of the project's change history.
//!
//! The undo list shows this user's steps by what they changed, newest
//! at the top, with the steps Redo would put back above them, greyed.
//! Choosing a step goes back (or forward) to just after it, as that
//! many undos or redos, so each can still be redone on its own. The
//! activity view shows who changed what and when, everyone's changes
//! included, read from the document (see `daw_project::ProjectDoc::
//! activity`).
//!
//! From outside the window: [`App::undo_steps`] and [`App::redo_steps`]
//! take a count, and `ProjectDoc::undo_list`, `redo_list` and `activity`
//! read the lists.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daw_project::{Activity, UndoStep};
use eframe::egui::{self, RichText};

use crate::App;

/// Runs shown at first, and added by Show more.
const PAGE: usize = 30;

#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum View {
    #[default]
    Undo,
    Activity,
}

#[derive(Default)]
pub(crate) struct History {
    pub(crate) open: bool,
    pub(crate) view: View,
    /// How many runs of activity to show.
    shown: usize,
    /// The activity read at a version of the document, for that many
    /// runs.
    found: Option<(Vec<u8>, usize, Vec<Activity>)>,
}

/// What the user chose in the window.
enum Action {
    Undo(usize),
    Redo(usize),
}

impl History {
    pub(crate) fn show(&mut self, view: View) {
        self.open = true;
        self.view = view;
    }

    fn window(
        &mut self,
        ctx: &egui::Context,
        undo: &[UndoStep],
        redo: &[UndoStep],
        now_ms: i64,
    ) -> Option<Action> {
        let mut open = self.open;
        let mut action = None;
        egui::Window::new("Change history")
            .open(&mut open)
            .default_pos([320.0, 120.0])
            .default_size([440.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // ai: view
                    ui.selectable_value(&mut self.view, View::Undo, "Undo list");
                    // ai: view
                    ui.selectable_value(&mut self.view, View::Activity, "Activity");
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.view {
                    View::Undo => action = undo_list(ui, undo, redo, now_ms),
                    View::Activity => self.activity(ui, now_ms),
                });
            });
        self.open = open;
        action
    }

    fn activity(&mut self, ui: &mut egui::Ui, now_ms: i64) {
        ui.label("Who changed what, and when, everyone's changes included.");
        let Some((_, _, runs)) = &self.found else {
            return;
        };
        if runs.is_empty() {
            ui.label(RichText::new("No changes yet.").weak());
        }
        for (i, r) in runs.iter().enumerate() {
            ui.separator();
            let who = if r.mine {
                format!("{} (you)", r.who)
            } else {
                r.who.clone()
            };
            let when = match r.from {
                0 => "time not recorded".to_string(),
                t => crate::project::ago(age(now_ms, t * 1000)),
            };
            let head = ui.label(RichText::new(format!("{who} · {when}")).strong());
            if r.from > 0 {
                head.on_hover_text(match r.to > r.from {
                    true => format!(
                        "{} to {} UTC",
                        crate::crash::utc(r.from as u64),
                        crate::crash::utc(r.to as u64)
                    ),
                    false => format!("{} UTC", crate::crash::utc(r.from as u64)),
                });
            }
            let few = 4;
            for d in r.changes.iter().take(few) {
                ui.label(format!("{}: {}", d.place, d.text));
            }
            if r.changes.len() > few {
                egui::CollapsingHeader::new(format!("{} more", r.changes.len() - few))
                    .id_salt(("activity-more", i, r.from))
                    .show(ui, |ui| {
                        for d in &r.changes[few..] {
                            ui.label(format!("{}: {}", d.place, d.text));
                        }
                    });
            }
        }
        if runs.len() >= self.shown {
            ui.separator();
            // ai: view
            if ui.button("Show more").clicked() {
                self.shown += PAGE;
            }
        }
    }

    /// Reads the activity again if the document changed, or more was
    /// asked for.
    fn find(&mut self, doc: &daw_project::ProjectDoc) {
        if self.view != View::Activity {
            return;
        }
        self.shown = self.shown.max(PAGE);
        let version = doc.version();
        if let Some((v, n, _)) = &self.found
            && *v == version
            && *n == self.shown
        {
            return;
        }
        let runs = doc.activity(self.shown);
        self.found = Some((version, self.shown, runs));
    }
}

/// The undo list: the redo steps, furthest first, greyed; where the
/// project is now; then the undo steps, newest first.
fn undo_list(
    ui: &mut egui::Ui,
    undo: &[UndoStep],
    redo: &[UndoStep],
    now_ms: i64,
) -> Option<Action> {
    let mut action = None;
    ui.label("Your changes, newest at the top. Choose one to go back to just after it.");
    if undo.is_empty() && redo.is_empty() {
        ui.label(RichText::new("Nothing to undo yet.").weak());
        return None;
    }
    ui.add_space(4.0);
    for (j, s) in redo.iter().enumerate().rev() {
        let text = RichText::new(&s.label).weak();
        if step_row(ui, s, text, now_ms, "Redo up to here").clicked() {
            action = Some(Action::Redo(j + 1));
        }
    }
    ui.label(RichText::new("Now").strong().underline());
    for (i, s) in undo.iter().enumerate() {
        let text = RichText::new(&s.label);
        if step_row(ui, s, text, now_ms, "Undo back to here").clicked() && i > 0 {
            action = Some(Action::Undo(i));
        }
    }
    let start = ui
        // ai: undo_steps.undo
        .selectable_label(false, RichText::new("Before these changes").italics())
        .on_hover_text("Undo every step on the list");
    if start.clicked() {
        action = Some(Action::Undo(undo.len()));
    }
    action
}

fn step_row(
    ui: &mut egui::Ui,
    s: &UndoStep,
    text: RichText,
    now_ms: i64,
    hint: &str,
) -> egui::Response {
    let r = ui
        .horizontal(|ui| {
            // ai: undo_steps.undo
            let r = ui.selectable_label(false, text);
            if s.at > 0 {
                ui.label(RichText::new(crate::project::ago(age(now_ms, s.at))).weak());
            }
            r
        })
        .inner;
    let mut tip = String::from(hint);
    for d in &s.changes {
        tip.push_str(&format!("\n{}: {}", d.place, d.text));
    }
    r.on_hover_text(tip)
}

fn age(now_ms: i64, at_ms: i64) -> Duration {
    Duration::from_millis(now_ms.saturating_sub(at_ms).max(0) as u64)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A step as the agent interface gives it.
fn step_value(s: &UndoStep) -> serde_json::Value {
    serde_json::json!({
        "label": s.label,
        "at_ms": s.at,
        "changes": s.changes.iter().map(|d| format!("{}: {}", d.place, d.text)).collect::<Vec<_>>(),
    })
}

impl App {
    /// The undo and redo lists and the last `runs` runs of activity, for
    /// the agent interface's `get_history`.
    pub(crate) fn history_value(&self, runs: usize) -> serde_json::Value {
        let doc = &self.proj.doc;
        let activity: Vec<serde_json::Value> = (doc.activity(runs).iter())
            .map(|a| {
                serde_json::json!({
                    "who": a.who,
                    "mine": a.mine,
                    "from": a.from,
                    "to": a.to,
                    "changes": a.changes.iter().map(|d| format!("{}: {}", d.place, d.text)).collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::json!({
            "undo": doc.undo_list().iter().map(step_value).collect::<Vec<_>>(),
            "redo": doc.redo_list().iter().map(step_value).collect::<Vec<_>>(),
            "activity": activity,
        })
    }

    pub(crate) fn history_window(&mut self, ctx: &egui::Context) {
        if !self.history.open {
            return;
        }
        self.history.find(&self.proj.doc);
        let (undo, redo) = (self.proj.song().undo_list(), self.proj.song().redo_list());
        match self.history.window(ctx, &undo, &redo, now_ms()) {
            Some(Action::Undo(n)) => {
                self.undo_steps(ctx, n);
            }
            Some(Action::Redo(n)) => {
                self.redo_steps(ctx, n);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::View;
    use crate::App;
    use crate::commands::Cmd;

    fn app() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    fn tempo(h: &mut Harness<'_, App>, bpm: f64) {
        h.state_mut().tempo_bpm = bpm;
        h.run();
    }

    #[test]
    fn the_undo_list_goes_back_several_steps_and_forward_again() {
        let mut h = app();
        for bpm in [90.0, 100.0, 110.0] {
            tempo(&mut h, bpm);
        }
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::History);
        h.run();
        h.get_by_label("Tempo 90 → 100 BPM").click();
        h.run();
        assert_eq!(h.state().tempo_bpm, 100.0);
        assert_eq!(h.state().proj.doc.redo_list().len(), 1);
        h.get_by_label("Before these changes").click();
        h.run();
        assert_eq!(h.state().tempo_bpm, 120.0);
        h.get_by_label("Tempo 100 → 110 BPM").click();
        h.run();
        assert_eq!(h.state().tempo_bpm, 110.0);
        assert!(h.state().proj.doc.redo_list().is_empty());
    }

    #[test]
    fn steps_can_be_taken_back_without_the_window() {
        let mut h = app();
        for bpm in [90.0, 100.0, 110.0] {
            tempo(&mut h, bpm);
        }
        let ctx = h.ctx.clone();
        assert_eq!(h.state_mut().undo_steps(&ctx, 2), 2);
        assert_eq!(h.state().tempo_bpm, 90.0);
        assert_eq!(h.state_mut().redo_steps(&ctx, 5), 2);
        assert_eq!(h.state().tempo_bpm, 110.0);
    }

    #[test]
    fn an_assistant_reads_the_undo_list_and_goes_back_through_it() {
        use serde_json::json;

        let mut d = crate::agent::driver::Driver::new();
        d.command("add-instrument-track");
        let key = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        for v in [-6.0, -3.0] {
            d.call(
                "set_parameter",
                json!({"track": key, "param": "gain", "value": v}),
            )
            .unwrap();
        }
        let h = d.call("get_history", json!({})).unwrap();
        assert_eq!(h["undo"].as_array().unwrap().len(), 3, "{h}");
        assert!(h["redo"].as_array().unwrap().is_empty());
        assert_eq!(h["activity"][0]["mine"], true);
        d.call("undo_steps", json!({"undo": 2})).unwrap();
        assert_eq!(d.project()["tracks"][0]["gain_db"], 0.0);
        let h = d.call("get_history", json!({})).unwrap();
        assert_eq!(h["redo"].as_array().unwrap().len(), 2);
        d.call("undo_steps", json!({"redo": 1})).unwrap();
        assert_eq!(d.project()["tracks"][0]["gain_db"], -6.0);
        assert!(d.call("undo_steps", json!({})).is_err());
    }

    #[test]
    fn activity_shows_what_changed() {
        let mut h = app();
        tempo(&mut h, 96.0);
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::Activity);
        h.run();
        assert!(h.state().history.view == View::Activity);
        h.get_by_label("Song: Tempo 120 → 96 BPM");
    }
}
