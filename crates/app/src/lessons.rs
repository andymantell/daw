// SPDX-License-Identifier: GPL-3.0-or-later
//! Lessons (roadmap 3.7): assignments with the progress made on them,
//! the practice log, and the lessons so far on a timeline.
//!
//! Practice is logged as you play (see `practice.rs`); nobody types it
//! in. An assignment's progress is worked out from the log
//! ([`daw_project::progress`]), so teacher and student see the same
//! numbers once their copies have met, through a bundle, the shared
//! folder or a live session. Starting a lesson takes a snapshot named
//! after it, so What changed since… shows what was done after it.
//!
//! Dates are shown in UTC: the app has no time zone table.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use daw_project::{Assignment, DoneMark, Lesson, PracticeSession, progress};
use eframe::egui::{self, RichText, Sense, Stroke, pos2, vec2};

use crate::App;
use crate::timeline::{Marker, section};

pub(crate) const DAY_MS: i64 = 86_400_000;

/// Time in seconds, times round the loop and top tempo, added up.
type Totals = (f64, u64, f64);

/// The bars looped, if any, and whose practice it was.
type SectionWho<'a> = (Option<(i64, i64)>, &'a str);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Tab {
    #[default]
    Assignments,
    Log,
    History,
}

/// How far back the practice log looks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Span {
    #[default]
    SinceLesson,
    Week,
    All,
}

#[derive(Default)]
pub(crate) struct Lessons {
    pub(crate) tab: Tab,
    span: Span,
    /// An assignment being set, or changed.
    draft: Option<Draft>,
    /// The title for the next lesson.
    title: String,
    /// A lesson being renamed, and its title so far.
    renaming: Option<(String, String)>,
    /// The log and lessons as the document had them at a version, so
    /// they aren't read out of it every frame.
    data: Option<(Vec<u8>, Data)>,
}

#[derive(Default)]
pub(crate) struct Data {
    log: Vec<PracticeSession>,
    done: Vec<DoneMark>,
    lessons: Vec<Lesson>,
}

/// An assignment as it is being written.
struct Draft {
    /// The assignment it changes, or `None` for a new one.
    id: Option<String>,
    /// First and last bar, both played.
    from: i64,
    last: i64,
    bpm: f64,
    /// Due in this many days, or 0 for no date.
    days: i64,
    note: String,
}

/// What the app has, for one frame.
pub(crate) struct Ctx<'a> {
    /// In bar order.
    pub(crate) markers: &'a [Marker],
    /// The first bar after the song's end.
    pub(crate) end_bar: i64,
    pub(crate) loop_bars: (i64, i64),
    /// The song's tempo where the loop starts.
    pub(crate) tempo: f64,
    /// Whose log is this computer's: see [`App::my_log_id`].
    pub(crate) me: &'a str,
    /// Who is setting things: see [`App::my_author_id`].
    pub(crate) author: &'a str,
    /// Commenters may keep a log and say they've done an assignment,
    /// but not set assignments or start lessons.
    pub(crate) may_edit: bool,
    /// A log's owner, for showing.
    pub(crate) name: &'a dyn Fn(&str) -> String,
    pub(crate) now: i64,
}

/// What the app should do after the pane is drawn.
#[derive(Default)]
pub(crate) struct Outcome {
    /// Loop the assignment's bars, with the trainer aiming at its tempo.
    pub(crate) practise: Option<Assignment>,
    /// Say this user has done an assignment, or take that back.
    pub(crate) done: Option<(String, bool)>,
    /// Start a lesson with this title.
    pub(crate) start_lesson: Option<String>,
    pub(crate) rename_lesson: Option<(String, String)>,
    pub(crate) delete_lesson: Option<String>,
    /// Show what changed since this snapshot.
    pub(crate) show_changes: Option<String>,
    /// Take this session out of this user's log.
    pub(crate) forget: Option<String>,
}

impl Lessons {
    /// Reads the log and lessons again if `doc` has changed.
    pub(crate) fn refresh(&mut self, doc: &daw_project::ProjectDoc) {
        let v = doc.version();
        if self.data.as_ref().is_none_or(|(at, _)| *at != v) {
            let d = Data {
                log: doc.practice_log(),
                done: doc.done_marks(),
                lessons: doc.lessons(),
            };
            self.data = Some((v, d));
        }
    }

