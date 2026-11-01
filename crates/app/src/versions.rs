// SPDX-License-Identifier: GPL-3.0-or-later
//! Track versions (roadmap Phase 4): other versions of what a track
//! plays and how it sounds (its clips, note clips, comps, effects,
//! instrument, automation and fader), kept side by side and switched in
//! one click. The track's name, routing and notation lane are shared.
//!
//! Every change goes through the project as plain values
//! ([`daw_project::Track::switch_version`] and the rest) and is one
//! undoable edit; the document keeps each version where it was written,
//! so switching writes one value and merges cleanly with someone editing
//! the version that was playing (see `daw_project`'s `doc.rs`).

use eframe::egui::{self, RichText};

use crate::App;

/// The longest version name kept.
const MAX_NAME: usize = 60;

/// The versions window.
#[derive(Default)]
pub(crate) struct Versions {
    pub(crate) open: bool,
    /// The track it shows, by key, while it is open (or a track's menu
    /// on the timeline is being acted on): the one version commands act
    /// on.
    pub(crate) track: Option<String>,
    /// A version being renamed, and its name so far.
    renaming: Option<(String, String)>,
    /// A version whose Delete was pressed, waiting for yes.
    asked: Option<String>,
}

/// What can be asked of a track's versions from its menu on the
/// timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VersionAsk {
    /// Play the version at this place in the list.
    Switch(usize),
    New,
    Duplicate,
    Window,
}

/// What the user chose in the window.
enum Action {
    Track(String),
    Switch(String),
    New,
    Duplicate,
    Rename(String, String),
    Delete(String),
}

/// A name for a new version not already used on the track.
fn next_name(t: &daw_project::Track) -> String {
    let mut n = t.versions.len().max(1) + 1;
    loop {
        let name = format!("Version {n}");
        if !t.versions.iter().any(|v| v.name == name) {
            return name;
        }
        n += 1;
    }
}

impl App {
    /// The track version commands act on: the versions window's, else
    /// the one with a selected clip or note clip, else the instrument
    /// window's, else the first with versions, else the first that holds
    /// clips.
    pub(crate) fn version_track(&self) -> Option<usize> {
        let by_key = |k: &str| self.tracks.iter().position(|t| t.key == k);
        let sel = (self.tracks.iter()).position(|t| {
            t.clips
                .iter()
                .any(|c| self.timeline.selected.contains(&c.id))
                || (self.timeline.note_sel.as_ref())
                    .is_some_and(|k| t.note_clips.iter().any(|n| &n.id == k))
        });
        (self.versions_ui.track.as_deref().and_then(by_key))
            .or(sel)
            .or(self.instrument_open.filter(|&i| i < self.tracks.len()))
            .or_else(|| self.tracks.iter().position(|t| !t.versions.is_empty()))
            .or_else(|| self.tracks.iter().position(|t| !t.kind.is_bus()))
    }

    /// Changes the versions of track `ti` with `f`, as one undoable edit,
    /// and makes the app play what results. False (with a message saying
    /// why, where there is one) if nothing changed.
    pub(crate) fn edit_versions(
        &mut self,
        ctx: &egui::Context,
        ti: usize,
        f: impl FnOnce(&mut daw_project::Track) -> bool,
    ) -> bool {
        let Some(key) = self.tracks.get(ti).map(|t| t.key.clone()) else {
            return false;
        };
        if self.recording {
            self.message = Some("Stop recording before changing a track's version.".into());
            return false;
        }
        if self.freezer.making(&key) {
            self.message = Some("Wait for the track to finish freezing first.".into());
            return false;
        }
        self.sync_doc(ctx, true);
        // Plugins as they are now, so the version left keeps their
        // settings.
        self.write_plugin_states();
        let before = self.proj.doc.get();
        let mut p = before.clone();
        let Some(t) = p.tracks.iter_mut().find(|t| t.id == key) else {
            return false;
        };
        if !f(t) {
            return false;
        }
        self.proj.doc.set(&p);
        self.reload_changed_fx(&before, &p);
        self.apply(p);
        self.proj.dirty = true;
        true
    }

    /// Adds a version to the track commands act on and plays it: a copy
    /// of the one playing, or (`copy` false) one with its sound but no
    /// clips, notes or automation.
    pub(crate) fn new_version(&mut self, ctx: &egui::Context, copy: bool) -> bool {
        let Some(ti) = self.version_track() else {
            self.message = Some("Add a track first.".into());
            return false;
        };
        self.edit_versions(ctx, ti, |t| {
            let name = next_name(t);
            t.add_version(daw_project::new_id(), name, "Version 1", copy);
            true
        })
    }

