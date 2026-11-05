// SPDX-License-Identifier: GPL-3.0-or-later
//! Review mode (Phase 4, B1): Edit → Suggest edits starts making edits
//! on a branch of the project, so a teacher can suggest changes to a
//! student's song without overwriting it. While suggesting, every edit
//! to the song (from the timeline, the panes or an assistant) goes to
//! the branch; comments and assignments still go to the project, and so
//! do other people's changes arriving from a shared folder, which wait
//! until the suggesting stops. Saving keeps the suggestion with the
//! project.
//!
//! Edit → Suggested edits… lists the suggestions, and for each the
//! changes it makes, in words, with Accept and Reject on each. Accepting
//! is one undoable edit to the song. See `daw_project::review` for what
//! counts as one change.

use daw_project::{Decision, Project, ProjectDoc, SuggestedChange, Suggestion};
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};

/// The branch edits are being suggested on.
pub(crate) struct Branch {
    /// The suggestion's id.
    pub(crate) id: String,
    pub(crate) doc: ProjectDoc,
}

#[derive(Default)]
pub(crate) struct Review {
    pub(crate) open: bool,
    /// The suggestion shown.
    chosen: Option<String>,
    /// A name being typed for a new suggestion.
    draft: String,
    /// The changes of the suggestion shown, worked out at a version of
    /// the document (and of the branch, while suggesting).
    found: Option<Found>,
}

struct Found {
    id: String,
    version: (Vec<u8>, Vec<u8>),
    changes: Option<Vec<SuggestedChange>>,
}

/// What the user chose in the window.
enum Action {
    Start(String),
    Resume(String),
    Stop,
    Accept(String, Vec<String>),
    Reject(String, Vec<String>),
    Delete(String),
}

impl App {
    /// The song as the app should show it: the project's, or the
    /// branch's while suggesting, with the project's comments and
    /// assignments either way.
    pub(crate) fn shown_project(&self) -> Project {
        let Some(b) = &self.proj.suggesting else {
            return self.proj.doc.get();
        };
        let mut p = b.doc.get();
        let now = self.proj.doc.get();
        p.comments = now.comments;
        p.assignments = now.assignments;
        p
    }

    /// While suggesting, writes `p`'s comments and assignments to the
    /// project, and returns `p` as the branch should have it: with the
    /// branch's own, unchanged. Otherwise `p` as it is.
    pub(crate) fn write_outside_suggestion(&mut self, p: &Project) -> Project {
        let Some(b) = &self.proj.suggesting else {
            return p.clone();
        };
        let mut song = p.clone();
        let branch = b.doc.get();
        song.comments = branch.comments;
        song.assignments = branch.assignments;
        let mut main = self.proj.doc.get();
        if main.comments != p.comments || main.assignments != p.assignments {
            main.comments = p.comments.clone();
            main.assignments = p.assignments.clone();
            self.proj.doc.set(&main);
        }
        song
    }

    /// Writes an edit to the song (the branch, while suggesting) as one
    /// undo step, called `label` if given.
    pub(crate) fn set_song(&mut self, p: &Project, label: Option<&str>) {
        let song = self.write_outside_suggestion(p);
        match label {
            Some(l) => self.proj.song_mut().set_labelled(&song, l),
            None => self.proj.song_mut().set(&song),
        };
    }

    /// Whether this user may accept suggestions: change the song itself.
    pub(crate) fn may_change_the_song(&self) -> bool {
        self.proj.doc.my_role() != Some(daw_project::Role::Commenter)
    }

    /// Starts suggesting edits, from the song as it is now, in a new
    /// suggestion called `name`. Returns its id.
    pub(crate) fn start_suggesting(&mut self, ctx: &egui::Context, name: &str) -> String {
        self.stop_suggesting(ctx);
        self.sync_doc(ctx, true);
        self.write_plugin_states();
        let name = match name.trim() {
            "" => format!("Suggestion {}", self.proj.doc.suggestions().len() + 1),
            n => n.to_string(),
        };
        let me = self.my_author_id();
        let id = self.proj.doc.start_suggestion(&name, &me, now_ms());
        // A suggestion just started can always be read.
        let _ = self.resume_suggesting(ctx, &id);
        self.proj.dirty = true;
        id
    }