    /// The Lessons pane, for the log and lessons last
    /// [refreshed](Self::refresh). Edits `assignments` in place.
    pub(crate) fn pane(
        &mut self,
        ui: &mut egui::Ui,
        assignments: &mut Vec<Assignment>,
        cx: &Ctx,
    ) -> Outcome {
        let mut out = Outcome::default();
        let (version, data) = self.data.take().unwrap_or_default();
        ui.horizontal(|ui| {
            for (t, name) in [
                (Tab::Assignments, "Assignments"),
                (Tab::Log, "Practice log"),
                (Tab::History, "History"),
            ] {
                ui.selectable_value(&mut self.tab, t, name);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                Tab::Assignments => self.assignments(ui, assignments, &data, cx, &mut out),
                Tab::Log => self.log(ui, &data, cx, &mut out),
                Tab::History => self.history(ui, assignments, &data, cx, &mut out),
            });
        self.data = Some((version, data));
        out
    }

    fn assignments(
        &mut self,
        ui: &mut egui::Ui,
        list: &mut Vec<Assignment>,
        data: &Data,
        cx: &Ctx,
        out: &mut Outcome,
    ) {
        if !cx.may_edit {
            ui.label(RichText::new("Only editors can set assignments.").weak());
        } else if self.draft.is_none()
            && ui
                .button("Set an assignment")
                .on_hover_text("Some bars to play at a tempo, by a date")
                .clicked()
        {
            self.draft = Some(Draft {
                id: None,
                from: cx.loop_bars.0,
                last: cx.loop_bars.1 - 1,
                bpm: cx.tempo.round(),
                days: 7,
                note: String::new(),
            });
        }
        if cx.may_edit {
            self.draft_form(ui, list, data, cx);
        }
        if list.is_empty() {
            ui.label(RichText::new("No assignments yet.").weak());
            return;
        }
        let mut change = None;
        let mut delete = None;
        // Newest first.
        for (i, a) in list.iter().enumerate().rev() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title(cx.markers, a)).strong());
                let mut when = match a.due {
                    0 => "No date".to_string(),
                    t => format!("Due {}", day(t, cx.now)),
                };
                if let Some(l) =
                    (a.lesson.as_ref()).and_then(|id| data.lessons.iter().find(|l| &l.id == id))
                {
                    when.push_str(&format!(" · set in {}", l.title));
                }
                let progress = progress(a, &data.log, &data.done);
                let reached = progress.iter().any(|p| p.reached(a) || p.done.is_some());
                if a.due != 0 && a.due < cx.now && !reached {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("{when} (past)"));
                } else {
                    ui.label(RichText::new(when).weak());
                }
                if !a.note.trim().is_empty() {
                    ui.label(&a.note);
                }
                if progress.is_empty() {
                    ui.label(RichText::new("Not practised yet.").weak());
                }
                for p in &progress {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{}:", (cx.name)(&p.who)));
                        let text = if p.best_bpm > 0.0 {
                            format!("{:.0} of {:.0} bpm", p.best_bpm, a.bpm)
                        } else {
                            "not yet all the bars".into()
                        };
                        ui.add(
                            egui::ProgressBar::new(p.fraction(a) as f32)
                                .desired_width(150.0)
                                .text(text),
                        );
                        ui.label(
                            RichText::new(format!("{} · {} loops", duration(p.seconds), p.loops))
                                .weak(),
                        );
                        if p.reached(a) {
                            ui.label("Tempo reached");
                        }
                        if let Some(t) = p.done {
                            ui.label(format!("Done {}", day(t, cx.now)));
                        }
                    });
                }
                ui.horizontal(|ui| {
                    if ui
                        .button("Practise")
                        .on_hover_text("Loop these bars, with the trainer aiming at the tempo")
                        .clicked()
                    {
                        out.practise = Some(a.clone());
                    }
                    let mine = progress.iter().any(|p| p.who == cx.me && p.done.is_some());
                    if mine {
                        if ui.button("Not done yet").clicked() {
                            out.done = Some((a.id.clone(), false));
                        }
                    } else if ui
                        .button("Mark done")
                        .on_hover_text("Tell your teacher or student you've done it")
                        .clicked()
                    {
                        out.done = Some((a.id.clone(), true));
                    }
                    if cx.may_edit {
                        if ui.button("Change").clicked() {
                            change = Some(i);
                        }
                        if ui.button("Delete").clicked() {
                            delete = Some(i);
                        }
                    }
                });
            });
        }
        if let Some(i) = change {
            let a = &list[i];
            self.draft = Some(Draft {
                id: Some(a.id.clone()),
                from: a.bars.0,
                last: a.bars.1 - 1,
                bpm: a.bpm,
                days: match a.due {
                    0 => 0,
                    t => ((t - cx.now) as f64 / DAY_MS as f64).ceil().max(1.0) as i64,
                },
                note: a.note.clone(),
            });
        }
        if let Some(i) = delete {
            list.remove(i);
        }
    }

    /// The form for the assignment being set or changed, if any.
    fn draft_form(&mut self, ui: &mut egui::Ui, list: &mut Vec<Assignment>, data: &Data, cx: &Ctx) {
        let Some(d) = &mut self.draft else { return };
        let mut close = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label("Bars");
                ui.add(egui::DragValue::new(&mut d.from).range(1..=999))
                    .accessible_name("Assignment from bar");
                ui.label("to");
                ui.add(egui::DragValue::new(&mut d.last).range(d.from..=999))
                    .accessible_name("Assignment to bar");
                d.last = d.last.max(d.from);
                if ui.button("The loop").clicked() {
                    (d.from, d.last) = (cx.loop_bars.0, cx.loop_bars.1 - 1);
                }
                if !cx.markers.is_empty() {
                    egui::ComboBox::from_id_salt("assignment section")
                        .selected_text("Section")
                        .show_ui(ui, |ui| {
                            for (i, m) in cx.markers.iter().enumerate() {
                                if ui.selectable_label(false, &m.name).clicked() {
                                    let (f, t) = section(cx.markers, i, cx.end_bar);
                                    (d.from, d.last) = (f, t - 1);
                                }
                            }
                        })
                        .response
                        .accessible_name("Assignment bars from a section");
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("At");
                ui.add(
                    egui::DragValue::new(&mut d.bpm)
                        .range(20.0..=400.0)
                        .speed(0.5)
                        .fixed_decimals(0)
                        .suffix(" bpm"),
                )
                .accessible_name("Assignment tempo");
                ui.label("due in");
                ui.add(
                    egui::DragValue::new(&mut d.days)
                        .range(0..=90)
                        .suffix(" days"),
                )
                .accessible_name("Assignment due in days");
                let when = match d.days {
                    0 => "no date".to_string(),
                    n => day(due_in(cx.now, n), cx.now),
                };
                ui.label(RichText::new(when).weak());
            });
            ui.add(
                egui::TextEdit::singleline(&mut d.note)
                    .hint_text("What to listen for, how to play it…")
                    .desired_width(f32::INFINITY),
            )
            .accessible_name("Assignment note");
            ui.horizontal(|ui| {
                let label = if d.id.is_some() { "Save" } else { "Set" };
                if ui.button(label).clicked() {
                    let was = (d.id.as_ref()).and_then(|id| list.iter().position(|a| &a.id == id));
                    let a = Assignment {
                        id: d.id.clone().unwrap_or_else(daw_project::new_id),
                        bars: (d.from, d.last + 1),
                        bpm: d.bpm,
                        due: match d.days {
                            0 => 0,
                            n => due_in(cx.now, n),
                        },
                        note: d.note.trim().to_string(),
                        lesson: match was {
                            Some(i) => list[i].lesson.clone(),
                            None => data.lessons.last().map(|l| l.id.clone()),
                        },
                        author: match was {
                            Some(i) => list[i].author.clone(),
                            None => cx.author.to_string(),
                        },
                        created: was.map_or(cx.now, |i| list[i].created),
                    };
                    match was {
                        Some(i) => list[i] = a,
                        None => list.push(a),
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.draft = None;
        }
    }

    fn log(&mut self, ui: &mut egui::Ui, data: &Data, cx: &Ctx, out: &mut Outcome) {
        let spans = [
            (Span::SinceLesson, "Since the last lesson"),
            (Span::Week, "The last 7 days"),
            (Span::All, "Everything"),
        ];
        let shown = spans
            .iter()
            .find(|(s, _)| *s == self.span)
            .map_or("", |s| s.1);
        egui::ComboBox::from_id_salt("practice log span")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (s, name) in spans {
                    ui.selectable_value(&mut self.span, s, name);
                }
            })
            .response
            .accessible_name("Practice log, how far back");
        let since = match self.span {
            Span::SinceLesson => data.lessons.last().map_or(0, |l| l.created),
            Span::Week => cx.now - 7 * DAY_MS,
            Span::All => 0,
        };
        let shown: Vec<&PracticeSession> = data.log.iter().filter(|s| s.started >= since).collect();
        if shown.is_empty() {
            ui.label(
                RichText::new(
                    "No practice logged here yet. Playing is logged as you go: \
                     each stretch from play to stop, or until the loop moves.",
                )
                .weak(),
            );
            return;
        }

        // Each person's total.
        let mut totals: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
        for s in &shown {
            let t = totals.entry(&s.who).or_default();
            t.0 += s.seconds;
            t.1 += 1;
        }
        for (who, (secs, n)) in &totals {
            ui.label(format!(
                "{}: {} over {} {}",
                (cx.name)(who),
                duration(*secs),
                n,
                if *n == 1 { "session" } else { "sessions" }
            ));
        }

        // By section.
        ui.add_space(6.0);
        ui.label(RichText::new("By section").strong());
        let mut by: BTreeMap<SectionWho, Totals> = BTreeMap::new();
        for s in &shown {
            let e = by.entry((s.bars, &s.who)).or_default();
            e.0 += s.seconds;
            e.1 += s.loops;
            e.2 = e.2.max(s.top_bpm);
        }
        egui::Grid::new("practice by section")
            .striped(true)
            .num_columns(5)
            .show(ui, |ui| {
                for h in ["Section", "Who", "Time", "Loops", "Top tempo"] {
                    ui.label(RichText::new(h).weak());
                }
                ui.end_row();
                for ((bars, who), (secs, loops, bpm)) in &by {
                    ui.label(place(cx.markers, *bars));
                    ui.label((cx.name)(who));
                    ui.label(duration(*secs));
                    ui.label(if bars.is_some() {
                        loops.to_string()
                    } else {
                        "–".into()
                    });
                    ui.label(format!("{bpm:.0} bpm"));
                    ui.end_row();
                }
            });

        // Each session, newest first.
        ui.add_space(6.0);
        ui.label(RichText::new("Sessions").strong());
        for s in shown.iter().rev().take(50) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(day_time(s.started, cx.now)).weak());
                ui.label(format!(
                    "{} · {} · {}",
                    (cx.name)(&s.who),
                    place(cx.markers, s.bars),
                    duration(s.seconds)
                ));
                if s.bars.is_some() {
                    ui.label(format!("{} loops", s.loops));
                }
                ui.label(
                    RichText::new(format!("up to {:.0}% ({:.0} bpm)", s.top_speed, s.top_bpm))
                        .weak(),
                );
                if s.who == cx.me
                    && ui
                        .small_button("Forget")
                        .on_hover_text("Take this out of your log")
                        .clicked()
                {
                    out.forget = Some(s.id.clone());
                }
            });
        }
    }

    fn history(
        &mut self,
        ui: &mut egui::Ui,
        assignments: &[Assignment],
        data: &Data,
        cx: &Ctx,
        out: &mut Outcome,
    ) {
        let lessons = &data.lessons;
        if cx.may_edit {
            ui.horizontal(|ui| {
                let next = format!("Lesson {}", lessons.len() + 1);
                let r = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.title)
                            .char_limit(daw_project::MAX_LESSON_TITLE)
                            .hint_text(&next)
                            .desired_width(160.0),
                    )
                    .accessible_name("Lesson title");
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui
                    .button("Start a lesson")
                    .on_hover_text(
                        "Takes a snapshot named after the lesson, and counts practice \
                         from here",
                    )
                    .clicked()
                    || enter
                {
                    let t = match self.title.trim() {
                        "" => next,
                        t => t.to_string(),
                    };
                    out.start_lesson = Some(t);
                    self.title.clear();
                }
            });
        } else {
            ui.label(RichText::new("Only editors can start lessons.").weak());
        }
        if lessons.is_empty() && data.log.is_empty() {
            ui.label(RichText::new("No lessons yet.").weak());
            return;
        }
        strip(ui, lessons, &data.log, cx.now);
        for (n, l) in lessons.iter().enumerate().rev() {
            let until = lessons.get(n + 1).map_or(i64::MAX, |l| l.created);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some((id, title)) = &mut self.renaming
                    && *id == l.id
                {
                    let mut done = false;
                    ui.horizontal(|ui| {
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(title)
                                    .char_limit(daw_project::MAX_LESSON_TITLE)
                                    .desired_width(160.0),
                            )
                            .accessible_name("New lesson title");
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Rename").clicked() || enter {
                            out.rename_lesson = Some((id.clone(), title.clone()));
                            done = true;
                        }
                        if ui.button("Cancel").clicked() {
                            done = true;
                        }
                    });
                    if done {
                        self.renaming = None;
                    }
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&l.title).strong());
                        ui.label(RichText::new(day(l.created, cx.now)).weak());
                    });
                }
                let set: Vec<&Assignment> = (assignments.iter())
                    .filter(|a| a.lesson.as_deref() == Some(l.id.as_str()))
                    .collect();
                for a in &set {
                    ui.label(format!("Set: {}", title(cx.markers, a)));
                }
                let after: Vec<&PracticeSession> = (data.log.iter())
                    .filter(|s| s.started >= l.created && s.started < until)
                    .collect();
                if after.is_empty() {
                    ui.label(RichText::new("No practice logged after it.").weak());
                } else {
                    let secs: f64 = after.iter().map(|s| s.seconds).sum();
                    ui.label(format!(
                        "Practice after it: {} over {} {}",
                        duration(secs),
                        after.len(),
                        if after.len() == 1 {
                            "session"
                        } else {
                            "sessions"
                        }
                    ));
                }
                ui.horizontal(|ui| {
                    if ui
                        .button("What changed since…")
                        .on_hover_text("What has been done to the song since this lesson began")
                        .clicked()
                    {
                        out.show_changes = Some(l.snapshot.clone());
                    }
                    if cx.may_edit {
                        if ui.button("Rename").clicked() {
                            self.renaming = Some((l.id.clone(), l.title.clone()));
                        }
                        if ui
                            .button("Remove")
                            .on_hover_text("Take it off this list. Its snapshot stays.")
                            .clicked()
                        {
                            out.delete_lesson = Some(l.id.clone());
                        }
                    }
                });
            });
        }
    }
}

