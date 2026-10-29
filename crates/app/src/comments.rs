// SPDX-License-Identifier: GPL-3.0-or-later
//! Pinned comments (roadmap 1.6): notes pinned to a time, or a stretch
//! of time, on one track or the whole song, with replies, resolving, and
//! a button to play from there. They live in the project document, so
//! they travel with bundles and shared folders; deleting one takes its
//! text out of the saved file too (see `daw_project::ProjectDoc`).

use std::time::{SystemTime, UNIX_EPOCH};

use daw_engine::time::{Frames, TempoMap};
use daw_project::Comment;
use eframe::egui::{self, Color32, RichText};

use crate::timeline::Pin;

/// Pins and their highlight on the timeline.
pub const COLOUR: Color32 = Color32::from_rgb(225, 110, 200);

/// Where a new comment goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// A track's id, or `None` for the whole song.
    pub track: Option<String>,
    pub start: i64,
    pub end: Option<i64>,
}

#[derive(Default)]
pub struct Comments {
    pub open: bool,
    /// The thread shown first and highlighted: its first comment's id.
    pub selected: Option<String>,
    /// Where the comment being written goes: set from the timeline's
    /// menu, or else the playhead (or the loop) when it is added.
    pub target: Option<Target>,
    /// On the loop rather than at the playhead.
    pub on_loop: bool,
    /// The track chosen for a new comment, if not the whole song.
    pub track: Option<String>,
    draft: String,
    /// A reply being written: the thread, and the text so far.
    reply: Option<(String, String)>,
    /// A comment being edited, and its text so far.
    editing: Option<(String, String)>,
    /// A comment that Delete was pressed on once, waiting for "Delete
    /// for good".
    deleting: Option<String>,
    pub show_resolved: bool,
    /// Give the new comment's field the keyboard next frame.
    focus_draft: bool,
}

/// What the app has, for one frame.
pub struct Ctx<'a> {
    pub map: &'a TempoMap,
    pub playhead: i64,
    /// The loop, in frames, when looping.
    pub loop_frames: Option<(i64, i64)>,
    /// Each track's id and name, in order.
    pub tracks: &'a [(String, String)],
    /// Who is writing: see [`my_author_id`](crate::App::my_author_id).
    pub me: &'a str,
    /// The project's members, for authors' names.
    pub members: &'a [daw_project::Member],
    /// Whether this user may change anyone's comments, not only their
    /// own.
    pub may_change_any: bool,
}

/// What the app should do after the panel is drawn.
#[derive(Default)]
pub struct Outcome {
    /// Move the playhead here and play.
    pub play_from: Option<i64>,
}

impl Comments {
    /// Opens the panel to write a comment pinned at `target`.
    pub fn write_at(&mut self, target: Target) {
        self.open = true;
        self.target = Some(target);
        self.focus_draft = true;
    }

    /// Opens the panel at a thread.
    pub fn show_thread(&mut self, id: &str) {
        self.open = true;
        self.selected = Some(id.to_string());
    }

    /// The side panel. Edits `comments` in place.
    pub fn panel(&mut self, ui: &mut egui::Ui, comments: &mut Vec<Comment>, cx: &Ctx) -> Outcome {
        let mut out = Outcome::default();
        self.new_comment(ui, comments, cx);
        ui.separator();
        let open = threads(comments).iter().filter(|c| !c.resolved).count();
        let resolved = threads(comments).len() - open;
        ui.horizontal(|ui| {
            ui.label(match open {
                0 => "No open comments".to_string(),
                1 => "1 open".to_string(),
                n => format!("{n} open"),
            });
            if resolved > 0 {
                ui.checkbox(&mut self.show_resolved, format!("Show {resolved} resolved"));
            }
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut shown: Vec<Comment> = threads(comments)
                    .into_iter()
                    .filter(|c| self.show_resolved || !c.resolved)
                    .cloned()
                    .collect();
                // The selected thread first, then in song order.
                shown.sort_by_key(|c| (self.selected.as_ref() != Some(&c.id), c.start, c.created));
                for root in shown {
                    self.thread(ui, comments, &root, cx, &mut out);
                }
            });
        out
    }

