// SPDX-License-Identifier: GPL-3.0-or-later
//! Snapshots (roadmap 1.8): File → Snapshots… names the project as it is
//! now ("Lesson 3 end") and puts it back that way later. Restoring is an
//! ordinary edit, so Undo takes it back and it merges with other
//! people's changes; comments stay as they are (see
//! `daw_project::ProjectDoc::restore_snapshot`).
//!
//! What changed… on a snapshot lists, in words, what changed from it to
//! the project now (what a restore would undo), or to another snapshot
//! (roadmap 2.8). Anyone may look, commenters included.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daw_project::{Difference, MAX_SNAPSHOT_NAME, Project, Snapshot};
use eframe::egui::{self, RichText};

use crate::App;

#[derive(Default)]
pub(crate) struct Snapshots {
    pub(crate) open: bool,
    /// The name for the next snapshot.
    draft: String,
    /// A snapshot being renamed, and its name so far.
    renaming: Option<(String, String)>,
    /// A snapshot whose Restore or Delete was pressed, waiting for yes.
    asked: Option<(String, Ask)>,
    /// The two versions being compared: a snapshot, and another or the
    /// project now (`None`).
    compare: Option<(String, Option<String>)>,
    /// What changed between them, worked out for that pair at a version
    /// of the document; `None` if they can't be read from this copy.
    found: Option<Found>,
}

struct Found {
    pair: (String, Option<String>),
    version: Vec<u8>,
    changes: Option<Vec<Difference>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Restore,
    Delete,
}

/// What the user chose in the window.
enum Action {
    Take(String),
    Restore(String),
    Rename(String, String),
    Delete(String),
}

