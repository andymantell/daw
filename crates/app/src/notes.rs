// SPDX-License-Identifier: GPL-3.0-or-later
//! Lesson notes (roadmap 1.7): the project's notes, written with a few
//! simple marks and shown formatted, and blocks of tab pinned to the
//! song's sections. Both live in the project document, so they travel
//! with bundles and shared folders.
//!
//! The marks are typed into the text itself, so the notes stay plain
//! text that merges letter by letter when two people type at once:
//! `# ` and `## ` start headings, `- ` and `1. ` list items, `**` goes
//! around bold words, and a line of three backticks starts and ends a
//! fixed-width block. Lines that look like tab (`e|--3--|`) are shown
//! fixed-width anyway.

use std::time::{SystemTime, UNIX_EPOCH};

use daw_project::TabBlock;
use eframe::egui::{self, RichText, text::CCursor, text::CCursorRange};

use crate::timeline::{Marker, section};

/// What a new block starts with: the six strings, high to low.
const NEW_TAB: &str = "e|----------------|\n\
                       B|----------------|\n\
                       G|----------------|\n\
                       D|----------------|\n\
                       A|----------------|\n\
                       E|----------------|";

/// What the marks are, for the formatting buttons' hover text.
const MARKS: &str = "Marks you can type: # heading, ## smaller heading, **bold**, \
                     - list item, 1. numbered item, and a line of ``` before and \
                     after fixed-width text";

#[derive(Default)]
pub struct Notes {
    pub open: bool,
    /// Writing rather than reading. `None` until first shown: then
    /// writing if there are no notes yet.
    editing: Option<bool>,
    /// A block to scroll to and outline.
    pub shown: Option<String>,
    /// Give the shown block's text the keyboard next frame.
    focus_shown: bool,
    /// The section for the next new block: a marker's id, or `None` for
    /// the whole song. Unset means the playhead's section.
    new_for: Option<Option<String>>,
}

/// What the app has, for one frame.
pub struct Ctx<'a> {
    /// In bar order.
    pub markers: &'a [Marker],
    /// The first bar after the song's end.
    pub end_bar: i64,
    pub playhead_bar: i64,
    /// Who is writing: see [`my_author_id`](crate::App::my_author_id).
    pub me: &'a str,
    /// Commenters may read notes but not change them.
    pub may_edit: bool,
}

/// What the app should do after the window is drawn.
#[derive(Default)]
pub struct Outcome {
    /// Move the playhead to the start of this bar.
    pub go_to_bar: Option<i64>,
    /// Loop these bars.
    pub loop_bars: Option<(i64, i64)>,
}

impl Notes {
    /// Opens the window with a new block of tab pinned to `marker`'s
    /// section, or to the whole song.
    pub fn add_tab(&mut self, tabs: &mut Vec<TabBlock>, marker: Option<String>, me: &str) {
        let t = TabBlock {
            id: daw_project::new_id(),
            marker,
            title: String::new(),
            text: NEW_TAB.to_string(),
            author: me.to_string(),
            created: now(),
        };
        self.open = true;
        self.editing = Some(true);
        self.shown = Some(t.id.clone());
        self.focus_shown = true;
        tabs.push(t);
    }