    fn new_comment(&mut self, ui: &mut egui::Ui, comments: &mut Vec<Comment>, cx: &Ctx) {
        let target = self.target.clone().unwrap_or_else(|| Target {
            track: self.track.clone(),
            start: match (self.on_loop, cx.loop_frames) {
                (true, Some((a, _))) => a,
                _ => cx.playhead,
            },
            end: match (self.on_loop, cx.loop_frames) {
                (true, Some((_, b))) => Some(b),
                _ => None,
            },
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("New comment on");
            let name = |id: &Option<String>| track_name(cx.tracks, id);
            let mut track = target.track.clone();
            egui::ComboBox::from_id_salt("comment track")
                .selected_text(name(&track))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut track, None, name(&None));
                    for (id, n) in cx.tracks {
                        ui.selectable_value(&mut track, Some(id.clone()), n);
                    }
                })
                .response
                .accessible_name("Comment on track");
            if track != target.track {
                match &mut self.target {
                    Some(t) => t.track = track.clone(),
                    None => self.track = track,
                }
            }
            if self.target.is_some() {
                ui.label(format!("at {}", place(cx.map, target.start, target.end)));
                if ui
                    .small_button("Use playhead")
                    .on_hover_text("Pin it at the playhead instead")
                    .clicked()
                {
                    self.target = None;
                }
            } else {
                ui.radio_value(&mut self.on_loop, false, "At the playhead");
                ui.add_enabled_ui(cx.loop_frames.is_some(), |ui| {
                    ui.radio_value(&mut self.on_loop, true, "On the loop")
                        .on_disabled_hover_text("Turn the loop on to pin a comment to it");
                });
            }
        });
        let field = ui.add(
            egui::TextEdit::multiline(&mut self.draft)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("What should we look at here?"),
        );
        let field = field.accessible_name("New comment");
        if std::mem::take(&mut self.focus_draft) {
            field.request_focus();
        }
        let send = ui
            .add_enabled(
                !self.draft.trim().is_empty(),
                egui::Button::new("Add comment"),
            )
            .on_hover_text("Ctrl+Enter in the text adds it too")
            .clicked()
            || (field.has_focus()
                && !self.draft.trim().is_empty()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)));
        if send {
            let c = Comment {
                id: daw_project::new_id(),
                author: cx.me.to_string(),
                parent: None,
                track: target.track,
                start: target.start.max(0),
                end: target.end.filter(|&e| e > target.start),
                text: self.draft.trim().to_string(),
                resolved: false,
                created: now(),
            };
            self.selected = Some(c.id.clone());
            comments.push(c);
            self.draft.clear();
            self.target = None;
        }
    }

    fn thread(
        &mut self,
        ui: &mut egui::Ui,
        comments: &mut Vec<Comment>,
        root: &Comment,
        cx: &Ctx,
        out: &mut Outcome,
    ) {
        let selected = self.selected.as_ref() == Some(&root.id);
        let stroke = if selected {
            egui::Stroke::new(1.5, COLOUR)
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke
        };
        egui::Frame::group(ui.style())
            .stroke(stroke)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let where_ = format!(
                        "{}, {}",
                        track_name(cx.tracks, &root.track),
                        place(cx.map, root.start, root.end)
                    );
                    let r = ui.add(
                        egui::Label::new(RichText::new(where_).strong())
                            .sense(egui::Sense::click()),
                    );
                    if r.on_hover_text("Show on the timeline").clicked() {
                        self.selected = Some(root.id.clone());
                    }
                    if root.resolved {
                        ui.label(RichText::new("resolved").weak());
                    }
                });
                let replies: Vec<Comment> = comments
                    .iter()
                    .filter(|c| c.parent.as_ref() == Some(&root.id))
                    .cloned()
                    .collect();
                self.one(ui, comments, root, cx);
                for r in &replies {
                    ui.indent(&r.id, |ui| self.one(ui, comments, r, cx));
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("▶ Play from here")
                        .on_hover_text("Move the playhead here and play")
                        .clicked()
                    {
                        self.selected = Some(root.id.clone());
                        out.play_from = Some(root.start);
                    }
                    if self.reply.as_ref().is_none_or(|(id, _)| *id != root.id)
                        && ui.button("Reply").clicked()
                    {
                        self.reply = Some((root.id.clone(), String::new()));
                    }
                    let label = if root.resolved { "Reopen" } else { "Resolve" };
                    if ui.button(label).clicked()
                        && let Some(c) = comments.iter_mut().find(|c| c.id == root.id)
                    {
                        c.resolved = !c.resolved;
                    }
                });
                if self.reply.as_ref().is_some_and(|(id, _)| *id == root.id)
                    && let Some((id, mut text)) = self.reply.take()
                {
                    let field = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("Reply"),
                    );
                    let field = field.accessible_name("Reply");
                    let mut keep = true;
                    ui.horizontal(|ui| {
                        let send = ui
                            .add_enabled(!text.trim().is_empty(), egui::Button::new("Send reply"))
                            .clicked()
                            || (field.has_focus()
                                && !text.trim().is_empty()
                                && ui.input_mut(|i| {
                                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)
                                }));
                        if send {
                            comments.push(Comment {
                                id: daw_project::new_id(),
                                author: cx.me.to_string(),
                                parent: Some(root.id.clone()),
                                track: None,
                                start: root.start,
                                end: None,
                                text: text.trim().to_string(),
                                resolved: false,
                                created: now(),
                            });
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                    if keep {
                        self.reply = Some((id, text));
                    }
                }
            });
    }

    /// One comment: who wrote it, the text, and its own buttons.
    fn one(&mut self, ui: &mut egui::Ui, comments: &mut Vec<Comment>, c: &Comment, cx: &Ctx) {
        ui.label(RichText::new(author_name(&c.author, cx.me, cx.members)).weak());
        if let Some((id, text)) = &mut self.editing
            && *id == c.id
        {
            let field = ui.add(
                egui::TextEdit::multiline(text)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            field.accessible_name("Comment text");
            let mut done = None;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!text.trim().is_empty(), egui::Button::new("Save"))
                    .clicked()
                {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
            if let Some(save) = done {
                let text = text.trim().to_string();
                if save && let Some(m) = comments.iter_mut().find(|m| m.id == c.id) {
                    m.text = text;
                }
                self.editing = None;
            }
            return;
        }
        ui.add(egui::Label::new(&c.text).wrap());
        if !may_change(c, cx.me, cx.may_change_any) {
            return;
        }
        ui.horizontal(|ui| {
            if self.deleting.as_ref() == Some(&c.id) {
                let what = if c.parent.is_none()
                    && comments.iter().any(|r| r.parent.as_ref() == Some(&c.id))
                {
                    "Delete it and its replies for good"
                } else {
                    "Delete for good"
                };
                if ui
                    .button(RichText::new(what).color(Color32::LIGHT_RED))
                    .on_hover_text("Also removes it from the saved file and anything shared later")
                    .clicked()
                {
                    delete(comments, &c.id);
                    self.deleting = None;
                }
                if ui.small_button("Keep").clicked() {
                    self.deleting = None;
                }
            } else {
                if ui
                    .small_button("Edit")
                    .accessible_name(format!("Edit comment: {}", short(&c.text)))
                    .clicked()
                {
                    self.editing = Some((c.id.clone(), c.text.clone()));
                }
                if ui
                    .small_button("Delete")
                    .accessible_name(format!("Delete comment: {}", short(&c.text)))
                    .clicked()
                {
                    self.deleting = Some(c.id.clone());
                }
            }
        });
    }

    /// The pins to draw on the timeline.
    pub fn pins(&self, comments: &[Comment], track_ids: &[String]) -> Vec<Pin> {
        threads(comments)
            .into_iter()
            .filter(|c| self.show_resolved || !c.resolved)
            .map(|c| {
                let lane = match &c.track {
                    None => None,
                    // A comment on a deleted track goes on the whole song.
                    Some(t) => track_ids.iter().position(|id| id == t),
                };
                Pin {
                    id: c.id.clone(),
                    lane,
                    start: c.start,
                    end: c.end,
                    resolved: c.resolved,
                    selected: self.selected.as_ref() == Some(&c.id),
                    text: short(&c.text),
                }
            })
            .collect()
    }
}

