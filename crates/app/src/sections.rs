// SPDX-License-Identifier: GPL-3.0-or-later
//! Song sections (Phase 4, A4): the bars from one marker to the next,
//! the last running to the end of the song. A section can be looped to
//! practise it, copied in after itself, deleted or moved, and the
//! sections can be put in a play order, with repeats, which plays as it
//! stands (the engine goes from one section's end to the next's start,
//! to the sample) and can be written into the song as a plain timeline.
//!
//! Also inserting and deleting whole bars across the whole song: every
//! track, tempo change, marker, automation line and note moves together
//! (see [`daw_project::insert_bars`]). Those commands act on the loop's
//! bars, and the window on any bars.
//!
//! Every action is a command acting on the section under the playhead,
//! so it works from outside the interface too.

use daw_engine::engine::Command;
use daw_engine::time::{Frames, TempoMap, Ticks};
use daw_project::{Bars, Section};
use eframe::egui::{self, RichText};

use crate::App;

/// The insert-or-delete-bars window.
#[derive(Debug)]
pub(crate) struct TimeWindow {
    pub open: bool,
    /// The first bar, counted from 1, and how many.
    pub bar: i64,
    pub count: i64,
}

impl Default for TimeWindow {
    fn default() -> TimeWindow {
        TimeWindow {
            open: false,
            bar: 1,
            count: 1,
        }
    }
}

/// Where `count` bars from `bar` fall now, for deleting them.
pub(crate) fn existing_bars(map: &TempoMap, bar: i64, count: i64) -> Bars {
    let bar = bar.max(1);
    let (t0, t1) = (map.bar_start(bar), map.bar_start(bar + count));
    let (f0, f1) = (map.frames_at(t0), map.frames_at(t1));
    Bars {
        bar,
        count,
        tick: t0.0,
        ticks: t1.0 - t0.0,
        frame: f0.0,
        frames: f1.0 - f0.0,
    }
}

/// Where `count` new bars before `bar` would fall: they take the time
/// signature and tempo in force just before it.
pub(crate) fn new_bars(map: &TempoMap, bar: i64, count: i64) -> Bars {
    let bar = bar.max(1);
    let t0 = map.bar_start(bar);
    let just_before = Ticks((t0.0 - 1).max(0));
    let sig = map.signature_at(just_before);
    let ticks = sig.bar_ticks() * count;
    let tempo = map.tempo_at(just_before);
    let frames = TempoMap::new(map.sample_rate(), tempo, sig)
        .frames_at(Ticks(ticks))
        .0;
    Bars {
        bar,
        count,
        tick: t0.0,
        ticks,
        frame: map.frames_at(t0).0,
        frames,
    }
}

/// The most bars inserted or deleted at once.
const MAX_BARS: i64 = 999;

impl App {
    /// Puts `count` empty bars before bar `bar`, across the whole song.
    /// False if nothing changed.
    pub(crate) fn insert_bars(&mut self, bar: i64, count: i64) -> bool {
        if !self.may_edit_song() || count < 1 {
            return false;
        }
        let b = new_bars(&self.tempo_map(), bar, count.min(MAX_BARS));
        let mut p = self.project();
        daw_project::insert_bars(&mut p, &b);
        self.commit(p);
        true
    }

    /// Takes out bars `bar` to `bar + count`, across the whole song.
    /// False if nothing changed.
    pub(crate) fn delete_bars(&mut self, bar: i64, count: i64) -> bool {
        if !self.may_edit_song() || count < 1 {
            return false;
        }
        let b = existing_bars(&self.tempo_map(), bar, count.min(MAX_BARS));
        let mut p = self.project();
        daw_project::delete_bars(&mut p, &b);
        self.commit(p);
        true
    }

    /// Writes an edit made to the whole project, as one undo step, and
    /// takes it up.
    fn commit(&mut self, p: daw_project::Project) {
        self.set_song(&p, None);
        self.apply(p);
        self.proj.dirty = true;
    }

    /// The loop's bars: the first, and how many.
    fn loop_span(&self) -> (i64, i64) {
        let (a, z) = self.loop_bars;
        (a.max(1), (z - a).max(1))
    }

    /// Inserts as many empty bars as the loop is long, before it.
    pub(crate) fn insert_loop_bars(&mut self) {
        let (bar, n) = self.loop_span();
        if self.insert_bars(bar, n) {
            self.message = Some(format!("Put in {} before bar {bar}.", bars_text(n)));
        }
    }