    /// The Lesson notes window. Edits `notes` and `tabs` in place.
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        notes: &mut String,
        tabs: &mut Vec<TabBlock>,
        cx: &Ctx,
    ) -> Outcome {
        let mut out = Outcome::default();
        let mut open = self.open;
        egui::Window::new("Lesson notes")
            .open(&mut open)
            .default_size([460.0, 480.0])
            .show(ctx, |ui| {
                let editing = *self
                    .editing
                    .get_or_insert(notes.trim().is_empty() && tabs.is_empty());
                let editing = editing && cx.may_edit;
                self.toolbar(ui, notes, editing, cx);
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if editing {
                            ui.add(
                                egui::TextEdit::multiline(notes)
                                    .id(text_id())
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(10)
                                    .hint_text(
                                        "What to practise, chord shapes… \
                                         # for a heading, - for a list, **bold**",
                                    )
                                    .font(egui::TextStyle::Monospace),
                            )
                            .accessible_name("Lesson notes");
                        } else if notes.trim().is_empty() {
                            ui.label(RichText::new("No notes yet.").weak());
                        } else {
                            show_formatted(ui, notes);
                        }
                        ui.add_space(8.0);
                        self.tab_blocks(ui, tabs, editing, cx, &mut out);
                    });
            });
        self.open = open;
        out
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, notes: &mut String, editing: bool, cx: &Ctx) {
        ui.horizontal_wrapped(|ui| {
            if !cx.may_edit {
                ui.label(RichText::new("Only editors can change lesson notes.").weak());
                return;
            }
            let mut e = editing;
            ui.selectable_value(&mut e, false, "Read");
            ui.selectable_value(&mut e, true, "Write");
            self.editing = Some(e);
            if !editing {
                return;
            }
            ui.separator();
            let id = text_id();
            for (label, what, mark) in [
                ("Heading", "Make the line a heading (#)", Mark::Line("# ")),
                ("Bold", "Make the selected words bold (**)", Mark::Bold),
                ("List", "Make the lines list items (-)", Mark::Line("- ")),
            ] {
                let r = ui.button(label).on_hover_text(format!("{what}. {MARKS}."));
                if r.clicked() {
                    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                    let n = notes.chars().count();
                    let (a, b) = state.cursor.char_range().map_or((n, n), |r| {
                        let (a, b) = (r.primary.index.0, r.secondary.index.0);
                        (a.min(b).min(n), a.max(b).min(n))
                    });
                    let (text, (a, b)) = match mark {
                        Mark::Bold => toggle_bold(notes, a, b),
                        Mark::Line(p) => toggle_prefix(notes, a, b, p),
                    };
                    *notes = text;
                    state
                        .cursor
                        .set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
                    state.store(ui.ctx(), id);
                    ui.memory_mut(|m| m.request_focus(id));
                }
            }
        });
    }

    fn tab_blocks(
        &mut self,
        ui: &mut egui::Ui,
        tabs: &mut Vec<TabBlock>,
        editing: bool,
        cx: &Ctx,
        out: &mut Outcome,
    ) {
        ui.heading("Tab");
        if editing {
            ui.horizontal_wrapped(|ui| {
                let at = section_at(cx.markers, cx.playhead_bar);
                let mut pick = self.new_for.clone().unwrap_or(at.clone());
                if ui.button("Add tab block").clicked() {
                    self.add_tab(tabs, pick.clone(), cx.me);
                }
                ui.label("for");
                section_picker(ui, "new tab section", &mut pick, cx.markers)
                    .accessible_name("Section for the new tab block");
                self.new_for = (pick != at).then_some(pick);
            });
        }
        if tabs.is_empty() {
            ui.label(
                RichText::new("No tab yet. Add a block to write a riff for a section.").weak(),
            );
            return;
        }
        let mut order: Vec<(i64, i64, String)> = (tabs.iter())
            .map(|t| (sort_bar(cx.markers, &t.marker), t.created, t.id.clone()))
            .collect();
        order.sort();
        let mut delete = None;
        for (_, _, id) in order {
            let Some(t) = tabs.iter_mut().find(|t| t.id == id) else {
                continue;
            };
            if self.block(ui, t, editing, cx, out) {
                delete = Some(id);
            }
        }
        if let Some(id) = delete {
            tabs.retain(|t| t.id != id);
        }
    }

    /// One block. True if Delete was pressed.
    fn block(
        &mut self,
        ui: &mut egui::Ui,
        t: &mut TabBlock,
        editing: bool,
        cx: &Ctx,
        out: &mut Outcome,
    ) -> bool {
        let shown = self.shown.as_ref() == Some(&t.id);
        let stroke = if shown {
            egui::Stroke::new(1.5, ui.visuals().selection.stroke.color)
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke
        };
        let i = marker_index(cx.markers, &t.marker);
        let mut delete = false;
        let r = egui::Frame::group(ui.style())
            .stroke(stroke)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(section_label(cx, &t.marker)).strong());
                    if let Some(i) = i {
                        let bars = section(cx.markers, i, cx.end_bar);
                        if ui
                            .small_button("Go to section")
                            .on_hover_text("Move the playhead to the start of this section")
                            .clicked()
                        {
                            self.shown = Some(t.id.clone());
                            out.go_to_bar = Some(bars.0);
                        }
                        if ui
                            .small_button("Loop section")
                            .on_hover_text("Loop this section")
                            .clicked()
                        {
                            self.shown = Some(t.id.clone());
                            out.loop_bars = Some(bars);
                        }
                    }
                });
                if editing {
                    ui.horizontal(|ui| {
                        ui.label("Section");
                        let mut pick = t.marker.clone().filter(|_| i.is_some());
                        let salt = format!("tab section {}", t.id);
                        section_picker(ui, &salt, &mut pick, cx.markers)
                            .accessible_name(format!("Section for {}", block_name(t)));
                        if pick != t.marker && (i.is_some() || pick.is_some()) {
                            t.marker = pick;
                        }
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut t.title)
                            .desired_width(f32::INFINITY)
                            .hint_text("Title, such as Intro riff"),
                    )
                    .accessible_name("Tab block title");
                    let text = ui.add(
                        egui::TextEdit::multiline(&mut t.text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(6),
                    );
                    let text = text.accessible_name(format!("Tab: {}", block_name(t)));
                    if shown && std::mem::take(&mut self.focus_shown) {
                        text.request_focus();
                    }
                    delete = ui
                        .button("Delete block")
                        .on_hover_text("Undo (Ctrl+Z) brings it back")
                        .clicked();
                } else {
                    if !t.title.trim().is_empty() {
                        ui.label(RichText::new(t.title.trim()).strong());
                    }
                    ui.add(egui::Label::new(RichText::new(&t.text).monospace()).extend());
                }
            });
        if shown && self.focus_shown {
            r.response.scroll_to_me(Some(egui::Align::Center));
        }
        delete
    }
}