impl Snapshots {
    /// The window. `may_edit` is false for commenters, who may look but
    /// not take or restore.
    fn window(
        &mut self,
        ctx: &egui::Context,
        list: &[Snapshot],
        who: impl Fn(&str) -> Option<String>,
        now_ms: i64,
        may_edit: bool,
    ) -> Option<Action> {
        let mut open = self.open;
        let changes = self.found.as_ref().map(|f| f.changes.clone());
        let mut action = None;
        egui::Window::new("Snapshots")
            .open(&mut open)
            .default_pos([300.0, 140.0])
            .default_size([420.0, 480.0])
            .show(ctx, |ui| {
                ui.label("Name the project as it is now, to come back to it later.");
                ui.add_enabled_ui(may_edit, |ui| {
                    ui.horizontal(|ui| {
                        let l = ui.label("Name");
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.draft)
                                    .char_limit(MAX_SNAPSHOT_NAME)
                                    .hint_text("e.g. Lesson 3 end")
                                    .desired_width(220.0),
                            )
                            .labelled_by(l.id);
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Take snapshot").clicked() || enter {
                            let name = match self.draft.trim() {
                                "" => format!("Snapshot {}", list.len() + 1),
                                n => n.to_string(),
                            };
                            action = Some(Action::Take(name));
                            self.draft.clear();
                        }
                    });
                });
                if !may_edit {
                    ui.label(
                        RichText::new("Only the owner and editors can take or restore snapshots.")
                            .weak(),
                    );
                }
                ui.separator();
                if let Some(changes) = &changes {
                    self.comparison(ui, list, changes.as_deref());
                    ui.separator();
                }
                if list.is_empty() {
                    ui.label(RichText::new("No snapshots yet.").weak());
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for s in list {
                        if let Some(a) = self.row(ui, s, &who, now_ms, may_edit) {
                            action = Some(a);
                        }
                        ui.separator();
                    }
                });
            });
        self.open = open;
        action
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        s: &Snapshot,
        who: &impl Fn(&str) -> Option<String>,
        now_ms: i64,
        may_edit: bool,
    ) -> Option<Action> {
        let mut action = None;
        if let Some((id, name)) = &mut self.renaming
            && *id == s.id
        {
            let mut done = false;
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(name)
                        .char_limit(MAX_SNAPSHOT_NAME)
                        .desired_width(240.0),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("OK").clicked() || enter) && !name.trim().is_empty() {
                    action = Some(Action::Rename(s.id.clone(), name.clone()));
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
            if done {
                self.renaming = None;
            }
            return action;
        }
        ui.label(RichText::new(&s.name).strong());
        let age = Duration::from_millis(now_ms.saturating_sub(s.created).max(0) as u64);
        let mut info = crate::project::ago(age);
        if let Some(name) = who(&s.author) {
            info = format!("{info} · {name}");
        }
        ui.label(RichText::new(info).weak()).on_hover_text(format!(
            "{} UTC",
            crate::crash::utc(s.created.max(0) as u64 / 1000)
        ));
        if !s.restorable {
            ui.label(RichText::new("Can't be restored from this copy of the project.").weak());
        }
        match self.asked.clone() {
            Some((id, ask)) if id == s.id => {
                ui.label(match ask {
                    Ask::Restore => {
                        "Put the project back as it was then? Comments stay as they are, \
                         and Undo takes this back."
                    }
                    Ask::Delete => {
                        "Delete this snapshot? The project stays as it is; only the name goes."
                    }
                });
                ui.horizontal(|ui| {
                    let yes = match ask {
                        Ask::Restore => "Restore",
                        Ask::Delete => "Delete",
                    };
                    if ui.button(yes).clicked() {
                        action = Some(match ask {
                            Ask::Restore => Action::Restore(s.id.clone()),
                            Ask::Delete => Action::Delete(s.id.clone()),
                        });
                        self.asked = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.asked = None;
                    }
                });
            }
            _ => {
                ui.add_enabled_ui(may_edit, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(s.restorable, egui::Button::new("Restore…"))
                            .clicked()
                        {
                            self.asked = Some((s.id.clone(), Ask::Restore));
                        }
                        if ui.button("Rename").clicked() {
                            self.renaming = Some((s.id.clone(), s.name.clone()));
                        }
                        if ui.button("Delete…").clicked() {
                            self.asked = Some((s.id.clone(), Ask::Delete));
                        }
                    });
                });
                if ui
                    .add_enabled(s.restorable, egui::Button::new("What changed…"))
                    .on_hover_text("What has changed since this snapshot")
                    .clicked()
                {
                    self.compare = Some((s.id.clone(), None));
                }
            }
        }
        action
    }

    /// The two versions being compared, which can be changed, and what
    /// changed between them.
    fn comparison(&mut self, ui: &mut egui::Ui, list: &[Snapshot], changes: Option<&[Difference]>) {
        let Some((from, to)) = &mut self.compare else {
            return;
        };
        let name = |id: &str| {
            (list.iter())
                .find(|s| s.id == id)
                .map_or("(deleted)".to_string(), |s| s.name.clone())
        };
        let mut close = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Changes from");
            egui::ComboBox::from_id_salt("compare from")
                .selected_text(name(from))
                .show_ui(ui, |ui| {
                    for s in list.iter().filter(|s| s.restorable) {
                        ui.selectable_value(from, s.id.clone(), &s.name);
                    }
                })
                .response
                .accessible_name("Compare from snapshot");
            ui.label("to");
            let to_name = to.as_deref().map_or("now".to_string(), name);
            egui::ComboBox::from_id_salt("compare to")
                .selected_text(to_name)
                .show_ui(ui, |ui| {
                    ui.selectable_value(to, None, "now");
                    for s in list.iter().filter(|s| s.restorable) {
                        ui.selectable_value(to, Some(s.id.clone()), &s.name);
                    }
                })
                .response
                .accessible_name("Compare to");
            close = ui.button("Close").clicked();
        });
        match changes {
            None => {
                ui.label(
                    RichText::new("Can't compare these from this copy of the project.").weak(),
                );
            }
            Some([]) => {
                ui.label(RichText::new("Nothing has changed.").weak());
            }
            Some(changes) => {
                egui::ScrollArea::vertical()
                    .id_salt("changes")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        let mut place = None;
                        for d in changes {
                            if place != Some(&d.place) {
                                ui.label(RichText::new(&d.place).strong());
                                place = Some(&d.place);
                            }
                            ui.label(format!("  {}", d.text));
                        }
                    });
            }
        }
        if close {
            self.compare = None;
            self.found = None;
        }
    }

    /// Works out what changed between the pair being compared, unless
    /// that is already known for this version of the document.
    fn find_changes(&mut self, doc: &daw_project::ProjectDoc) {
        let Some(pair) = &self.compare else {
            self.found = None;
            return;
        };
        let version = doc.version();
        if let Some(f) = &self.found
            && f.pair == *pair
            && f.version == version
        {
            return;
        }
        let changes = doc.snapshot_differences(&pair.0, pair.1.as_deref());
        self.found = Some(Found {
            pair: pair.clone(),
            version,
            changes,
        });
    }
}

impl App {
    pub(crate) fn snapshots_window(&mut self, ctx: &egui::Context) {
        if !self.snapshot_ui.open {
            return;
        }
        let ui = &self.snapshot_ui;
        if ui.compare.is_some() && ui.found.as_ref().map(|f| &f.pair) != ui.compare.as_ref() {
            // Plugin settings as they are now, for a new comparison.
            self.write_plugin_states();
        }
        self.snapshot_ui.find_changes(&self.proj.doc);
        let list = self.proj.doc.snapshots();
        let may_edit = self.may_edit_song();
        let doc = &self.proj.doc;
        let me = doc.my_key().unwrap_or_default().to_string();
        let who = |key: &str| match key {
            "" => None,
            k if k == me => Some("you".to_string()),
            k => doc.name_of(k),
        };
        let action = self.snapshot_ui.window(ctx, &list, who, now_ms(), may_edit);
        match action {
            Some(Action::Take(name)) => {
                self.take_snapshot(ctx, &name);
            }
            Some(Action::Restore(id)) => {
                if let Err(e) = self.restore_snapshot(ctx, &id) {
                    self.message = Some(e);
                }
            }
            Some(Action::Rename(id, name)) => {
                self.proj.doc.rename_snapshot(&id, &name);
                self.proj.dirty = true;
            }
            Some(Action::Delete(id)) => {
                self.proj.doc.delete_snapshot(&id);
                self.proj.dirty = true;
            }
            None => {}
        }
    }