    /// Carries on suggesting edits in suggestion `id`: the app shows
    /// the song as the suggestion has it.
    pub(crate) fn resume_suggesting(
        &mut self,
        ctx: &egui::Context,
        id: &str,
    ) -> Result<(), String> {
        if self.proj.suggesting.as_ref().is_some_and(|b| b.id == id) {
            return Ok(());
        }
        self.stop_suggesting(ctx);
        let doc = (self.proj.doc.suggestion_branch(id))
            .ok_or("This suggestion can't be read from this copy of the project.")?;
        self.sync_doc(ctx, true);
        let before = self.project();
        self.proj.suggesting = Some(Branch {
            id: id.to_string(),
            doc,
        });
        self.show_song(before);
        self.review.chosen = Some(id.to_string());
        Ok(())
    }

    /// Stops suggesting: keeps the edits with the suggestion and shows
    /// the song again, with any changes that arrived meanwhile.
    pub(crate) fn stop_suggesting(&mut self, ctx: &egui::Context) {
        if self.proj.suggesting.is_none() {
            return;
        }
        self.sync_doc(ctx, true);
        self.write_plugin_states();
        self.proj.keep_suggestion();
        let before = self.project();
        self.proj.suggesting = None;
        self.show_song(before);
        self.proj.dirty = true;
    }

    /// Makes the app show the song (or branch) after a switch.
    fn show_song(&mut self, before: Project) {
        let after = self.shown_project();
        self.reload_changed_fx(&before, &after);
        self.apply(after);
        self.agent.revision += 1;
    }

    /// The suggestion edits are being made in, if any.
    pub(crate) fn suggesting(&self) -> Option<&str> {
        self.proj.suggesting.as_ref().map(|b| b.id.as_str())
    }

    /// Accepts changes `keys` of suggestion `id` (all its open ones if
    /// `keys` is empty) as one undoable edit. Answers the keys taken.
    pub(crate) fn accept_suggested(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        keys: &[String],
    ) -> Result<Vec<String>, String> {
        self.may_review(id)?;
        let all = keys.is_empty();
        let mut keys = self.keys_or_open(id, keys)?;
        self.sync_doc(ctx, true);
        self.write_plugin_states();
        let before = self.proj.doc.get();
        let mut p = before.clone();
        let mut label = String::new();
        let mut failed = Vec::new();
        for k in &keys {
            match self.proj.doc.take_change(id, k, &mut p) {
                Ok(l) => label = l,
                // Accepting all takes what it can.
                Err(e) if all => failed.push((k.clone(), e)),
                Err(e) => return Err(e),
            }
        }
        keys.retain(|k| !failed.iter().any(|(f, _)| f == k));
        if keys.is_empty() && !failed.is_empty() {
            return Err(failed[0].1.clone());
        }
        if keys.len() > 1 {
            label = format!("Accept {} suggested changes", keys.len());
        }
        if p != before {
            self.proj.doc.set_labelled(&p, &label);
        }
        for k in &keys {
            self.proj.doc.mark_change(id, k, Decision::Accepted);
        }
        let after = self.proj.doc.get();
        self.reload_changed_fx(&before, &after);
        self.apply(after);
        self.proj.dirty = true;
        self.agent.revision += 1;
        Ok(keys)
    }

    /// Rejects changes `keys` of suggestion `id` (all its open ones if
    /// empty). The song is left as it is.
    pub(crate) fn reject_suggested(
        &mut self,
        id: &str,
        keys: &[String],
    ) -> Result<Vec<String>, String> {
        self.may_review(id)?;
        let keys = self.keys_or_open(id, keys)?;
        for k in &keys {
            self.proj.doc.mark_change(id, k, Decision::Rejected);
        }
        self.proj.dirty = true;
        self.agent.revision += 1;
        Ok(keys)
    }