enum Mark {
    Bold,
    /// A mark at the start of each line.
    Line(&'static str),
}

fn text_id() -> egui::Id {
    egui::Id::unique("lesson notes text")
}

/// The marker whose section `bar` is in, if any.
fn section_at(markers: &[Marker], bar: i64) -> Option<String> {
    markers
        .iter()
        .rev()
        .find(|m| m.bar <= bar)
        .map(|m| m.id.clone())
}

fn marker_index(markers: &[Marker], id: &Option<String>) -> Option<usize> {
    let id = id.as_ref()?;
    markers.iter().position(|m| m.id == *id)
}

/// Where a block goes in the list: whole-song blocks first, then in
/// song order, then those whose marker has gone.
fn sort_bar(markers: &[Marker], id: &Option<String>) -> i64 {
    match (id, marker_index(markers, id)) {
        (None, _) => 0,
        (Some(_), Some(i)) => markers[i].bar,
        (Some(_), None) => i64::MAX,
    }
}

fn section_label(cx: &Ctx, id: &Option<String>) -> String {
    match (id, marker_index(cx.markers, id)) {
        (None, _) => "Whole song".into(),
        (Some(_), Some(i)) => {
            let (a, b) = section(cx.markers, i, cx.end_bar);
            format!("{}, bars {a} to {}", cx.markers[i].name, b - 1)
        }
        (Some(_), None) => "No section: its marker was deleted".into(),
    }
}

fn block_name(t: &TabBlock) -> &str {
    match t.title.trim() {
        "" => "untitled",
        s => s,
    }
}

fn section_picker(
    ui: &mut egui::Ui,
    salt: &str,
    pick: &mut Option<String>,
    markers: &[Marker],
) -> egui::Response {
    let name = |id: &Option<String>| match id {
        None => "Whole song".to_string(),
        Some(id) => (markers.iter().find(|m| m.id == *id))
            .map_or_else(|| "Choose a section".into(), |m| m.name.clone()),
    };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(name(pick))
        .show_ui(ui, |ui| {
            ui.selectable_value(pick, None, "Whole song");
            for m in markers {
                ui.selectable_value(pick, Some(m.id.clone()), &m.name);
            }
        })
        .response
}

/// One line of notes, as shown.
#[derive(Debug, PartialEq)]
pub enum Line<'a> {
    /// Level 1 or 2, and the words.
    Heading(u8, &'a str),
    Bullet(&'a str),
    /// The number as typed, and the words.
    Numbered(&'a str, &'a str),
    /// Shown in a fixed-width font, as written.
    Fixed(&'a str),
    Text(&'a str),
    Blank,
}

/// How each line of `text` is shown. The lines of three backticks that
/// open and close a fixed-width block are left out.
pub fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut fixed = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fixed = !fixed;
            continue;
        }
        if fixed || looks_like_tab(line) {
            out.push(Line::Fixed(line));
            continue;
        }
        let t = line.trim_start();
        out.push(if t.is_empty() {
            Line::Blank
        } else if let Some(s) = t.strip_prefix("## ") {
            Line::Heading(2, s.trim())
        } else if let Some(s) = t.strip_prefix("# ") {
            Line::Heading(1, s.trim())
        } else if let Some(s) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            Line::Bullet(s)
        } else if let Some((n, s)) = t.split_once(". ")
            && !n.is_empty()
            && n.len() <= 3
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            Line::Numbered(n, s)
        } else {
            Line::Text(line)
        });
    }
    out
}