    /// Names the project as it is now, plugin settings included.
    /// Returns the snapshot's id.
    pub(crate) fn take_snapshot(&mut self, ctx: &egui::Context, name: &str) -> String {
        self.sync_doc(ctx, true);
        self.write_plugin_states();
        let me = self.my_author_id();
        let id = self.proj.doc.take_snapshot(name, &me, now_ms());
        self.proj.dirty = true;
        id
    }

    /// Puts the project back as it was at snapshot `id`, as one undoable
    /// edit.
    pub(crate) fn restore_snapshot(&mut self, ctx: &egui::Context, id: &str) -> Result<(), String> {
        self.sync_doc(ctx, true);
        // Plugins as they are now, so Undo can bring their settings back.
        self.write_plugin_states();
        let before = self.proj.doc.get();
        let p = (self.proj.doc.restore_snapshot(id))
            .ok_or("This snapshot can't be restored from this copy of the project.")?;
        self.reload_changed_fx(&before, &p);
        self.apply(p);
        self.proj.dirty = true;
        Ok(())
    }

    /// Lets go of live plugins whose settings differ between `before`
    /// and `after`, so that [`apply`](App::apply) loads them afresh with
    /// `after`'s.
    pub(crate) fn reload_changed_fx(&mut self, before: &Project, after: &Project) {
        for pt in &after.tracks {
            let Some(t) = self.tracks.iter_mut().find(|t| t.key == pt.id) else {
                continue;
            };
            let was = before.tracks.iter().find(|b| b.id == pt.id);
            for pe in &pt.effects {
                let Some(state) = &pe.state else { continue };
                let old = was
                    .and_then(|b| b.effects.iter().find(|e| e.id == pe.id))
                    .and_then(|e| e.state.as_ref());
                if old != Some(state) {
                    t.fx.retain(|s| s.key != pe.id);
                }
            }
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::App;

    fn app() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    fn add_marker(h: &mut Harness<'_, App>, name: &str, bar: i64) {
        h.state_mut().markers.push(crate::timeline::Marker {
            id: daw_project::new_id(),
            bar,
            name: name.into(),
        });
        h.run();
    }

    #[test]
    fn a_snapshot_taken_in_the_window_restores_and_undoes() {
        let mut h = app();
        add_marker(&mut h, "Intro", 1);
        h.state_mut().tempo_bpm = 96.0;
        h.run();
        h.state_mut().snapshot_ui.open = true;
        h.run();
        h.get_by_label("Name").focus();
        h.run();
        h.get_by_label("Name").type_text("Lesson 3 end");
        h.run();
        h.get_by_label("Take snapshot").click();
        h.run();
        let list = h.state().proj.doc.snapshots();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Lesson 3 end");

        // Then the lesson moves on.
        add_marker(&mut h, "Solo", 9);
        h.state_mut().tempo_bpm = 120.0;
        h.run();

        h.get_by_label("Restore…").click();
        h.run();
        h.get_by_label("Restore").click();
        h.run();
        let app = h.state();
        assert_eq!(app.tempo_bpm, 96.0);
        assert_eq!(
            app.markers
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["Intro"]
        );
        assert!(app.proj.dirty);

        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        let app = h.state();
        assert_eq!(app.tempo_bpm, 120.0, "undo takes the restore back");
        assert_eq!(app.markers.len(), 2);
        assert_eq!(app.proj.doc.snapshots().len(), 1, "the snapshot stays");
    }

    #[test]
    fn changes_since_a_snapshot_are_listed_in_words() {
        let mut h = app();
        h.state_mut().tempo_bpm = 96.0;
        h.run();
        let ctx = h.ctx.clone();
        h.state_mut().take_snapshot(&ctx, "Lesson 3 end");
        add_marker(&mut h, "Solo", 9);
        h.state_mut().tempo_bpm = 120.0;
        h.run();
        h.state_mut().snapshot_ui.open = true;
        h.run();
        h.get_by_label("What changed…").click();
        h.run();
        h.get_by_label_contains("Tempo 96 → 120 BPM");
        h.get_by_label_contains("Added “Solo” at bar 9");

        // Undone, the list follows.
        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        h.run();
        assert!(h.query_by_label_contains("Tempo 96").is_none());
        h.get_by_label_contains("Added “Solo” at bar 9");
        h.get_by_label("Close").click();
        h.run();
        assert!(h.query_by_label_contains("Added “Solo”").is_none());
    }

    #[test]
    fn delete_asks_first() {
        let mut h = app();
        let ctx = h.ctx.clone();
        h.state_mut().take_snapshot(&ctx, "Before");
        h.state_mut().snapshot_ui.open = true;
        h.run();
        h.get_by_label("Delete…").click();
        h.run();
        assert_eq!(h.state().proj.doc.snapshots().len(), 1);
        h.get_by_label("Delete").click();
        h.run();
        assert!(h.state().proj.doc.snapshots().is_empty());
    }
}