    fn may_review(&self, id: &str) -> Result<(), String> {
        if !self.may_change_the_song() {
            return Err("Only the owner and editors can accept or reject suggestions.".into());
        }
        if self.suggesting().is_some() {
            return Err("Stop suggesting before reviewing suggestions.".into());
        }
        if !self.proj.doc.suggestions().iter().any(|s| s.id == id) {
            return Err(format!("There is no suggestion {id}."));
        }
        Ok(())
    }

    /// `keys` checked against the suggestion's changes, or its open
    /// ones when `keys` is empty.
    fn keys_or_open(&self, id: &str, keys: &[String]) -> Result<Vec<String>, String> {
        let changes = (self.proj.doc.suggested_changes(id))
            .ok_or_else(|| format!("There is no suggestion {id}, or it can't be read here."))?;
        if keys.is_empty() {
            return Ok(changes
                .into_iter()
                .filter(|c| c.open())
                .map(|c| c.key)
                .collect());
        }
        for k in keys {
            if !changes.iter().any(|c| &c.key == k) {
                return Err(format!("Suggestion {id} has no change {k}."));
            }
        }
        Ok(keys.to_vec())
    }

    pub(crate) fn review_window(&mut self, ctx: &egui::Context) {
        self.suggesting_banner(ctx);
        if !self.review.open {
            return;
        }
        let list = self.proj.doc.suggestions();
        if self.review.chosen.is_none()
            || !list
                .iter()
                .any(|s| Some(&s.id) == self.review.chosen.as_ref())
        {
            self.review.chosen = list.first().map(|s| s.id.clone());
        }
        self.find_changes();
        let doc = &self.proj.doc;
        let me = self.my_author_id();
        let who = |key: &str| match key {
            "" => "someone".to_string(),
            k if k == me => "you".to_string(),
            k => doc.name_of(k).unwrap_or_else(|| "someone".into()),
        };
        let may_review = self.may_change_the_song();
        let suggesting = self.suggesting().map(String::from);
        let mut open = self.review.open;
        let mut action = None;
        let review = &mut self.review;
        crate::workspace::centred(egui::Window::new("Suggested edits"), ctx)
            .open(&mut open)
            .default_size([460.0, 520.0])
            .show(ctx, |ui| {
                ui.label(
                    "Edits suggested on a copy of the song. Accept the ones you want: \
                     each is an ordinary edit, which Undo takes back.",
                );
                ui.horizontal(|ui| {
                    // ai: edit_suggestion
                    ui.add(
                        egui::TextEdit::singleline(&mut review.draft)
                            .hint_text("Name, such as Lesson 4 ideas")
                            .desired_width(200.0),
                    );
                    // ai: suggest-edits
                    if ui.button("Suggest edits").clicked() {
                        action = Some(Action::Start(std::mem::take(&mut review.draft)));
                    }
                });
                ui.separator();
                if list.is_empty() {
                    ui.label("No suggestions yet.");
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for s in &list {
                        let label = format!("{} ({})", s.name, who(&s.author));
                        let on = review.chosen.as_deref() == Some(&s.id);
                        // ai: view
                        if ui.selectable_label(on, label).clicked() {
                            review.chosen = Some(s.id.clone());
                        }
                    }
                });
                let Some(s) = list
                    .iter()
                    .find(|s| review.chosen.as_deref() == Some(&s.id))
                else {
                    return;
                };
                ui.separator();
                suggestion_ui(
                    ui,
                    s,
                    review.found.as_ref(),
                    suggesting.as_deref(),
                    may_review,
                    &mut action,
                );
            });
        self.review.open = open;
        match action {
            Some(Action::Start(name)) => {
                self.start_suggesting(ctx, &name);
            }
            Some(Action::Resume(id)) => {
                if let Err(e) = self.resume_suggesting(ctx, &id) {
                    self.message = Some(e);
                }
            }
            Some(Action::Stop) => self.stop_suggesting(ctx),
            Some(Action::Accept(id, keys)) => {
                if let Err(e) = self.accept_suggested(ctx, &id, &keys) {
                    self.message = Some(e);
                }
            }
            Some(Action::Reject(id, keys)) => {
                if let Err(e) = self.reject_suggested(&id, &keys) {
                    self.message = Some(e);
                }
            }
            Some(Action::Delete(id)) => {
                if self.suggesting() == Some(&id) {
                    self.stop_suggesting(ctx);
                }
                self.proj.doc.delete_suggestion(&id);
                self.proj.dirty = true;
            }
            None => {}
        }
    }

    /// Works out the shown suggestion's changes again if the project or
    /// the branch changed.
    fn find_changes(&mut self) {
        let Some(id) = self.review.chosen.clone() else {
            self.review.found = None;
            return;
        };
        let branch = (self.proj.suggesting.as_ref())
            .filter(|b| b.id == id)
            .map(|b| b.doc.version())
            .unwrap_or_default();
        let found = self.review.found.as_ref().filter(|f| f.id == id);
        if !branch.is_empty() && found.is_none_or(|f| f.version.1 != branch) {
            // Shown as they are so far.
            self.proj.keep_suggestion();
        }
        let version = (self.proj.doc.version(), branch);
        if found.is_some_and(|f| f.version == version) {
            return;
        }
        self.review.found = Some(Found {
            changes: self.proj.doc.suggested_changes(&id),
            id,
            version,
        });
    }

    /// A strip across the top while suggesting, so it's never forgotten.
    fn suggesting_banner(&mut self, ctx: &egui::Context) {
        let Some(id) = self.suggesting().map(String::from) else {
            return;
        };
        let name = (self.proj.doc.suggestions().into_iter())
            .find(|s| s.id == id)
            .map_or_else(String::new, |s| s.name);
        let mut stop = false;
        let mut show = false;
        egui::Area::new(egui::Id::unique("suggesting-banner"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Suggesting edits: “{name}”. The song isn't changed."
                            ))
                            .strong(),
                        );
                        // ai: suggested-edits
                        if ui.button("Show").clicked() {
                            show = true;
                        }
                        // ai: stop-suggesting
                        if ui.button("Stop suggesting").clicked() {
                            stop = true;
                        }
                    });
                });
            });
        if show {
            self.review.open = true;
            self.review.chosen = Some(id);
        }
        if stop {
            self.stop_suggesting(ctx);
        }
    }

    pub(crate) fn suggestions_value(&mut self) -> Value {
        // The edits being suggested now, as they are so far.
        self.proj.keep_suggestion();
        let doc = &self.proj.doc;
        let list: Vec<Value> = (doc.suggestions().into_iter())
            .map(|s| {
                let changes = doc.suggested_changes(&s.id);
                json!({
                    "id": s.id, "name": s.name, "created_ms": s.created,
                    "by": doc.name_of(&s.author), "readable": s.readable,
                    "changes": changes.map(|v| v.iter().map(change_value).collect::<Vec<_>>()),
                })
            })
            .collect();
        json!({"suggestions": list, "suggesting": self.suggesting()})
    }

    pub(crate) fn review_tool(&mut self, ctx: &egui::Context, args: &Value) -> Done {
        let action = args["action"].as_str().ok_or("Say what to do (action).")?;
        let id = || -> Result<String, String> {
            args["suggestion"]
                .as_str()
                .map(String::from)
                .ok_or_else(|| "Give the suggestion's id.".to_string())
        };
        let known = |app: &App, id: &str| -> Result<(), String> {
            match app.proj.doc.suggestions().iter().any(|s| s.id == id) {
                true => Ok(()),
                false => Err(format!("There is no suggestion {id}.")),
            }
        };
        let keys: Vec<String> = (args["changes"].as_array().into_iter().flatten())
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        match action {
            "start" => {
                let id = self.start_suggesting(ctx, args["name"].as_str().unwrap_or(""));
                Ok((json!({"suggesting": id}), None))
            }
            "resume" => {
                let id = id()?;
                known(self, &id)?;
                self.resume_suggesting(ctx, &id)?;
                Ok((json!({"suggesting": id}), None))
            }
            "stop" => {
                let was = self
                    .suggesting()
                    .map(String::from)
                    .ok_or("No edits are being suggested.")?;
                self.stop_suggesting(ctx);
                Ok((json!({"stopped": was}), None))
            }
            "accept" => {
                let id = id()?;
                known(self, &id)?;
                let done = self.accept_suggested(ctx, &id, &keys)?;
                Ok((json!({"accepted": done}), None))
            }
            "reject" => {
                let id = id()?;
                known(self, &id)?;
                let done = self.reject_suggested(&id, &keys)?;
                Ok((json!({"rejected": done}), None))
            }
            "rename" => {
                let id = id()?;
                known(self, &id)?;
                let name = args["name"].as_str().map(str::trim).unwrap_or("");
                if name.is_empty() {
                    return Err("Give a name that isn't empty.".into());
                }
                self.proj.doc.rename_suggestion(&id, name);
                self.proj.dirty = true;
                Ok((json!({"suggestion": id, "name": name}), None))
            }
            "delete" => {
                let id = id()?;
                known(self, &id)?;
                if self.suggesting() == Some(&id) {
                    self.stop_suggesting(ctx);
                }
                self.proj.doc.delete_suggestion(&id);
                self.proj.dirty = true;
                Ok((json!({"deleted": id}), None))
            }
            other => Err(format!("There is no suggestion action {other}.")),
        }
    }
}