/// The first comment of each thread. Replies whose thread has gone are
/// left out.
pub fn threads(comments: &[Comment]) -> Vec<&Comment> {
    comments.iter().filter(|c| c.parent.is_none()).collect()
}

/// Deletes a comment, and its replies if it starts a thread.
pub fn delete(comments: &mut Vec<Comment>, id: &str) {
    comments.retain(|c| c.id != id && c.parent.as_deref() != Some(id));
}

/// Whether this user may edit or delete `c`: the owner and editors
/// any, commenters only their own (the project document refuses other
/// changes from them anyway).
pub fn may_change(c: &Comment, me: &str, any: bool) -> bool {
    any || (!c.author.is_empty() && c.author == me)
}

/// Who wrote a comment, for showing: their name as the project's
/// members list has it, so a renamed person shows their new name.
pub fn author_name(author: &str, me: &str, members: &[daw_project::Member]) -> String {
    if !author.is_empty() && author == me {
        return "You".into();
    }
    members
        .iter()
        .find(|m| m.key == author)
        .map_or("Someone else".into(), daw_project::Member::shown_name)
}

fn track_name(tracks: &[(String, String)], id: &Option<String>) -> String {
    match id {
        None => "Whole song".into(),
        Some(id) => tracks
            .iter()
            .find(|(t, _)| t == id)
            .map_or("Whole song".into(), |(_, n)| n.clone()),
    }
}