    /// Plays version `id` of track `ti`.
    pub(crate) fn switch_version(&mut self, ctx: &egui::Context, ti: usize, id: &str) -> bool {
        self.edit_versions(ctx, ti, |t| t.switch_version(id))
    }

    /// Plays the next (or, with `back`, the previous) version of the
    /// track commands act on, round to the first after the last.
    pub(crate) fn step_version(&mut self, ctx: &egui::Context, back: bool) -> bool {
        let Some(ti) = self.version_track() else {
            return false;
        };
        let t = &self.tracks[ti];
        let n = t.versions.len();
        let Some(at) = (t.versions.iter()).position(|v| Some(&v.id) == t.version.as_ref()) else {
            self.message = Some(format!("{} has no other versions.", t.name));
            return false;
        };
        let to = if back { (at + n - 1) % n } else { (at + 1) % n };
        let id = t.versions[to].id.clone();
        self.switch_version(ctx, ti, &id)
    }

    /// Deletes the version playing on the track commands act on; the
    /// one before it plays instead.
    pub(crate) fn delete_playing_version(&mut self, ctx: &egui::Context) -> bool {
        let Some(ti) = self.version_track() else {
            return false;
        };
        let Some(id) = self.tracks[ti].version.clone() else {
            let name = &self.tracks[ti].name;
            self.message = Some(format!("{name} has no other versions."));
            return false;
        };
        self.edit_versions(ctx, ti, |t| t.delete_version(&id))
    }

    /// Acts on a choice from a track's menu on the timeline.
    pub(crate) fn ask_version(&mut self, ctx: &egui::Context, ti: usize, ask: VersionAsk) {
        let Some(key) = self.tracks.get(ti).map(|t| t.key.clone()) else {
            return;
        };
        self.versions_ui.track = Some(key);
        // Through the commands, so they act the same from the menu, the
        // keys and outside the app.
        match ask {
            VersionAsk::Switch(i) => {
                if let Some(id) = self.tracks[ti].versions.get(i).map(|v| v.id.clone()) {
                    self.switch_version(ctx, ti, &id);
                }
            }
            VersionAsk::New => self.run(ctx, crate::commands::Cmd::NewVersion),
            VersionAsk::Duplicate => self.run(ctx, crate::commands::Cmd::DuplicateVersion),
            VersionAsk::Window => self.run(ctx, crate::commands::Cmd::TrackVersions),
        }
        if !self.versions_ui.open {
            self.versions_ui.track = None;
        }
    }

    /// Each lane's versions for the timeline: their names and the place
    /// of the one playing, or nothing for a track without versions.
    pub(crate) fn lane_versions(&self) -> Vec<(Vec<String>, usize)> {
        (self.tracks.iter())
            .map(|t| {
                let names = t.versions.iter().map(|v| v.name.clone()).collect();
                let at = (t.versions.iter())
                    .position(|v| Some(&v.id) == t.version.as_ref())
                    .unwrap_or(0);
                (names, at)
            })
            .collect()
    }