    /// Deletes the loop's bars.
    pub(crate) fn delete_loop_bars(&mut self) {
        let (bar, n) = self.loop_span();
        if self.delete_bars(bar, n) {
            self.message = Some(format!("Took out {} from bar {bar}.", bars_text(n)));
        }
    }

    pub(crate) fn open_time_window(&mut self) {
        let (bar, count) = self.loop_span();
        self.time_ui = TimeWindow {
            open: true,
            bar,
            count,
        };
    }

    pub(crate) fn time_window(&mut self, ctx: &egui::Context) {
        if !self.time_ui.open {
            return;
        }
        let may_edit = self.may_edit_song();
        let mut open = true;
        let mut act = None;
        let w = &mut self.time_ui;
        egui::Window::new("Insert or delete bars")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Every track, note, automation line, tempo change and marker \
                     moves with the bars.",
                );
                if !may_edit {
                    ui.label(
                        RichText::new("You can comment on this project, not change it.").weak(),
                    );
                }
                ui.add_enabled_ui(may_edit, |ui| {
                    egui::Grid::new("time_grid").show(ui, |ui| {
                        ui.label("From bar");
                        // ai: set_loop.from_bar
                        ui.add(egui::DragValue::new(&mut w.bar).range(1..=9999))
                            .accessible_name("From bar");
                        ui.end_row();
                        ui.label("Bars");
                        // ai: set_loop.to_bar
                        ui.add(egui::DragValue::new(&mut w.count).range(1..=MAX_BARS))
                            .accessible_name("How many bars");
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui
                            // ai: insert-bars
                            .button("Insert empty bars")
                            .on_hover_text("Before the first bar; what was there moves later")
                            .clicked()
                        {
                            act = Some(true);
                        }
                        if ui
                            // ai: delete-bars
                            .button("Delete bars")
                            .on_hover_text("What is in them goes; what comes after moves up")
                            .clicked()
                        {
                            act = Some(false);
                        }
                    });
                });
            });
        let (bar, count) = (self.time_ui.bar, self.time_ui.count);
        match act {
            Some(true) => {
                self.insert_bars(bar, count);
            }
            Some(false) => {
                self.delete_bars(bar, count);
            }
            None => {}
        }
        self.time_ui.open = open && act.is_none();
    }
}

/// The sections window.
#[derive(Debug, Default)]
pub(crate) struct SectionsWindow {
    pub open: bool,
}

/// The play order playing: each section's frames, and which plays now.
#[derive(Debug)]
pub(crate) struct Arranging {
    spans: Vec<(i64, i64)>,
    at: usize,
    /// The engine's loop count when `at` began.
    loops: u64,
}

/// The bar the song ends at: the first with nothing in it after the
/// last clip, note or marker.
pub(crate) fn song_end(map: &TempoMap, p: &daw_project::Project) -> i64 {
    let mut ticks = 0;
    for t in &p.tracks {
        for c in &t.clips {
            ticks = ticks.max(map.ticks_at(Frames(c.start + c.len)).0);
        }
        for c in &t.note_clips {
            ticks = ticks.max(c.end());
        }
    }
    let bb = map.bar_beat(Ticks(ticks));
    let mut end = if map.bar_start(bb.bar) == Ticks(ticks) {
        bb.bar
    } else {
        bb.bar + 1
    };
    for m in &p.markers {
        end = end.max(m.bar + 1);
    }
    end.max(2)
}

impl App {
    /// Where each bar starts, in ticks and frames, as the song is now.
    fn bar_clock(&self) -> impl Fn(i64) -> (i64, i64) + use<> {
        let map = self.tempo_map();
        move |bar| {
            let t = map.bar_start(bar);
            (t.0, map.frames_at(t).0)
        }
    }

    fn song_end(&self) -> i64 {
        song_end(&self.tempo_map(), &self.project())
    }

    pub(crate) fn sections(&self) -> Vec<Section> {
        daw_project::sections(&self.project(), self.song_end())
    }

    /// The section under the playhead, if it is in one.
    pub(crate) fn section_here(&self) -> Option<Section> {
        let map = self.tempo_map();
        let at = map.ticks_at(self.ctl.status().position());
        let bar = map.bar_beat(at).bar;
        (self.sections().into_iter()).find(|s| s.from <= bar && bar < s.to)
    }