/// The lessons and the practice between them, on a strip of days: a bar
/// for each day's practice, and a line with its number for each lesson.
fn strip(ui: &mut egui::Ui, lessons: &[Lesson], log: &[PracticeSession], now: i64) {
    let first = (lessons.iter().map(|l| l.created))
        .chain(log.iter().map(|s| s.started))
        .min()
        .unwrap_or(now);
    let today = now.div_euclid(DAY_MS);
    let start = first.div_euclid(DAY_MS).min(today - 13);
    let days = (today - start + 1) as f32;
    let mut per_day: BTreeMap<i64, f64> = BTreeMap::new();
    for s in log {
        *per_day.entry(s.started.div_euclid(DAY_MS)).or_default() += s.seconds;
    }
    let most = per_day.values().copied().fold(60.0, f64::max);

    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
    let v = ui.visuals();
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, v.extreme_bg_color);
    let w = rect.width() / days;
    let base = rect.bottom() - 14.0;
    let x_of = |ms: i64| rect.left() + (ms - start * DAY_MS) as f32 / DAY_MS as f32 * w;
    for (d, secs) in &per_day {
        let x = rect.left() + (d - start) as f32 * w;
        let h = (secs / most) as f32 * (base - rect.top() - 16.0);
        let bar = egui::Rect::from_min_max(pos2(x + w * 0.15, base - h), pos2(x + w * 0.85, base));
        p.rect_filled(bar, 1.0, v.selection.bg_fill);
    }
    let line = Stroke::new(1.5, v.strong_text_color());
    for (n, l) in lessons.iter().enumerate() {
        let x = x_of(l.created);
        p.line_segment([pos2(x, rect.top() + 14.0), pos2(x, base)], line);
        p.text(
            pos2(x, rect.top() + 2.0),
            egui::Align2::CENTER_TOP,
            (n + 1).to_string(),
            egui::FontId::proportional(11.0),
            v.strong_text_color(),
        );
    }
    let small = egui::FontId::proportional(10.0);
    p.text(
        pos2(rect.left() + 3.0, rect.bottom() - 1.0),
        egui::Align2::LEFT_BOTTOM,
        day(start * DAY_MS, now),
        small.clone(),
        v.weak_text_color(),
    );
    p.text(
        pos2(rect.right() - 3.0, rect.bottom() - 1.0),
        egui::Align2::RIGHT_BOTTOM,
        "Today",
        small,
        v.weak_text_color(),
    );
    if let Some(at) = r.hover_pos() {
        let d = start + ((at.x - rect.left()) / w).floor() as i64;
        let mut text = format!(
            "{}: {}",
            day(d * DAY_MS, now),
            match per_day.get(&d) {
                Some(s) => format!("{} practice", duration(*s)),
                None => "no practice".into(),
            }
        );
        for l in lessons.iter().filter(|l| l.created.div_euclid(DAY_MS) == d) {
            text.push_str(&format!("\n{}", l.title));
        }
        r.on_hover_text(text);
    }
}