/// "bar 5 beat 3", or "bars 5 to 9" for a range on bar lines.
pub fn place(map: &TempoMap, start: i64, end: Option<i64>) -> String {
    let at = |f: i64| map.bar_beat(map.ticks_at(Frames(f)));
    let a = at(start);
    match end {
        None => format!("bar {} beat {}", a.bar, a.beat),
        Some(e) => {
            let b = at(e);
            if a.beat == 1 && a.tick == 0 && b.beat == 1 && b.tick == 0 {
                format!("bars {} to {}", a.bar, b.bar)
            } else {
                format!(
                    "bar {} beat {} to bar {} beat {}",
                    a.bar, a.beat, b.bar, b.beat
                )
            }
        }
    }
}

/// The start of a comment, for a label or a tooltip.
fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    match line.char_indices().nth(40) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::time::{Tempo, TimeSignature};

    fn c(id: &str, parent: Option<&str>) -> Comment {
        Comment {
            id: id.into(),
            author: "me".into(),
            parent: parent.map(Into::into),
            track: None,
            start: 0,
            end: None,
            text: "Hi".into(),
            resolved: false,
            created: 0,
        }
    }

    #[test]
    fn deleting_a_thread_takes_its_replies() {
        let mut all = vec![
            c("a", None),
            c("r1", Some("a")),
            c("b", None),
            c("r2", Some("b")),
        ];
        delete(&mut all, "r2");
        assert_eq!(all.len(), 3);
        delete(&mut all, "a");
        let ids: Vec<&str> = all.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["b"]);
    }

    #[test]
    fn places_read_as_bars_and_beats() {
        let map = TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON);
        // A beat is 24 000 frames, a bar 96 000.
        assert_eq!(place(&map, 0, None), "bar 1 beat 1");
        assert_eq!(place(&map, 96_000 + 48_000, None), "bar 2 beat 3");
        assert_eq!(place(&map, 96_000 * 4, Some(96_000 * 8)), "bars 5 to 9");
        assert_eq!(
            place(&map, 24_000, Some(96_000)),
            "bar 1 beat 2 to bar 2 beat 1"
        );
    }

    #[test]
    fn long_text_is_cut_for_labels() {
        assert_eq!(short("Rushing\nhere"), "Rushing");
        assert_eq!(short(&"a".repeat(50)), format!("{}…", "a".repeat(40)));
    }
}