    pub(crate) fn versions_window(&mut self, ctx: &egui::Context) {
        if !self.versions_ui.open {
            return;
        }
        let Some(ti) = self.version_track() else {
            self.versions_ui.open = false;
            return;
        };
        // It stays on the track it opened on.
        self.versions_ui.track = Some(self.tracks[ti].key.clone());
        let tracks: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| !t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let t = &self.tracks[ti];
        let (key, name) = (t.key.clone(), t.name.clone());
        let list: Vec<(String, String)> = (t.versions.iter())
            .map(|v| (v.id.clone(), v.name.clone()))
            .collect();
        let playing = t.version.clone();
        let may_edit = self.may_edit_song();
        let mut open = true;
        let mut action = None;
        let ui_state = &mut self.versions_ui;
        egui::Window::new("Track versions")
            .open(&mut open)
            .resizable(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Track");
                    egui::ComboBox::from_id_salt("versions-track")
                        .selected_text(&name)
                        .show_ui(ui, |ui| {
                            for (k, n) in &tracks {
                                if ui.selectable_label(*k == key, n).clicked() {
                                    action = Some(Action::Track(k.clone()));
                                }
                            }
                        })
                        .response
                        .accessible_name("Track whose versions are shown");
                });
                ui.label(
                    RichText::new(
                        "Each version keeps its own clips, notes, takes, effects, instrument, \
                         automation and fader. The name and routing are shared.",
                    )
                    .weak(),
                );
                ui.separator();
                if list.is_empty() {
                    ui.label(
                        RichText::new(format!("{name} has one version: what it plays now.")).weak(),
                    );
                }
                ui.add_enabled_ui(may_edit, |ui| {
                    for (id, vname) in &list {
                        if let Some(a) = ui_state.row(ui, id, vname, playing.as_ref() == Some(id)) {
                            action = Some(a);
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui
                            .button("Duplicate")
                            .on_hover_text("A copy of the version playing, to change and compare")
                            .clicked()
                        {
                            action = Some(Action::Duplicate);
                        }
                        if ui
                            .button("New empty")
                            .on_hover_text(
                                "A version with the same instrument, effects and fader, but no \
                                 clips, notes or automation",
                            )
                            .clicked()
                        {
                            action = Some(Action::New);
                        }
                    });
                });
            });
        if !open {
            self.versions_ui.open = false;
            self.versions_ui.track = None;
        }
        match action {
            Some(Action::Track(k)) => {
                self.versions_ui.track = Some(k);
                self.versions_ui.renaming = None;
                self.versions_ui.asked = None;
            }
            Some(Action::Switch(id)) => {
                self.switch_version(ctx, ti, &id);
            }
            Some(Action::New) => {
                self.new_version(ctx, false);
            }
            Some(Action::Duplicate) => {
                self.new_version(ctx, true);
            }
            Some(Action::Rename(id, n)) => {
                let n: String = n.trim().chars().take(MAX_NAME).collect();
                if !n.is_empty() {
                    self.edit_versions(ctx, ti, |t| t.rename_version(&id, &n));
                }
            }
            Some(Action::Delete(id)) => {
                self.edit_versions(ctx, ti, |t| t.delete_version(&id));
            }
            None => {}
        }
    }
}