impl App {
    /// The Lessons pane (roadmap 3.7).
    pub(crate) fn lessons_pane(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let end_bar = self.with_song(|_, song| crate::timeline::Timeline::end_bar(&map, song));
        let me = self.my_log_id();
        let author = self.my_author_id();
        let members = self.proj.doc.members();
        let my_key = self.proj.doc.my_key().map(String::from);
        let name = |who: &str| {
            if who == me || my_key.as_deref() == Some(who) {
                return "You".to_string();
            }
            (members.iter())
                .find(|m| m.key == who)
                .map_or("Someone else".into(), daw_project::Member::shown_name)
        };
        let cx = Ctx {
            markers: &self.markers,
            end_bar,
            loop_bars: self.loop_bars,
            tempo: map.tempo_at(map.bar_start(self.loop_bars.0)).as_f64(),
            me: &me,
            author: &author,
            may_edit: self.may_edit_song(),
            name: &name,
            now: now_ms(),
        };
        self.lessons_ui.refresh(&self.proj.doc);
        let out = self.lessons_ui.pane(ui, &mut self.assignments, &cx);
        self.lessons_outcome(ui.ctx(), out, &me, &author);
    }

    fn lessons_outcome(&mut self, ctx: &egui::Context, out: Outcome, me: &str, author: &str) {
        if let Some(a) = out.practise {
            self.practise(&a);
        }
        if let Some((id, done)) = out.done
            && self.proj.doc.mark_done(me, &id, done.then(now_ms))
        {
            self.proj.dirty = true;
        }
        if let Some(title) = out.start_lesson {
            // The snapshot is of the song as it is now, and practice up
            // to now counts before the lesson.
            self.sync_doc(ctx, true);
            if let Some(s) = self.practice_log.end() {
                self.proj.doc.log_practice(&s);
            }
            self.proj.doc.start_lesson(&title, author, now_ms());
            self.proj.dirty = true;
        }
        if let Some((id, title)) = out.rename_lesson
            && self.proj.doc.rename_lesson(&id, &title)
        {
            self.proj.dirty = true;
        }
        if let Some(id) = out.delete_lesson
            && self.proj.doc.delete_lesson(&id)
        {
            self.proj.dirty = true;
        }
        if let Some(id) = out.show_changes {
            self.snapshot_ui.compare_with_now(&id);
        }
        if let Some(id) = out.forget
            && self.proj.doc.forget_practice(me, &id)
        {
            self.proj.dirty = true;
        }
    }

