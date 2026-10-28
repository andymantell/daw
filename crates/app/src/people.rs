// SPDX-License-Identifier: GPL-3.0-or-later
//! Who is who (roadmap 1.6): this user's name, the project's members,
//! and letting in people whose changes arrive.
//!
//! The install's secret and name are kept in the settings folder
//! (`identity.txt`). Only a display name is asked for, never anything
//! else (research/PRIVACY_CHILD_DATA.md). Changes from people the
//! owner hasn't let in wait here until the owner says yes.

use std::path::PathBuf;

use daw_project::{Identity, MAX_NAME, Member, Role};
use eframe::egui;

use crate::{App, config_dir};

fn identity_path() -> PathBuf {
    config_dir().join("identity.txt")
}

/// Kept when the user says "Not now" to giving a name, so it isn't
/// asked every time.
fn skipped_path() -> PathBuf {
    config_dir().join("name-skipped.txt")
}

#[derive(Default)]
pub(crate) struct People {
    /// The name being typed, while the name window is open.
    naming: Option<String>,
    pub(crate) open: bool,
    /// Changes refused for coming from people not let in, waiting for
    /// the owner.
    waiting: Vec<Waiting>,
    /// A bundle to merge once its project has opened.
    pub(crate) after_open: Option<(PathBuf, Vec<u8>)>,
}

struct Waiting {
    people: Vec<Member>,
    bytes: Vec<u8>,
}

/// This install's identity, made the first time. Tests get a fresh one.
pub(crate) fn load_identity() -> Identity {
    if cfg!(test) {
        return Identity::generate("Tester").expect("random numbers");
    }
    let path = identity_path();
    if let Some(me) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| Identity::from_text(&s))
    {
        return me;
    }
    let me = Identity::generate("").expect("random numbers");
    save_identity(&me);
    me
}

fn save_identity(me: &Identity) {
    let _ = std::fs::create_dir_all(config_dir());
    let _ = std::fs::write(identity_path(), me.to_text());
}

/// Whether to ask for a name when the app starts.
pub(crate) fn ask_name_at_start(me: &Identity) -> bool {
    !cfg!(test) && me.name.is_empty() && !skipped_path().exists()
}