/// A line of tab: a string's name, then a bar line, such as `e|--3--|`
/// or `F#|-0-`.
fn looks_like_tab(line: &str) -> bool {
    let t = line.trim_start();
    let mut c = t.chars();
    let Some(note) = c.next() else { return false };
    let rest = c.as_str();
    let rest = rest
        .strip_prefix('#')
        .or_else(|| rest.strip_prefix('b'))
        .unwrap_or(rest);
    "ABCDEFGabcdefg".contains(note) && rest.starts_with('|') && rest.contains('-')
}

/// `line` split into plain and bold stretches. An unclosed `**` is
/// shown as typed.
pub fn spans(line: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(a) = rest.find("**") {
        let after = &rest[a + 2..];
        let Some(b) = after.find("**") else { break };
        if b == 0 {
            // "****" is nothing in bold: show it as typed.
            out.push((false, &rest[..a + 4]));
            rest = &after[2..];
            continue;
        }
        if a > 0 {
            out.push((false, &rest[..a]));
        }
        out.push((true, &after[..b]));
        rest = &after[b + 2..];
    }
    if !rest.is_empty() {
        out.push((false, rest));
    }
    out
}

/// Draws notes with their marks turned into formatting.
fn show_formatted(ui: &mut egui::Ui, notes: &str) {
    let job = |ui: &egui::Ui, s: &str, size: Option<f32>| {
        let mut job = egui::text::LayoutJob::default();
        for (bold, part) in spans(s) {
            let mut t = RichText::new(part);
            if bold {
                t = t.strong();
            }
            if let Some(size) = size {
                t = t.size(size).strong();
            }
            t.append_to(
                &mut job,
                ui.style(),
                egui::FontSelection::Default,
                egui::Align::Center,
            );
        }
        job
    };
    let body = egui::TextStyle::Body.resolve(ui.style()).size;
    for line in lines(notes) {
        match line {
            Line::Heading(level, s) => {
                ui.add_space(4.0);
                let size = if level == 1 { body * 1.5 } else { body * 1.2 };
                ui.add(egui::Label::new(job(ui, s, Some(size))).wrap());
            }
            Line::Bullet(s) => {
                ui.horizontal(|ui| {
                    ui.label("•");
                    ui.add(egui::Label::new(job(ui, s, None)).wrap());
                });
            }
            Line::Numbered(n, s) => {
                ui.horizontal(|ui| {
                    ui.label(format!("{n}."));
                    ui.add(egui::Label::new(job(ui, s, None)).wrap());
                });
            }
            Line::Fixed(s) => {
                ui.add(egui::Label::new(RichText::new(s).monospace()).extend());
            }
            Line::Text(s) => {
                ui.add(egui::Label::new(job(ui, s, None)).wrap());
            }
            Line::Blank => {
                ui.add_space(body * 0.6);
            }
        }
    }
}