type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_suggestions",
        description: "Suggested edits (review mode): each suggestion's id, name, who made \
                      it and when, and every change it makes, each with its key, where it \
                      is, what it does in words, its decision (accepted, rejected or none), \
                      whether the song already has it (in_song), and whether the same thing \
                      was changed in the song since (changed_since). \"suggesting\" is the \
                      suggestion edits are going to now, if any.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "edit_suggestion",
        description: "Review mode. start: begin suggesting edits (with a name): from then \
                      on every edit to the song, by any tool, goes to the suggestion and \
                      the song is left as it is, until stop. resume: carry on suggesting \
                      in an earlier suggestion. stop: keep the edits and go back to the \
                      song. accept: take changes into the song, by their keys from \
                      get_suggestions (all open ones if none are given), as one undoable \
                      edit. reject: turn changes down (all open ones if none are given). \
                      rename, delete.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "action": {"type": "string", "enum": ["start", "resume", "stop", "accept", "reject", "rename", "delete"]},
                "suggestion": {"type": "string", "description": "Its id, for all but start and stop."},
                "name": {"type": "string"},
                "changes": {"type": "array", "items": {"type": "string"},
                            "description": "Changes' keys, for accept and reject."}
            }, "required": ["action"]})
        },
    },
];

fn change_value(c: &SuggestedChange) -> Value {
    json!({
        "key": c.key, "place": c.place, "text": c.text,
        "decision": c.decision.map(|d| match d {
            Decision::Accepted => "accepted",
            Decision::Rejected => "rejected",
        }),
        "in_song": c.in_song, "changed_since": c.changed_since,
    })
}

