// SPDX-License-Identifier: GPL-3.0-or-later
//! Song sections (Phase 4, A4). For now, inserting and deleting whole
//! bars across the whole song: every track, tempo change, marker,
//! automation line and note moves together (see
//! [`daw_project::insert_bars`]). The commands act on the loop's bars,
//! and the window on any bars.

use daw_engine::time::{TempoMap, Ticks};
use daw_project::Bars;
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
        self.proj.doc.set(&p);
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
                        ui.add(egui::DragValue::new(&mut w.bar).range(1..=9999))
                            .accessible_name("From bar");
                        ui.end_row();
                        ui.label("Bars");
                        ui.add(egui::DragValue::new(&mut w.count).range(1..=MAX_BARS))
                            .accessible_name("How many bars");
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .button("Insert empty bars")
                            .on_hover_text("Before the first bar; what was there moves later")
                            .clicked()
                        {
                            act = Some(true);
                        }
                        if ui
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
}