/// The byte offset of character `i` in `s`.
fn byte_at(s: &str, i: usize) -> usize {
    s.char_indices().nth(i).map_or(s.len(), |(b, _)| b)
}

/// Puts `**` around characters `a..b`, or takes them away if they are
/// there already. With nothing selected, adds a pair to type between.
/// Returns the new text and the selection, in characters.
pub fn toggle_bold(text: &str, a: usize, b: usize) -> (String, (usize, usize)) {
    let (ba, bb) = (byte_at(text, a), byte_at(text, b));
    if ba >= 2 && text[..ba].ends_with("**") && text[bb..].starts_with("**") {
        let out = format!("{}{}{}", &text[..ba - 2], &text[ba..bb], &text[bb + 2..]);
        return (out, (a - 2, b - 2));
    }
    let out = format!("{}**{}**{}", &text[..ba], &text[ba..bb], &text[bb..]);
    (out, (a + 2, b + 2))
}

/// Starts each line touching characters `a..b` with `mark`, or takes it
/// off if they all start with it already. Returns the new text and the
/// selection, in characters.
pub fn toggle_prefix(text: &str, a: usize, b: usize, mark: &str) -> (String, (usize, usize)) {
    let (ba, bb) = (byte_at(text, a), byte_at(text, b));
    let first = text[..ba].rfind('\n').map_or(0, |i| i + 1);
    let last = text[bb..].find('\n').map_or(text.len(), |i| bb + i);
    let block = &text[first..last];
    let all = block.lines().all(|l| l.starts_with(mark)) && !block.is_empty();
    let lines: Vec<String> = block
        .split('\n')
        .map(|l| {
            if all {
                l[mark.len()..].to_string()
            } else {
                // A heading or list mark already there is swapped.
                let bare = ["## ", "# ", "- ", "* "]
                    .iter()
                    .find_map(|m| l.strip_prefix(m))
                    .unwrap_or(l);
                format!("{mark}{bare}")
            }
        })
        .collect();
    let new = lines.join("\n");
    let out = format!("{}{}{}", &text[..first], new, &text[last..]);
    let a = text[..first].chars().count();
    (out, (a, a + new.chars().count()))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_become_formatting() {
        let notes = "# Week 3\n\
                     ## Warm up\n\
                     - **Slow** the bridge down\n\
                     2. Then at 80%\n\
                     \n\
                     e|---3---|\n\
                     ```\n\
                     # not a heading\n\
                     ```\n\
                     Plain words";
        assert_eq!(
            lines(notes),
            [
                Line::Heading(1, "Week 3"),
                Line::Heading(2, "Warm up"),
                Line::Bullet("**Slow** the bridge down"),
                Line::Numbered("2", "Then at 80%"),
                Line::Blank,
                Line::Fixed("e|---3---|"),
                Line::Fixed("# not a heading"),
                Line::Text("Plain words"),
            ]
        );
    }

    #[test]
    fn old_plain_notes_show_as_they_were() {
        assert_eq!(
            lines("Watch the bends in bar 7.\nUse the bridge pickup."),
            [
                Line::Text("Watch the bends in bar 7."),
                Line::Text("Use the bridge pickup.")
            ]
        );
        assert_eq!(lines("Bar 12. Is hard")[0], Line::Text("Bar 12. Is hard"));
        assert_eq!(lines("Am|C|G")[0], Line::Text("Am|C|G"));
    }

    #[test]
    fn bold_stretches_are_found() {
        assert_eq!(
            spans("a **b** c **d**"),
            [(false, "a "), (true, "b"), (false, " c "), (true, "d")]
        );
        assert_eq!(spans("not **closed"), [(false, "not **closed")]);
        assert_eq!(spans("****x"), [(false, "****"), (false, "x")]);
    }

    #[test]
    fn the_bold_button_wraps_and_unwraps() {
        let (t, sel) = toggle_bold("play slowly", 5, 11);
        assert_eq!((t.as_str(), sel), ("play **slowly**", (7, 13)));
        let (t, sel) = toggle_bold(&t, sel.0, sel.1);
        assert_eq!((t.as_str(), sel), ("play slowly", (5, 11)));
        let (t, sel) = toggle_bold("née ", 4, 4);
        assert_eq!((t.as_str(), sel), ("née ****", (6, 6)));
    }

    #[test]
    fn line_buttons_mark_every_line_touched() {
        let text = "intro\nverse\nchorus";
        let (t, sel) = toggle_prefix(text, 7, 9, "- ");
        assert_eq!(t, "intro\n- verse\nchorus");
        assert_eq!(sel, (6, 13));
        let (t, _) = toggle_prefix(&t, 6, 13, "- ");
        assert_eq!(t, text, "pressing again takes it off");
        let (t, _) = toggle_prefix(text, 0, 11, "# ");
        assert_eq!(t, "# intro\n# verse\nchorus");
        let (t, _) = toggle_prefix("- verse", 0, 0, "# ");
        assert_eq!(t, "# verse", "a list item becomes a heading");
    }

    #[test]
    fn new_blocks_go_in_the_playheads_section() {
        let m = |id: &str, bar| Marker {
            id: id.into(),
            bar,
            name: id.into(),
        };
        let markers = [m("intro", 1), m("verse", 5)];
        assert_eq!(section_at(&markers, 3).as_deref(), Some("intro"));
        assert_eq!(section_at(&markers, 7).as_deref(), Some("verse"));
        assert_eq!(section_at(&markers[1..], 2), None);
        assert_eq!(sort_bar(&markers, &Some("gone".into())), i64::MAX);
    }

    mod window {
        use eframe::egui;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;

        use crate::App;
        use crate::timeline::Marker;

        fn harness() -> Harness<'static, App> {
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let app = h.state_mut();
            for (id, bar) in [("intro", 1), ("verse", 5)] {
                app.markers.push(Marker {
                    id: id.into(),
                    bar,
                    name: format!("Section {id}"),
                });
            }
            app.notes_ui.open = true;
            h.run();
            h
        }

        #[test]
        fn notes_are_written_with_buttons_and_read_formatted() {
            let mut h = harness();
            h.state_mut().notes = "Week 3".into();
            h.run();
            h.get_by_label("Heading").click();
            h.run();
            assert_eq!(h.state().notes, "# Week 3");
            h.get_by_label("Read").click();
            h.run();
            assert!(
                (h.query_by_role(egui::accesskit::Role::MultilineTextInput)).is_none(),
                "no text field"
            );
            assert!(h.query_by_label("Week 3").is_some(), "the heading shows");
            assert!(h.query_by_label("# Week 3").is_none());
        }

        #[test]
        fn a_tab_block_goes_to_its_section_and_is_saved() {
            let mut h = harness();
            h.get_by_label("Add tab block").click();
            h.run();
            assert_eq!(h.state().tabs.len(), 1);
            assert_eq!(h.state().tabs[0].marker.as_deref(), Some("intro"));

            // Moved to the verse, it takes the playhead there.
            h.state_mut().tabs[0].marker = Some("verse".into());
            h.run();
            h.get_by_label("Go to section").click();
            h.run();
            let app = h.state();
            let map = app.tempo_map();
            let want = map.frames_at(map.bar_start(5));
            for _ in 0..100 {
                h.step();
                if h.state().ctl.status().position() == want {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert_eq!(h.state().ctl.status().position(), want);

            // It is part of the project, and comes back with it.
            let ctx = h.ctx.clone();
            h.state_mut().sync_doc(&ctx, true);
            let p = h.state().proj.doc.get();
            assert_eq!(p.tabs, h.state().tabs);
            h.state_mut().tabs.clear();
            h.state_mut().apply(p);
            assert_eq!(h.state().tabs.len(), 1);
            assert!(h.state().tabs[0].text.starts_with("e|"));
        }

        #[test]
        fn a_markers_menu_adds_tab_for_its_section() {
            let mut h = harness();
            h.state_mut().notes_ui.open = false;
            let me = h.state().my_author_id();
            let app = h.state_mut();
            app.notes_ui
                .add_tab(&mut app.tabs, Some("verse".into()), &me);
            h.run();
            assert!(h.state().notes_ui.open);
            assert!(h.query_by_label("Section verse, bars 5 to 5").is_some());
        }
    }
}