/// One suggestion: what to do with it, and its changes.
fn suggestion_ui(
    ui: &mut egui::Ui,
    s: &Suggestion,
    found: Option<&Found>,
    suggesting: Option<&str>,
    may_review: bool,
    action: &mut Option<Action>,
) {
    let here = suggesting == Some(s.id.as_str());
    ui.horizontal(|ui| {
        ui.heading(&s.name);
        if here {
            // ai: stop-suggesting
            if ui.button("Stop suggesting").clicked() {
                *action = Some(Action::Stop);
            }
        } else if s.readable {
            // ai: edit_suggestion
            if ui.button("Carry on suggesting").clicked() {
                *action = Some(Action::Resume(s.id.clone()));
            }
        }
        // ai: edit_suggestion
        if ui.button("Delete").clicked() {
            *action = Some(Action::Delete(s.id.clone()));
        }
    });
    let Some(changes) = found
        .filter(|f| f.id == s.id)
        .and_then(|f| f.changes.as_ref())
    else {
        ui.label("This suggestion can't be read from this copy of the project.");
        return;
    };
    if changes.is_empty() {
        ui.label("No changes yet.");
        return;
    }
    let open = changes.iter().filter(|c| c.open()).count();
    let reviewing = may_review && !here;
    if reviewing && open > 0 {
        ui.horizontal(|ui| {
            ui.label(format!("{open} waiting"));
            // ai: edit_suggestion
            if ui.button("Accept all").clicked() {
                *action = Some(Action::Accept(s.id.clone(), Vec::new()));
            }
            // ai: edit_suggestion
            if ui.button("Reject all").clicked() {
                *action = Some(Action::Reject(s.id.clone(), Vec::new()));
            }
        });
    } else if here {
        ui.label("Being suggested now: review it once you stop.");
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut place = "";
        for c in changes {
            if c.place != place {
                place = &c.place;
                ui.label(RichText::new(place).strong());
            }
            ui.horizontal(|ui| {
                let state = match (c.decision, c.in_song) {
                    (Some(Decision::Accepted), _) => "Accepted",
                    (Some(Decision::Rejected), _) => "Rejected",
                    (None, true) => "Already in the song",
                    (None, false) => "",
                };
                ui.vertical(|ui| {
                    for line in &c.text {
                        ui.label(line);
                    }
                    if c.changed_since && c.open() {
                        ui.label(
                            RichText::new("Changed in the song since: accepting replaces that")
                                .small()
                                .weak(),
                        );
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !state.is_empty() {
                        ui.label(RichText::new(state).weak());
                    }
                    if reviewing && c.open() {
                        // ai: edit_suggestion
                        if ui.button("Reject").clicked() {
                            *action = Some(Action::Reject(s.id.clone(), vec![c.key.clone()]));
                        }
                        // ai: edit_suggestion
                        if ui.button("Accept").clicked() {
                            *action = Some(Action::Accept(s.id.clone(), vec![c.key.clone()]));
                        }
                    }
                });
            });
        }
    });
}