    /// Runs `f` on the section under the playhead, or says there is none.
    fn on_section_here(&mut self, f: impl FnOnce(&mut App, &Section)) {
        match self.section_here() {
            Some(s) => f(self, &s),
            None => {
                self.message = Some(
                    "The playhead isn't in a section: mark one with M (Mark a section).".into(),
                )
            }
        }
    }

    /// Loops section `s`, to practise it.
    pub(crate) fn loop_section(&mut self, s: &Section) {
        self.stop_arranging();
        self.loop_bars = (s.from, s.to);
        self.looping = true;
        self.send_loop();
    }

    /// Puts a copy of section `s` straight after it.
    pub(crate) fn duplicate_section(&mut self, s: &Section) {
        if !self.may_edit_song() {
            return;
        }
        let clock = self.bar_clock();
        let mut p = self.project();
        let piece = daw_project::piece(&p, &clock, s.from, s.to, self.song_end(), false);
        let len = daw_project::bars_between(&clock, s.from, s.to);
        let (tick, frame) = clock(s.to);
        let at = Bars {
            bar: s.to,
            tick,
            frame,
            ..len
        };
        daw_project::paste_bars(&mut p, &piece, &at, true);
        self.commit(p);
        self.message = Some(format!("Copied “{}” in after itself.", s.name));
    }

    /// Takes section `s` out of the song, closing the gap.
    pub(crate) fn delete_section(&mut self, s: &Section) {
        if self.delete_bars(s.from, s.to - s.from) {
            self.message = Some(format!("Took out “{}”.", s.name));
        }
    }

    /// Swaps section `s` with the one before it (or after it).
    pub(crate) fn move_section(&mut self, s: &Section, earlier: bool) {
        if !self.may_edit_song() {
            return;
        }
        let all = self.sections();
        let Some(i) = all.iter().position(|x| x.id == s.id) else {
            return;
        };
        let j = if earlier {
            i.checked_sub(1)
        } else {
            Some(i + 1)
        };
        let Some(j) = j.filter(|&j| j < all.len()) else {
            return;
        };
        let mut order: Vec<String> = all.iter().map(|x| x.id.clone()).collect();
        order.swap(i, j);
        let mut p =
            daw_project::flatten(&self.project(), &self.bar_clock(), &order, self.song_end());
        p.arrangement = self.arrangement.clone();
        self.commit(p);
    }

    /// Adds section `s` to the end of the play order.
    pub(crate) fn add_to_order(&mut self, s: &Section) {
        self.arrangement.push(s.id.clone());
    }

    /// The play order, leaving out sections since deleted.
    fn order(&self) -> Vec<Section> {
        let all = self.sections();
        (self.arrangement.iter())
            .filter_map(|id| all.iter().find(|s| &s.id == id).cloned())
            .collect()
    }

    /// Plays the sections in the play order, round and round, until
    /// stopped.
    pub(crate) fn play_order(&mut self) {
        let map = self.tempo_map();
        let f = |bar| map.frames_at(map.bar_start(bar)).0;
        let spans: Vec<(i64, i64)> = self.order().iter().map(|s| (f(s.from), f(s.to))).collect();
        let Some(&first) = spans.first() else {
            self.message = Some("Put some sections in the play order first.".into());
            return;
        };
        let next = spans[1 % spans.len()];
        self.send(Command::Stop);
        self.send(Command::SetLoop(Some((Frames(first.0), Frames(first.1)))));
        self.send(Command::QueueLoop(Some((Frames(next.0), Frames(next.1)))));
        self.send(Command::Seek(Frames(first.0)));
        self.send(Command::Play);
        self.arranging = Some(Arranging {
            spans,
            at: 0,
            loops: self.ctl.status().loops(),
        });
    }

    /// Back to the song's own loop.
    fn stop_arranging(&mut self) {
        if self.arranging.take().is_some() {
            self.send_loop();
        }
    }

    /// Keeps the next section queued while the play order plays.
    pub(crate) fn arrangement_tick(&mut self) {
        let Some(a) = &mut self.arranging else {
            return;
        };
        let st = self.ctl.status();
        if !st.playing() && !st.counting_in() {
            // Stopped, but give the play command a moment to arrive.
            if st.loops() == a.loops && a.at == 0 && st.position().0 != a.spans[0].0 {
                return;
            }
            self.stop_arranging();
            return;
        }
        let gone = st.loops().saturating_sub(a.loops);
        if gone == 0 {
            return;
        }
        a.loops = st.loops();
        a.at = (a.at + gone as usize) % a.spans.len();
        let next = a.spans[(a.at + 1) % a.spans.len()];
        self.send(Command::QueueLoop(Some((Frames(next.0), Frames(next.1)))));
    }