    /// Loops an assignment's bars from their start, with the tempo
    /// trainer aiming at its tempo.
    pub(crate) fn practise(&mut self, a: &Assignment) {
        let map = self.tempo_map();
        let bpm = map.tempo_at(map.bar_start(a.bars.0)).as_f64();
        self.loop_bars = a.bars;
        self.looping = true;
        self.send_loop();
        if bpm > 0.0 {
            self.trainer.on = true;
            self.trainer.target = (a.bpm / bpm * 100.0).clamp(50.0, 150.0).round();
        }
        let f = map.frames_at(map.bar_start(a.bars.0));
        self.send(daw_engine::engine::Command::Seek(f));
    }
}

/// An assignment in a line: "“Verse”, bars 9–16 at 90 bpm".
fn title(markers: &[Marker], a: &Assignment) -> String {
    format!("{} at {:.0} bpm", place(markers, Some(a.bars)), a.bpm)
}

/// Where some practice was: the section a marker starts there, and the
/// bars, or the whole song without the loop.
fn place(markers: &[Marker], bars: Option<(i64, i64)>) -> String {
    let Some((from, to)) = bars else {
        return "Whole song".into();
    };
    let span = if to - from == 1 {
        format!("bar {from}")
    } else {
        format!("bars {from}–{}", to - 1)
    };
    match markers.iter().find(|m| m.bar == from) {
        Some(m) => format!("“{}”, {span}", m.name),
        None => {
            let mut s = span;
            s[..1].make_ascii_uppercase();
            s
        }
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The end of the day `days` days after `now`'s.
fn due_in(now: i64, days: i64) -> i64 {
    (now.div_euclid(DAY_MS) + days + 1) * DAY_MS - 1
}

/// Year, month, day and weekday (0 for Monday) of a time, in UTC.
fn civil(ms: i64) -> (i64, usize, i64, usize) {
    let days = ms.div_euclid(DAY_MS);
    // 1970-01-01 was a Thursday.
    let weekday = (days + 3).rem_euclid(7) as usize;
    // Days to a civil date (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m as usize, d, weekday)
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Thu 29 Oct", with the year if it isn't `now`'s.
pub(crate) fn day(ms: i64, now: i64) -> String {
    let (y, m, d, wd) = civil(ms);
    let mut s = format!("{} {d} {}", WEEKDAYS[wd], MONTHS[m - 1]);
    if y != civil(now).0 {
        s.push_str(&format!(" {y}"));
    }
    s
}

/// "Thu 29 Oct 19:02".
fn day_time(ms: i64, now: i64) -> String {
    let secs = ms.rem_euclid(DAY_MS) / 1000;
    format!(
        "{} {:02}:{:02}",
        day(ms, now),
        secs / 3600,
        secs % 3600 / 60
    )
}

/// "45 s", "12 min" or "1 h 05 min".
pub(crate) fn duration(secs: f64) -> String {
    let s = secs.round() as i64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {:02} min", s / 3600, s % 3600 / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_durations_read_as_words() {
        // 2026-10-29 19:02 UTC, a Thursday.
        let t = 1_793_300_520_000;
        assert_eq!(day(t, t), "Thu 29 Oct");
        assert_eq!(day_time(t, t), "Thu 29 Oct 19:02");
        assert_eq!(day(t, t + 400 * DAY_MS), "Thu 29 Oct 2026");
        assert_eq!(day(due_in(t, 7), t), "Thu 5 Nov");
        assert_eq!(day(0, 0), "Thu 1 Jan");
        assert_eq!(duration(44.6), "45 s");
        assert_eq!(duration(12.0 * 60.0 + 5.0), "12 min");
        assert_eq!(duration(3900.0), "1 h 05 min");
    }

    #[test]
    fn places_name_the_section_a_marker_starts() {
        let markers = vec![Marker {
            id: "m".into(),
            bar: 9,
            name: "Verse".into(),
        }];
        assert_eq!(place(&markers, Some((9, 17))), "“Verse”, bars 9–16");
        assert_eq!(place(&markers, Some((3, 4))), "Bar 3");
        assert_eq!(place(&markers, Some((1, 5))), "Bars 1–4");
        assert_eq!(place(&markers, None), "Whole song");
    }

    mod pane {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;

        use crate::App;
        use crate::commands::Cmd;
        use crate::lessons::{DAY_MS, Tab, now_ms};
        use crate::timeline::Marker;

        fn harness() -> Harness<'static, App> {
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let app = h.state_mut();
            for (id, bar) in [("intro", 1), ("verse", 9)] {
                app.markers.push(Marker {
                    id: id.into(),
                    bar,
                    name: format!("Section {id}"),
                });
            }
            app.loop_bars = (9, 17);
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, Cmd::Lessons);
            h.run();
            h
        }

        fn session(bars: (i64, i64), bpm: f64, ago_days: i64) -> daw_project::PracticeSession {
            daw_project::PracticeSession {
                id: daw_project::new_id(),
                who: "me".into(),
                started: now_ms() - ago_days * DAY_MS,
                seconds: 600.0,
                bars: Some(bars),
                loops: 20,
                top_speed: bpm / 1.2,
                top_bpm: bpm,
            }
        }

        #[test]
        fn an_assignment_is_set_from_the_loop_and_practised() {
            let mut h = harness();
            h.get_by_label("Set an assignment").click();
            h.run();
            h.get_by_label("Set").click();
            h.run();
            let a = h.state().assignments[0].clone();
            assert_eq!(a.bars, (9, 17));
            assert_eq!(a.bpm, 120.0);
            assert!(a.due > now_ms() + 6 * DAY_MS);
            assert!(
                h.query_by_label("“Section verse”, bars 9–16 at 120 bpm")
                    .is_some()
            );
            assert!(h.query_by_label("Not practised yet.").is_some());

            // It is part of the song, and undoable.
            let ctx = h.ctx.clone();
            h.state_mut().sync_doc(&ctx, true);
            assert_eq!(h.state().proj.doc.get().assignments, vec![a.clone()]);

            h.state_mut().loop_bars = (1, 5);
            h.state_mut().looping = false;
            h.get_by_label("Practise").click();
            h.run();
            let app = h.state();
            assert_eq!(app.loop_bars, (9, 17));
            assert!(app.looping);
            assert!(app.trainer.on);
            assert_eq!(app.trainer.target, 100.0);
        }

        #[test]
        fn progress_comes_from_the_log_and_done_is_each_persons_own() {
            let mut h = harness();
            h.get_by_label("Set an assignment").click();
            h.run();
            h.get_by_label("Set").click();
            h.run();
            // Created a moment ago: count sessions from then on.
            h.state_mut().assignments[0].created = now_ms() - 3 * DAY_MS;
            h.state_mut().assignments[0].bpm = 100.0;
            let doc = &mut h.state_mut().proj.doc;
            doc.log_practice(&session((9, 17), 80.0, 2));
            doc.log_practice(&session((9, 13), 110.0, 1));
            h.run();
            assert!(h.query_by_label("You:").is_some());
            assert!(h.query_by_label("20 min · 40 loops").is_some());

            h.get_by_label("Mark done").click();
            h.run();
            assert_eq!(h.state().proj.doc.done_marks().len(), 1);
            h.get_by_label("Not done yet").click();
            h.run();
            assert!(h.state().proj.doc.done_marks().is_empty());

            h.state_mut().lessons_ui.tab = Tab::Log;
            h.run();
            assert!(h.query_by_label("You: 20 min over 2 sessions").is_some());
            h.get_all_by_label("Forget").next().unwrap().click();
            h.run();
            assert_eq!(h.state().proj.doc.practice_log().len(), 1);
        }

        #[test]
        fn starting_a_lesson_takes_a_snapshot_and_lists_it() {
            let mut h = harness();
            h.state_mut().lessons_ui.tab = Tab::History;
            h.run();
            h.get_by_label("Start a lesson").click();
            h.run();
            let doc = &h.state().proj.doc;
            let lessons = doc.lessons();
            assert_eq!(lessons.len(), 1);
            assert_eq!(lessons[0].title, "Lesson 1");
            assert_eq!(doc.snapshots()[0].id, lessons[0].snapshot);
            assert!(h.query_by_label("No practice logged after it.").is_some());

            // An assignment set now belongs to it.
            h.state_mut().lessons_ui.tab = Tab::Assignments;
            h.run();
            h.get_by_label("Set an assignment").click();
            h.run();
            h.get_by_label("Set").click();
            h.run();
            let id = lessons[0].id.clone();
            assert_eq!(
                h.state().assignments[0].lesson.as_deref(),
                Some(id.as_str())
            );

            h.state_mut().lessons_ui.tab = Tab::History;
            h.run();
            h.get_by_label("What changed since…").click();
            h.run();
            assert!(h.state().snapshot_ui.open);
        }
    }
}