impl Versions {
    /// One version's row: chosen to play, renamed or deleted.
    fn row(&mut self, ui: &mut egui::Ui, id: &str, name: &str, playing: bool) -> Option<Action> {
        let mut action = None;
        let mut done = false;
        if let Some((rid, text)) = &mut self.renaming
            && rid == id
        {
            ui.horizontal(|ui| {
                let r = ui
                    .add(
                        egui::TextEdit::singleline(text)
                            .char_limit(MAX_NAME)
                            .desired_width(180.0),
                    )
                    .accessible_name("Version name");
                r.request_focus();
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Rename").clicked() || enter {
                    action = Some(Action::Rename(id.to_string(), text.clone()));
                }
                if ui.button("Cancel").clicked() || action.is_some() {
                    done = true;
                }
            });
            if done {
                self.renaming = None;
            }
            return action;
        }
        if self.asked.as_deref() == Some(id) {
            ui.label(format!(
                "Delete “{name}”? Its clips and settings go with it; Undo brings them back."
            ));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    action = Some(Action::Delete(id.to_string()));
                    self.asked = None;
                }
                if ui.button("Cancel").clicked() {
                    self.asked = None;
                }
            });
            return action;
        }
        ui.horizontal(|ui| {
            if ui
                .radio(playing, name)
                .on_hover_text("Play this version")
                .clicked()
                && !playing
            {
                action = Some(Action::Switch(id.to_string()));
            }
            if ui.small_button("Rename").clicked() {
                self.renaming = Some((id.to_string(), name.to_string()));
            }
            if ui.small_button("Delete…").clicked() {
                self.asked = Some(id.to_string());
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

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

    fn run(h: &mut Harness<'_, App>, cmd: Cmd) {
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, cmd);
        h.run();
    }

    fn clip_names(h: &Harness<'_, App>) -> Vec<String> {
        (h.state().tracks[0].note_clips.iter())
            .map(|c| c.name.clone())
            .collect()
    }

    /// An instrument track with one note clip, "Riff".
    fn with_riff() -> Harness<'static, App> {
        let mut h = app();
        run(&mut h, Cmd::AddInstrumentTrack);
        let ppq = daw_project::PPQ;
        h.state_mut().tracks[0]
            .note_clips
            .push(daw_project::NoteClip::new(
                "riff".into(),
                "Riff".into(),
                0,
                ppq * 4,
            ));
        h.run();
        h
    }

    #[test]
    fn versions_are_made_switched_and_undone_through_the_commands() {
        let mut h = with_riff();
        assert!(!h.state().can_run(Cmd::NextVersion), "nothing to switch to");
        run(&mut h, Cmd::DuplicateVersion);
        let t = &h.state().tracks[0];
        let names: Vec<&str> = t.versions.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Version 1", "Version 2"]);
        assert_eq!(clip_names(&h), ["Riff"], "the copy has the clip too");

        // Change the copy.
        h.state_mut().tracks[0].note_clips[0].name = "Riff, faster".into();
        h.run();
        run(&mut h, Cmd::PrevVersion);
        assert_eq!(clip_names(&h), ["Riff"]);
        run(&mut h, Cmd::NextVersion);
        assert_eq!(clip_names(&h), ["Riff, faster"]);

        // A new empty version keeps the instrument but no clips.
        run(&mut h, Cmd::NewVersion);
        let t = &h.state().tracks[0];
        assert_eq!(t.versions.len(), 3);
        assert!(t.note_clips.is_empty());
        assert!(t.instrument.is_some());

        // Undo takes back one step at a time.
        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        assert_eq!(h.state().tracks[0].versions.len(), 2);
        assert_eq!(clip_names(&h), ["Riff, faster"]);

        // Deleting the one playing leaves the other, and no versions.
        run(&mut h, Cmd::DeleteVersion);
        let t = &h.state().tracks[0];
        assert!(t.versions.is_empty() && t.version.is_none());
        assert_eq!(clip_names(&h), ["Riff"]);
        // And the document has it so.
        assert_eq!(h.state().proj.doc.get().tracks[0].note_clips.len(), 1);
    }

    #[test]
    fn the_window_switches_renames_and_deletes() {
        let mut h = with_riff();
        run(&mut h, Cmd::NewVersion);
        run(&mut h, Cmd::TrackVersions);
        h.get_by_label("Version 1").click();
        h.run();
        assert_eq!(clip_names(&h), ["Riff"]);

        h.get_all_by_label("Rename").next().unwrap().click();
        h.run();
        h.get_by_label("Version name").type_text(" (slow)");
        h.run();
        // The first row's Rename now keeps the name.
        h.get_all_by_label("Rename").next().unwrap().click();
        h.run();
        h.run();
        let t = &h.state().tracks[0];
        assert_eq!(t.versions[0].name, "Version 1 (slow)");

        h.get_all_by_label("Delete…").nth(1).unwrap().click();
        h.run();
        h.get_by_label("Delete").click();
        h.run();
        let t = &h.state().tracks[0];
        assert!(t.versions.is_empty());
        assert_eq!(clip_names(&h), ["Riff"]);
    }

    #[test]
    fn the_commands_are_found_by_search() {
        let found = crate::commands::search("track version");
        for c in [
            Cmd::TrackVersions,
            Cmd::NewVersion,
            Cmd::DuplicateVersion,
            Cmd::NextVersion,
            Cmd::PrevVersion,
            Cmd::DeleteVersion,
        ] {
            assert!(found.contains(&c), "{c:?}");
        }
    }

    #[test]
    fn an_assistant_makes_and_switches_versions_end_to_end() {
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        d.command("add-instrument-track");
        let key = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        d.command("duplicate-track-version");
        d.call(
            "set_parameter",
            json!({"track": key, "param": "gain", "value": -6.0}),
        )
        .unwrap();
        let t = d.project()["tracks"][0].clone();
        let versions = t["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[1]["playing"], true);
        assert_eq!(t["gain_db"], -6.0);
        let first = versions[0]["id"].as_str().unwrap().to_string();
        d.call("switch_version", json!({"track": key, "version": first}))
            .unwrap();
        let t = d.project()["tracks"][0].clone();
        assert_eq!(t["gain_db"], 0.0, "version 1 has its own fader");
        assert_eq!(t["versions"][0]["playing"], true);
        d.command("next-track-version");
        assert_eq!(d.project()["tracks"][0]["gain_db"], -6.0);
        // Each is one undo step.
        d.command("undo");
        assert_eq!(d.project()["tracks"][0]["gain_db"], 0.0);
        assert!(
            d.call("switch_version", json!({"track": key, "version": "nope"}))
                .is_err()
        );
    }
}