    /// Which section of the play order is playing, if it is.
    pub(crate) fn arranging_at(&self) -> Option<usize> {
        self.arranging.as_ref().map(|a| a.at)
    }

    /// Writes the play order into the song as a plain timeline.
    pub(crate) fn write_order(&mut self) {
        if !self.may_edit_song() {
            return;
        }
        if self.order().is_empty() {
            self.message = Some("Put some sections in the play order first.".into());
            return;
        }
        self.stop_arranging();
        let p = daw_project::flatten(
            &self.project(),
            &self.bar_clock(),
            &self.arrangement,
            self.song_end(),
        );
        self.commit(p);
        self.message = Some("The song now plays in the play order.".into());
    }

    pub(crate) fn sections_window(&mut self, ctx: &egui::Context) {
        if !self.sections_ui.open {
            return;
        }
        let may_edit = self.may_edit_song();
        let all = self.sections();
        let here = self.section_here().map(|s| s.id);
        let playing = self.arranging_at();
        let mut open = true;
        let mut act: Option<(Section, &str)> = None;
        let mut order = self.arrangement.clone();
        let (mut play, mut write) = (false, false);
        egui::Window::new("Sections")
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                if all.is_empty() {
                    ui.label(
                        "A section runs from one marker to the next. Mark one at the \
                         playhead's bar with M (Song → Mark a section).",
                    );
                    return;
                }
                if !may_edit {
                    ui.label(
                        RichText::new("You can comment on this project, not change it.").weak(),
                    );
                }
                egui::Grid::new("sections_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        for (i, s) in all.iter().enumerate() {
                            let name = if here.as_ref() == Some(&s.id) {
                                RichText::new(format!("{} (here)", s.name)).strong()
                            } else {
                                RichText::new(&s.name)
                            };
                            ui.label(name);
                            ui.label(format!("bars {}–{}", s.from, s.to - 1));
                            if ui
                                // ai: loop-section
                                .button("Loop")
                                .on_hover_text("Loop it, to practise")
                                .clicked()
                            {
                                act = Some((s.clone(), "loop"));
                            }
                            if ui
                                // ai: add-to-play-order
                                .button("+ Play order")
                                .on_hover_text("Add it to the end of the play order")
                                .clicked()
                            {
                                order.push(s.id.clone());
                            }
                            ui.add_enabled_ui(may_edit, |ui| {
                                ui.horizontal(|ui| {
                                    if ui
                                        // ai: section-earlier
                                        .add_enabled(i > 0, egui::Button::new("Earlier"))
                                        .accessible_name(format!("Move {} earlier", s.name))
                                        .clicked()
                                    {
                                        act = Some((s.clone(), "earlier"));
                                    }
                                    if ui
                                        // ai: section-later
                                        .add_enabled(i + 1 < all.len(), egui::Button::new("Later"))
                                        .accessible_name(format!("Move {} later", s.name))
                                        .clicked()
                                    {
                                        act = Some((s.clone(), "later"));
                                    }
                                    if ui
                                        // ai: duplicate-section
                                        .button("Copy after")
                                        .on_hover_text("Put a copy of it straight after it")
                                        .clicked()
                                    {
                                        act = Some((s.clone(), "copy"));
                                    }
                                    if ui
                                        // ai: delete-section
                                        .button("Delete")
                                        .on_hover_text("Take its bars out of every track")
                                        .clicked()
                                    {
                                        act = Some((s.clone(), "delete"));
                                    }
                                });
                            });
                            ui.end_row();
                        }
                    });
                ui.separator();
                ui.label(RichText::new("Play order").strong());
                if order.is_empty() {
                    ui.label(
                        RichText::new(
                            "Add sections in any order, as often as you like, to try a \
                             new arrangement before writing it in.",
                        )
                        .weak(),
                    );
                }
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    for (i, id) in order.iter().enumerate() {
                        let Some(s) = all.iter().find(|s| &s.id == id) else {
                            continue;
                        };
                        let mut text = RichText::new(format!("{} ×", s.name));
                        if playing == Some(i) {
                            text = text.strong().underline();
                        }
                        // ai: edit_markers.play_order
                        if ui
                            .button(text)
                            .on_hover_text("Take it out of the play order")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    }
                });
                if let Some(i) = remove {
                    order.remove(i);
                }
                ui.horizontal(|ui| {
                    let some = !order.is_empty();
                    if ui
                        // ai: play-order
                        .add_enabled(some, egui::Button::new("Play in this order"))
                        .on_hover_text("Plays the sections in this order, round and round")
                        .clicked()
                    {
                        play = true;
                    }
                    ui.add_enabled_ui(may_edit && some, |ui| {
                        if ui
                            // ai: write-play-order
                            .button("Write into the song")
                            .on_hover_text(
                                "Lays the song out in this order, so it plays this way \
                                 from start to end",
                            )
                            .clicked()
                        {
                            write = true;
                        }
                    });
                    // ai: clear-play-order
                    if ui.add_enabled(some, egui::Button::new("Clear")).clicked() {
                        order.clear();
                    }
                });
            });
        self.sections_ui.open = open;
        if may_edit {
            self.arrangement = order;
        }
        if let Some((s, what)) = act {
            match what {
                "loop" => self.loop_section(&s),
                "earlier" => self.move_section(&s, true),
                "later" => self.move_section(&s, false),
                "copy" => self.duplicate_section(&s),
                _ => self.delete_section(&s),
            }
        }
        if play {
            self.play_order();
        }
        if write {
            self.write_order();
        }
    }

    /// Runs a section command on the section under the playhead.
    pub(crate) fn section_command(&mut self, cmd: crate::commands::Cmd) {
        use crate::commands::Cmd;
        match cmd {
            Cmd::Sections => self.sections_ui.open = true,
            Cmd::LoopSection => self.on_section_here(|a, s| a.loop_section(s)),
            Cmd::DuplicateSection => self.on_section_here(|a, s| a.duplicate_section(s)),
            Cmd::DeleteSection => self.on_section_here(|a, s| a.delete_section(s)),
            Cmd::SectionEarlier => self.on_section_here(|a, s| a.move_section(s, true)),
            Cmd::SectionLater => self.on_section_here(|a, s| a.move_section(s, false)),
            Cmd::AddToPlayOrder => self.on_section_here(|a, s| {
                a.add_to_order(s);
                a.message = Some(format!("Added “{}” to the play order.", s.name));
            }),
            Cmd::ClearPlayOrder => self.arrangement.clear(),
            Cmd::PlayOrder => self.play_order(),
            Cmd::WriteOrder => self.write_order(),
            Cmd::ExportSections => {
                let along = self.export.play_along();
                self.export_sections(None, along, None);
            }
            Cmd::ExportThisSection => self.on_section_here(|a, s| {
                let along = a.export.play_along();
                a.export_sections(Some(std::slice::from_ref(&s.id)), along, None);
            }),
            Cmd::ClearExportQueue => self.export.clear_queue(),
            _ => {}
        }
    }

    /// Exports a practice file for each section, or for those whose
    /// marker id or name is in `only`: now, or after the exports already
    /// going. `play_along` leaves out the tracks muted for practice.
    /// `format` is the window's file type unless given. Answers with
    /// what was queued, or says why not.
    pub(crate) fn export_sections(
        &mut self,
        only: Option<&[String]>,
        play_along: bool,
        format: Option<daw_engine::export::Format>,
    ) -> Option<String> {
        let spans: Vec<crate::export::Span> = (self.sections().into_iter())
            .filter(|s| only.is_none_or(|o| o.iter().any(|k| *k == s.id || *k == s.name)))
            .map(|s| crate::export::Span {
                name: s.name,
                from: s.from,
                to: s.to,
            })
            .collect();
        if spans.is_empty() {
            self.message = Some(if self.markers.is_empty() {
                "There are no sections to export: mark one with M (Mark a section).".into()
            } else {
                "None of those sections are in the song.".into()
            });
            return None;
        }
        let mut req = self.export.sections_request(spans, play_along);
        if let Some(f) = format {
            req.format = f;
        }
        let label = req.label();
        match self.export_or_queue(req) {
            Ok(()) => {
                self.export.open = true;
                Some(label)
            }
            Err(e) => {
                self.message = Some(e);
                None
            }
        }
    }
}