/// "Pat", "Pat and Sam".
fn names(people: &[Member]) -> String {
    let n: Vec<String> = people.iter().map(Member::shown_name).collect();
    match n.as_slice() {
        [] => "someone".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn role_name(r: Option<Role>) -> &'static str {
    match r {
        Some(Role::Owner) => "Owner",
        Some(Role::Editor) => "Can edit",
        Some(Role::Commenter) => "Can comment",
        None => "Not let in",
    }
}

impl App {
    /// Signs the open project in as this user. Called whenever a
    /// project is made or opened.
    pub(crate) fn sign_in(&mut self) {
        self.people.waiting.clear();
        self.proj.doc.sign_in(&self.me);
    }

    /// Asks for this user's name.
    pub(crate) fn ask_name(&mut self) {
        self.people.naming = Some(self.me.name.clone());
    }

    /// Merges another copy's changes. Changes from people the owner
    /// hasn't let in are kept for the owner to decide on: false then.
    pub(crate) fn merge_or_ask(&mut self, bytes: &[u8]) -> Result<bool, String> {
        let check = self.proj.doc.check(bytes)?;
        if check.ok() {
            self.proj.doc.merge(bytes)?;
            return Ok(true);
        }
        if check.unknown || self.proj.doc.my_role() != Some(Role::Owner) {
            return Err(format!("not merged: {}", check.reason()));
        }
        // A newer copy from the same people replaces the older.
        let keys = |p: &[Member]| p.iter().map(|m| m.key.clone()).collect::<Vec<_>>();
        self.people
            .waiting
            .retain(|w| keys(&w.people) != keys(&check.refused));
        self.people.waiting.push(Waiting {
            people: check.refused,
            bytes: bytes.to_vec(),
        });
        Ok(false)
    }

    /// Merges a bundle that was waiting for its project to open.
    pub(crate) fn merge_bundle_after_open(&mut self, ctx: &egui::Context) {
        let Some((dir, bytes)) = self.people.after_open.take() else {
            return;
        };
        if self.proj.folder.as_ref().is_none_or(|f| f.dir != dir) {
            return;
        }
        self.sync_doc(ctx, true);
        match self.merge_or_ask(&bytes) {
            Ok(true) => {
                self.apply(self.proj.doc.get());
                self.proj.dirty = true;
            }
            Ok(false) => {}
            Err(e) => self.message = Some(format!("Couldn't merge the bundle: {e}")),
        }
    }

    /// Lets the first waiting people in as `role`, then merges whatever
    /// that allows.
    fn let_in(&mut self, ctx: &egui::Context, role: Role) {
        if self.people.waiting.is_empty() {
            return;
        }
        let w = self.people.waiting.remove(0);
        for m in &w.people {
            if let Err(e) = self.proj.doc.set_role(&m.key, Some(role)) {
                self.message = Some(format!("Couldn't let {} in: {e}", m.shown_name()));
                return;
            }
        }
        self.sync_doc(ctx, true);
        let mut merged = false;
        for w in std::iter::once(w).chain(std::mem::take(&mut self.people.waiting)) {
            match self.merge_or_ask(&w.bytes) {
                Ok(m) => merged |= m,
                Err(e) => self.message = Some(format!("Couldn't merge: {e}")),
            }
        }
        if merged {
            self.apply(self.proj.doc.get());
        }
        self.proj.dirty = true;
    }

    /// The name, people and waiting-changes windows.
    pub(crate) fn people_windows(&mut self, ctx: &egui::Context) {
        self.name_window(ctx);
        self.waiting_window(ctx);
        self.members_window(ctx);
    }

    fn name_window(&mut self, ctx: &egui::Context) {
        let Some(mut name) = self.people.naming.take() else {
            return;
        };
        let mut done = None;
        egui::Window::new("Your name")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("What should people you share projects with see you as?");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .char_limit(MAX_NAME)
                        .hint_text("A first name or nickname is enough"),
                );
                r.request_focus();
                ui.small("Nothing else is asked for, and it is only stored in your projects.");
                ui.horizontal(|ui| {
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ok = !name.trim().is_empty();
                    if ui.add_enabled(ok, egui::Button::new("OK")).clicked() || (enter && ok) {
                        done = Some(true);
                    }
                    if ui.button("Not now").clicked() {
                        done = Some(false);
                    }
                });
            });
        match done {
            Some(true) => {
                self.me.name = daw_project::clean_name(&name);
                if !cfg!(test) {
                    save_identity(&self.me);
                }
                self.proj.doc.set_my_name(&self.me.name);
            }
            Some(false) => {
                if !cfg!(test) {
                    let _ = std::fs::create_dir_all(config_dir());
                    let _ = std::fs::write(skipped_path(), "");
                }
            }
            None => self.people.naming = Some(name),
        }
    }

    fn waiting_window(&mut self, ctx: &egui::Context) {
        let Some(w) = self.people.waiting.first() else {
            return;
        };
        let who = names(&w.people);
        let mut chose = None;
        egui::Window::new("Someone wants to join")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "{who} made changes to this project, but you haven't let them in yet."
                ));
                ui.label("Only let them in if you know who this is and were expecting them.");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("Let {who} edit"))
                        .on_hover_text("They can change anything but who is in the project")
                        .clicked()
                    {
                        chose = Some(Some(Role::Editor));
                    }
                    if ui
                        .button(format!("Let {who} comment only"))
                        .on_hover_text("They can add comments, and change only their own")
                        .clicked()
                    {
                        chose = Some(Some(Role::Commenter));
                    }
                    if ui
                        .button("Not now")
                        .on_hover_text("Their changes stay out of your copy")
                        .clicked()
                    {
                        chose = Some(None);
                    }
                });
            });
        match chose {
            Some(Some(role)) => self.let_in(ctx, role),
            Some(None) => {
                self.people.waiting.remove(0);
            }
            None => {}
        }
    }

    fn members_window(&mut self, ctx: &egui::Context) {
        if !self.people.open {
            return;
        }
        let mut open = true;
        let members = self.proj.doc.members();
        let me = self.proj.doc.my_key().unwrap_or_default().to_string();
        let owner = self.proj.doc.my_role() == Some(Role::Owner);
        let mut change = None;
        let mut rename = false;
        egui::Window::new("People")
            .open(&mut open)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let shown = if self.me.name.is_empty() {
                        "(no name yet)"
                    } else {
                        &self.me.name
                    };
                    ui.label(format!("You are {shown}"));
                    if ui.button("Change your name…").clicked() {
                        rename = true;
                    }
                });
                ui.separator();
                egui::Grid::new("members")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for m in &members {
                            let name = if m.key == me {
                                format!("{} (you)", m.shown_name())
                            } else {
                                m.shown_name()
                            };
                            ui.label(name);
                            if owner && m.role != Some(Role::Owner) {
                                let mut role = m.role;
                                egui::ComboBox::from_id_salt(&m.key)
                                    .selected_text(role_name(role))
                                    .show_ui(ui, |ui| {
                                        for r in [Some(Role::Editor), Some(Role::Commenter), None] {
                                            ui.selectable_value(&mut role, r, role_name(r));
                                        }
                                    })
                                    .response
                                    .accessible_name(format!("What {} may do", m.shown_name()));
                                if role != m.role {
                                    change = Some((m.key.clone(), role));
                                }
                            } else {
                                ui.label(role_name(m.role));
                            }
                            ui.end_row();
                        }
                    });
                if members.is_empty() {
                    ui.label("Nobody else has this project yet.");
                } else if !owner && self.proj.doc.my_role().is_none() {
                    let owner = members.iter().find(|m| m.role == Some(Role::Owner));
                    let owner = owner.map_or("the owner".into(), Member::shown_name);
                    ui.add_space(4.0);
                    ui.label(format!(
                        "{owner} owns this project. Your changes reach their copy once they \
                         let you in, which they're asked when your changes first arrive."
                    ));
                }
                ui.add_space(4.0);
                ui.small(
                    "People are known by a key their copy of the app keeps, so nobody can \
                     pretend to be them. There are no accounts.",
                );
            });
        if let Some((key, role)) = change {
            match self.proj.doc.set_role(&key, role) {
                Ok(()) => self.proj.dirty = true,
                Err(e) => self.message = Some(format!("Couldn't change that: {e}")),
            }
        }
        if rename {
            self.ask_name();
        }
        self.people.open = open;
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT as _, Queryable as _};

    use daw_project::{Identity, Marker, ProjectDoc, Role};

    use super::*;

    fn harness() -> Harness<'static, App> {
        Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()))
    }

    /// The teacher's copy of the open project, with a marker added.
    fn teachers_changes(app: &App) -> (String, Vec<u8>) {
        let pat = Identity::generate("Pat").unwrap();
        let mut theirs = ProjectDoc::from_bytes(&app.proj.doc.to_bytes(), 99).unwrap();
        theirs.sign_in(&pat);
        let mut p = theirs.get();
        p.markers.push(Marker {
            id: "m1".into(),
            bar: 3,
            name: "Chorus".into(),
        });
        theirs.set(&p);
        (theirs.my_key().unwrap().to_string(), theirs.to_bytes())
    }

    fn labels(h: &Harness<'_, App>) -> Vec<String> {
        h.root()
            .children_recursive()
            .filter(|n| egui::accessibility::is_input(n.accesskit_node().role()))
            .filter_map(|n| n.accesskit_node().label())
            .collect()
    }

    #[test]
    fn the_owner_lets_a_teacher_in() {
        let mut h = harness();
        let (pat, bytes) = teachers_changes(h.state());
        assert_eq!(h.state_mut().merge_or_ask(&bytes), Ok(false));
        h.run();
        assert!(
            h.state().markers.is_empty(),
            "nothing merged before saying yes"
        );
        h.get_by_label("Let Pat edit").click();
        h.run();
        let app = h.state();
        assert_eq!(app.markers.len(), 1, "merged once let in");
        let members = app.proj.doc.members();
        assert_eq!(members.len(), 2);
        assert_eq!(members[1].key, pat);
        assert_eq!(members[1].role, Some(Role::Editor));
    }

    #[test]
    fn not_now_keeps_their_changes_out() {
        let mut h = harness();
        let (_, bytes) = teachers_changes(h.state());
        let _ = h.state_mut().merge_or_ask(&bytes);
        h.run();
        h.get_by_label("Not now").click();
        h.run();
        assert!(h.state().markers.is_empty());
        assert!(h.query_by_label("Let Pat edit").is_none());
    }

    #[test]
    fn the_people_window_lists_everyone_and_names_its_controls() {
        let mut h = harness();
        let (pat, bytes) = teachers_changes(h.state());
        let app = h.state_mut();
        app.proj.doc.set_role(&pat, Some(Role::Editor)).unwrap();
        assert_eq!(app.merge_or_ask(&bytes), Ok(true));
        app.people.open = true;
        h.run();
        assert!(h.query_by_label("Tester (you)").is_some());
        assert!(h.query_by_label("Pat").is_some());
        let unnamed: Vec<String> = labels(&h)
            .into_iter()
            .filter(|n| !n.chars().any(char::is_alphanumeric))
            .collect();
        assert_eq!(unnamed, Vec::<String>::new());
        h.get_by_label("Change your name…").click();
        h.run();
        assert!(h.query_by_label("OK").is_some());
    }

    #[test]
    fn names_are_asked_for_once_and_kept_short() {
        let mut h = harness();
        h.state_mut().me.name.clear();
        h.state_mut().ask_name();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput).focus();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput)
            .type_text("  Andy  ");
        h.run();
        h.get_by_label("OK").click();
        h.run();
        assert_eq!(h.state().me.name, "Andy");
        assert_eq!(
            h.state()
                .proj
                .doc
                .name_of(h.state().proj.doc.my_key().unwrap())
                .as_deref(),
            Some("Andy")
        );
        assert!(h.query_by_label("OK").is_none());
    }
}