fn now_ms() -> i64 {
    crate::lessons::now_ms()
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use crate::agent::driver::Driver;

    fn changes(d: &mut Driver) -> Vec<serde_json::Value> {
        let v = d.call("get_suggestions", json!({})).unwrap();
        v["suggestions"][0]["changes"].as_array().unwrap().clone()
    }

    fn keys(d: &mut Driver) -> Vec<String> {
        (changes(d).iter())
            .map(|c| c["key"].as_str().unwrap().to_string())
            .collect()
    }

    /// A teacher suggests edits through the assistant interface, and the
    /// student takes some and turns others down.
    #[test]
    fn edits_are_suggested_then_accepted_or_rejected_one_by_one() {
        let mut d = Driver::new();
        d.call("add_track", json!({"kind": "audio", "name": "Guitar"}))
            .unwrap();
        let track = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();

        let id = d
            .call(
                "edit_suggestion",
                json!({"action": "start", "name": "Lesson 4 ideas"}),
            )
            .unwrap()["suggesting"]
            .as_str()
            .unwrap()
            .to_string();
        d.call("set_tempo", json!({"bpm": 100})).unwrap();
        d.call(
            "edit_markers",
            json!({"add": [{"bar": 9, "name": "Chorus"}]}),
        )
        .unwrap();
        d.call("edit_track", json!({"track": track, "name": "Lead guitar"}))
            .unwrap();
        d.call(
            "add_comment",
            json!({"text": "Try it slower", "seconds": 0}),
        )
        .unwrap();
        // The assistant sees the song as suggested.
        assert_eq!(d.project()["tempo_bpm"], 100.0);
        let v = d.call("get_suggestions", json!({})).unwrap();
        assert_eq!(v["suggesting"], id.as_str());

        d.call("edit_suggestion", json!({"action": "stop"}))
            .unwrap();
        // The song is as it was; the comment went to it straight away.
        let p = d.project();
        assert_eq!(p["tempo_bpm"], 120.0);
        assert_eq!(p["tracks"][0]["name"], "Guitar");
        assert!(p["markers"].as_array().unwrap().is_empty());
        assert_eq!(d.app().project().comments.len(), 1);

        let marker = d.app().proj.doc.suggested_project(&id).unwrap().markers[0]
            .id
            .clone();
        assert_eq!(
            keys(&mut d),
            [
                "tempo".to_string(),
                format!("track/{track}/settings"),
                format!("marker/{marker}"),
            ]
        );
        assert_eq!(changes(&mut d)[0]["text"][0], "Tempo 120 → 100 BPM");
        assert_eq!(changes(&mut d)[1]["place"], "Lead guitar");

        let done = d
            .call(
                "edit_suggestion",
                json!({"action": "accept", "suggestion": id, "changes": ["tempo"]}),
            )
            .unwrap();
        assert_eq!(done["accepted"], json!(["tempo"]));
        assert_eq!(d.project()["tempo_bpm"], 100.0);
        assert_eq!(d.project()["tracks"][0]["name"], "Guitar");
        d.call(
            "edit_suggestion",
            json!({"action": "reject", "suggestion": id, "changes": [format!("marker/{marker}")]}),
        )
        .unwrap();
        let c = changes(&mut d);
        assert_eq!(c[0]["decision"], "accepted");
        assert_eq!(c[0]["in_song"], true);
        assert_eq!(c[2]["decision"], "rejected");
        assert!(c[1]["decision"].is_null());

        // Accepting is an ordinary edit.
        d.command("undo");
        assert_eq!(d.project()["tempo_bpm"], 120.0);

        // Accepting with no keys takes every change still open.
        let done = d
            .call(
                "edit_suggestion",
                json!({"action": "accept", "suggestion": id}),
            )
            .unwrap();
        assert_eq!(done["accepted"], json!([format!("track/{track}/settings")]));
        assert_eq!(d.project()["tracks"][0]["name"], "Lead guitar");
        assert!(d.project()["markers"].as_array().unwrap().is_empty());

        // Carrying on shows the suggestion again; it can't be reviewed
        // meanwhile.
        d.call(
            "edit_suggestion",
            json!({"action": "resume", "suggestion": id}),
        )
        .unwrap();
        assert_eq!(d.project()["markers"].as_array().unwrap().len(), 1);
        assert!(
            d.call(
                "edit_suggestion",
                json!({"action": "reject", "suggestion": id})
            )
            .is_err()
        );
        d.call(
            "edit_suggestion",
            json!({"action": "delete", "suggestion": id}),
        )
        .unwrap();
        assert!(d.app().suggesting().is_none());
        let v = d.call("get_suggestions", json!({})).unwrap();
        assert!(v["suggestions"].as_array().unwrap().is_empty());
        assert!(
            d.call(
                "edit_suggestion",
                json!({"action": "accept", "suggestion": id})
            )
            .is_err()
        );
    }

    #[test]
    fn suggested_edits_are_undone_on_their_own_and_kept_by_saving() {
        let mut d = Driver::new();
        d.command("suggest-edits");
        assert!(d.app().suggesting().is_some());
        d.call("set_tempo", json!({"bpm": 90})).unwrap();
        d.call("set_tempo", json!({"bpm": 80})).unwrap();
        d.command("undo");
        assert_eq!(d.project()["tempo_bpm"], 90.0);
        // What saving writes holds the suggestion's edits.
        d.app().proj.keep_suggestion();
        let bytes = d.app().proj.doc.to_bytes();
        let copy = daw_project::ProjectDoc::from_bytes(&bytes, 7).unwrap();
        let s = &copy.suggestions()[0];
        assert_eq!(s.name, "Suggestion 1");
        assert_eq!(copy.suggested_project(&s.id).unwrap().tempo_bpm, 90.0);
        assert_eq!(copy.get().tempo_bpm, 120.0);
        d.command("stop-suggesting");
        assert_eq!(d.project()["tempo_bpm"], 120.0);
        assert!(!d.app().can_undo(), "the song has no edits to undo");
    }

    #[test]
    fn the_window_accepts_a_change() {
        let mut d = Driver::new();
        d.command("suggest-edits");
        d.call("set_tempo", json!({"bpm": 100})).unwrap();
        d.command("stop-suggesting");
        d.command("suggested-edits");
        d.h.run();
        d.h.get_by_label("Tempo 120 → 100 BPM");
        d.h.get_by_label("Accept").click();
        d.h.run();
        assert_eq!(d.app().project().tempo_bpm, 100.0);
        d.h.get_by_label("Accepted");
    }
}