fn bars_text(n: i64) -> String {
    if n == 1 {
        "1 bar".into()
    } else {
        format!("{n} bars")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::time::{Tempo, TimeSignature};

    #[test]
    fn new_bars_take_the_time_before_them() {
        let mut map = TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON);
        let at = map.bar_start(3);
        map.add_meter(at, TimeSignature::new(3, 4)).unwrap();
        map.add_tempo(at, Tempo::bpm(60)).unwrap();
        let b = new_bars(&map, 3, 2);
        // Two bars of 4/4 at 120: 8 beats, 4 s.
        assert_eq!((b.tick, b.ticks), (2 * 3840, 2 * 3840));
        assert_eq!((b.frame, b.frames), (4 * 48_000, 4 * 48_000));
        // Deleting bars 3–4 takes two bars of 3/4 at 60: 6 s.
        let d = existing_bars(&map, 3, 2);
        assert_eq!((d.ticks, d.frames), (2 * 2880, 6 * 48_000));
    }

    fn harness() -> egui_kittest::Harness<'static, App> {
        let mut h = egui_kittest::Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    #[test]
    fn the_loop_commands_insert_and_delete_its_bars_in_one_undo_step() {
        use crate::commands::Cmd;
        let mut h = harness();
        let ctx = h.ctx.clone();
        let a = h.state_mut();
        a.markers.push(crate::timeline::Marker {
            id: daw_project::new_id(),
            bar: 5,
            name: "Chorus".into(),
        });
        a.loop_bars = (2, 4);
        a.sync_doc(&ctx, true);
        a.run(&ctx, Cmd::InsertBars);
        a.sync_doc(&ctx, true);
        let p = a.proj.doc.get();
        assert_eq!(p.markers[0].bar, 7);
        assert_eq!(p.loop_bars, (4, 6));
        a.loop_bars = (2, 4);
        a.run(&ctx, Cmd::DeleteBars);
        a.sync_doc(&ctx, true);
        assert_eq!(a.proj.doc.get().markers[0].bar, 5);
        a.undo(&ctx);
        assert_eq!(a.markers[0].bar, 7);
    }

    #[test]
    fn sections_loop_copy_move_and_play_in_order_through_the_commands() {
        use crate::agent::driver::Driver;
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        for bar in [1, 3, 5] {
            d.call("seek", json!({"bar": bar})).unwrap();
            d.command("marker");
        }
        let bars = |d: &mut Driver| -> Vec<i64> {
            (d.project()["markers"].as_array().unwrap().iter())
                .map(|m| m["bar"].as_i64().unwrap())
                .collect()
        };
        let first = d.app().markers[0].id.clone();
        d.call("seek", json!({"bar": 3.5})).unwrap();
        d.command("loop-section");
        assert_eq!(d.app().loop_bars, (3, 5));
        d.command("duplicate-section");
        assert_eq!(bars(&mut d), [1, 3, 5, 7]);
        d.command("undo");
        assert_eq!(bars(&mut d), [1, 3, 5]);
        // The first section swaps with the second.
        d.call("seek", json!({"bar": 1})).unwrap();
        d.command("section-later");
        let m = &d.app().markers;
        assert_eq!(m.iter().position(|m| m.id == first), Some(1));
        assert_eq!(m[1].bar, 3);
        // A play order with a repeat, played, then written in.
        d.call("seek", json!({"bar": 1})).unwrap();
        d.command("add-to-play-order");
        d.command("add-to-play-order");
        d.call("seek", json!({"bar": 5})).unwrap();
        d.command("add-to-play-order");
        assert_eq!(d.app().arrangement.len(), 3);
        d.command("play-order");
        assert_eq!(d.app().arranging_at(), Some(0));
        d.command("write-play-order");
        assert_eq!(bars(&mut d), [1, 3, 5]);
        assert!(d.app().arrangement.is_empty());
        let names: Vec<String> = d.app().markers.iter().map(|m| m.name.clone()).collect();
        assert_eq!(names[0], names[1]);
    }

    #[test]
    fn an_assistant_sets_the_play_order() {
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        d.call(
            "edit_markers",
            json!({"add": [{"bar": 1, "name": "Verse"}, {"bar": 5, "name": "Chorus"}]}),
        )
        .unwrap();
        let ids: Vec<String> = (d.project()["markers"].as_array().unwrap().iter())
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect();
        let order = json!([ids[0], ids[1], ids[1], ids[0]]);
        d.call("edit_markers", json!({"play_order": order}))
            .unwrap();
        assert_eq!(d.project()["play_order"], order);
        assert_eq!(d.app().arrangement.len(), 4);
        assert!(
            d.call("edit_markers", json!({"play_order": ["nothing"]}))
                .is_err()
        );
        // Taking one out of the middle.
        d.call("edit_markers", json!({"play_order": [ids[0], ids[0]]}))
            .unwrap();
        assert_eq!(d.project()["play_order"], json!([ids[0], ids[0]]));
    }
}
