// SPDX-License-Identifier: GPL-3.0-or-later
//! Tempo and time-signature changes (roadmap 2.7): the song can speed up,
//! slow down, glide from one tempo to another, or change time signature
//! at a bar. The metronome, count-in, bar numbers, loop and markers all
//! follow, as they all go through the tempo map built here.
//!
//! The changes are kept by bar and beat, like markers. Changes on the
//! same beat as one before them, or a tempo change on the very start, are
//! left out of the map (the start's tempo is the one in the transport
//! bar), and the window says so.

use daw_engine::time::{Tempo, TempoMap, Ticks, TimeSignature};
use daw_project::{MeterChange, TempoChange};
use eframe::egui::{self, RichText};

use crate::App;

/// Beat units offered for a time signature.
const UNITS: [u32; 4] = [2, 4, 8, 16];

/// The song's time, beyond its starting tempo and beats per bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Changes {
    /// The beat's note value at the start: 4 for quarter notes.
    pub beat_unit: u32,
    pub tempos: Vec<TempoChange>,
    pub meters: Vec<MeterChange>,
}

impl Default for Changes {
    fn default() -> Changes {
        Changes {
            beat_unit: 4,
            tempos: Vec::new(),
            meters: Vec::new(),
        }
    }
}

/// A time signature from what the project holds, made safe: at least one
/// beat, and a beat unit the map takes.
fn signature(beats: u32, unit: u32) -> TimeSignature {
    let unit = if unit.is_power_of_two() && unit <= 32 {
        unit
    } else {
        4
    };
    TimeSignature::new(beats.clamp(1, 64), unit)
}

fn tempo(bpm: f64) -> Tempo {
    Tempo::from_f64(bpm.clamp(10.0, 999.0))
}

/// The tempo map for a song starting at `bpm` with `beats` to the bar.
pub(crate) fn map(rate: u32, bpm: f64, beats: u32, c: &Changes) -> TempoMap {
    let mut m = TempoMap::new(rate, tempo(bpm), signature(beats, c.beat_unit));
    // Time signatures first: they decide where the bars are.
    let mut meters: Vec<&MeterChange> = c.meters.iter().collect();
    meters.sort_by_key(|s| s.bar);
    for s in meters {
        if s.bar > 1 {
            let at = m.bar_start(s.bar);
            let _ = m.add_meter(at, signature(s.beats, s.unit));
        }
    }
    for t in sorted(&c.tempos) {
        let Some(at) = tempo_tick(&m, t) else {
            continue;
        };
        let _ = if t.glide {
            m.add_glide(at, tempo(t.bpm))
        } else {
            m.add_tempo(at, tempo(t.bpm))
        };
    }
    m
}

fn sorted(tempos: &[TempoChange]) -> Vec<&TempoChange> {
    let mut v: Vec<&TempoChange> = tempos.iter().collect();
    v.sort_by_key(|t| (t.bar, t.beat));
    v
}

/// Where a tempo change falls, or `None` at the very start. A beat past
/// the end of its bar is taken as the bar's last.
fn tempo_tick(m: &TempoMap, t: &TempoChange) -> Option<Ticks> {
    let bar = m.bar_start(t.bar.max(1));
    let sig = m.signature_at(bar);
    let beat = (t.beat.max(1) - 1).min(sig.beats - 1) as i64;
    let at = Ticks(bar.0 + beat * sig.beat_ticks());
    (at.0 > 0).then_some(at)
}

/// Why a change is left out of the map, if it is.
fn tempo_problem(m: &TempoMap, all: &[TempoChange], i: usize) -> Option<&'static str> {
    let at = tempo_tick(m, &all[i]);
    if at.is_none() {
        return Some("the start's tempo is set in the transport bar");
    }
    let earlier = all.iter().enumerate().any(|(j, o)| {
        j != i && tempo_tick(m, o) == at && (o.bar, o.beat, j) < (all[i].bar, all[i].beat, i)
    });
    earlier.then_some("another tempo change is on this beat")
}

fn meter_problem(all: &[MeterChange], i: usize) -> Option<&'static str> {
    let bar = all[i].bar;
    if bar <= 1 {
        return Some("the start's time signature is set in the transport bar");
    }
    let earlier = all.iter().take(i).any(|o| o.bar == bar);
    earlier.then_some("another time signature change is on this bar")
}

/// What the timeline asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// A tempo change at this bar and beat.
    Tempo(i64, u32),
    /// A time-signature change at this bar.
    Meter(i64),
    /// Just open the window.
    Open,
}

/// The Tempo and time signature window.
#[derive(Default)]
pub(crate) struct TempoWindow {
    pub(crate) open: bool,
}

impl TempoWindow {
    /// Adds a change as `ask` says and opens the window. `m` is the map as
    /// it is now, `bpm` the tempo in force where the change goes.
    pub(crate) fn ask(&mut self, c: &mut Changes, m: &TempoMap, ask: Ask) {
        self.open = true;
        match ask {
            Ask::Tempo(bar, beat) => {
                let probe = TempoChange {
                    id: daw_project::new_id(),
                    bar: bar.max(1),
                    beat: beat.max(1),
                    bpm: 0.0,
                    glide: false,
                };
                let bpm = tempo_tick(m, &probe).map_or(120.0, |t| m.tempo_at(t).as_f64());
                c.tempos.push(TempoChange { bpm, ..probe });
            }
            Ask::Meter(bar) => {
                let sig = m.signature_at(m.bar_start(bar.max(2)));
                c.meters.push(MeterChange {
                    id: daw_project::new_id(),
                    bar: bar.max(2),
                    beats: sig.beats,
                    unit: sig.unit,
                });
            }
            Ask::Open => {}
        }
    }

    /// The window. `playhead_bar` is where the add buttons put a change.
    /// True if anything changed.
    pub(crate) fn window(
        &mut self,
        ctx: &egui::Context,
        c: &mut Changes,
        m: &TempoMap,
        playhead_bar: i64,
        may_edit: bool,
    ) -> bool {
        if !self.open {
            return false;
        }
        let mut open = self.open;
        let mut changed = false;
        let mut ask = None;
        egui::Window::new("Tempo and time signature")
            .open(&mut open)
            .default_pos([320.0, 160.0])
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(
                    "Changes after the start. The metronome, count-in, bar numbers, loop and \
                     markers all follow them. The start's tempo and time signature are in the \
                     transport bar.",
                );
                ui.add_enabled_ui(may_edit, |ui| {
                    ui.add_space(6.0);
                    ui.heading("Tempo changes");
                    changed |= tempo_rows(ui, c, m);
                    if ui
                        .button(format!("Add tempo change at bar {}", playhead_bar.max(2)))
                        .clicked()
                    {
                        ask = Some(Ask::Tempo(playhead_bar.max(2), 1));
                    }
                    ui.add_space(6.0);
                    ui.heading("Time signature changes");
                    changed |= meter_rows(ui, c);
                    if ui
                        .button(format!(
                            "Add time signature change at bar {}",
                            playhead_bar.max(2)
                        ))
                        .clicked()
                    {
                        ask = Some(Ask::Meter(playhead_bar.max(2)));
                    }
                });
            });
        self.open = open;
        if let Some(a) = ask {
            self.ask(c, m, a);
            changed = true;
        }
        // In song order once nothing is being edited, so rows don't jump
        // about under the pointer.
        if !ctx.egui_wants_keyboard_input() && ctx.dragged_id().is_none() {
            c.tempos.sort_by_key(|t| (t.bar, t.beat));
            c.meters.sort_by_key(|s| s.bar);
        }
        changed
    }
}

fn tempo_rows(ui: &mut egui::Ui, c: &mut Changes, m: &TempoMap) -> bool {
    let mut changed = false;
    let mut remove = None;
    if c.tempos.is_empty() {
        ui.label(RichText::new("None yet.").weak());
    }
    for i in 0..c.tempos.len() {
        let problem = tempo_problem(m, &c.tempos, i);
        let t = &mut c.tempos[i];
        ui.horizontal(|ui| {
            let n = i + 1;
            ui.label("Bar");
            changed |= ui
                .add(egui::DragValue::new(&mut t.bar).range(1..=9999))
                .accessible_name(format!("Tempo change {n} bar"))
                .changed();
            ui.label("beat");
            changed |= ui
                .add(egui::DragValue::new(&mut t.beat).range(1..=64))
                .accessible_name(format!("Tempo change {n} beat"))
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut t.bpm)
                        .range(20.0..=300.0)
                        .speed(0.2)
                        .suffix(" BPM"),
                )
                .accessible_name(format!("Tempo change {n} tempo"))
                .changed();
            changed |= ui
                .checkbox(&mut t.glide, "Glide")
                .on_hover_text("Change smoothly from the tempo before, reaching this one here")
                .changed();
            if ui
                .button("Delete")
                .accessible_name(format!("Delete tempo change {n}"))
                .clicked()
            {
                remove = Some(i);
            }
        });
        if let Some(p) = problem {
            ui.label(RichText::new(format!("Not used: {p}.")).weak().small());
        }
    }
    if let Some(i) = remove {
        c.tempos.remove(i);
        changed = true;
    }
    changed
}

fn meter_rows(ui: &mut egui::Ui, c: &mut Changes) -> bool {
    let mut changed = false;
    let mut remove = None;
    if c.meters.is_empty() {
        ui.label(RichText::new("None yet.").weak());
    }
    for i in 0..c.meters.len() {
        let problem = meter_problem(&c.meters, i);
        let s = &mut c.meters[i];
        ui.horizontal(|ui| {
            let n = i + 1;
            ui.label("Bar");
            changed |= ui
                .add(egui::DragValue::new(&mut s.bar).range(2..=9999))
                .accessible_name(format!("Time signature change {n} bar"))
                .changed();
            changed |= ui
                .add(egui::DragValue::new(&mut s.beats).range(1..=32))
                .accessible_name(format!("Time signature change {n} beats"))
                .changed();
            ui.label("/");
            egui::ComboBox::from_id_salt(("meter unit", &s.id))
                .width(48.0)
                .selected_text(s.unit.to_string())
                .show_ui(ui, |ui| {
                    for u in UNITS {
                        changed |= ui.selectable_value(&mut s.unit, u, u.to_string()).changed();
                    }
                })
                .response
                .accessible_name(format!("Time signature change {n} beat unit"));
            if ui
                .button("Delete")
                .accessible_name(format!("Delete time signature change {n}"))
                .clicked()
            {
                remove = Some(i);
            }
        });
        if let Some(p) = problem {
            ui.label(RichText::new(format!("Not used: {p}.")).weak().small());
        }
    }
    if let Some(i) = remove {
        c.meters.remove(i);
        changed = true;
    }
    changed
}

/// The beat unit picker for the start's time signature, in the transport
/// bar.
pub(crate) fn unit_picker(ui: &mut egui::Ui, unit: &mut u32) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt("beat unit")
        .width(44.0)
        .selected_text(unit.to_string())
        .show_ui(ui, |ui| {
            for u in UNITS {
                changed |= ui.selectable_value(unit, u, u.to_string()).changed();
            }
        })
        .response
        .accessible_name("Beat unit");
    changed
}

impl App {
    pub(crate) fn tempo_window(&mut self, ctx: &egui::Context) {
        let map = self.tempo_map();
        let pos = self.ctl.status().position();
        let bar = map.bar_beat(map.ticks_at(pos)).bar;
        let may_edit = self.may_edit_song();
        if self
            .tempo_ui
            .window(ctx, &mut self.time, &map, bar, may_edit)
        {
            self.time_changed();
        }
    }

    /// The timeline asked for a change, or the window.
    pub(crate) fn ask_tempo(&mut self, ask: Ask) {
        let map = self.tempo_map();
        let before = self.time.clone();
        self.tempo_ui.ask(&mut self.time, &map, ask);
        if self.time != before {
            self.time_changed();
        }
    }

    /// The song's time changed: the engine plays to the new map, and the
    /// loop, which is in bars, moves with it.
    pub(crate) fn time_changed(&mut self) {
        self.rebuild();
        self.send_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::time::{Frames, PPQ};

    fn change(bar: i64, beat: u32, bpm: f64, glide: bool) -> TempoChange {
        TempoChange {
            id: daw_project::new_id(),
            bar,
            beat,
            bpm,
            glide,
        }
    }

    fn meter(bar: i64, beats: u32, unit: u32) -> MeterChange {
        MeterChange {
            id: daw_project::new_id(),
            bar,
            beats,
            unit,
        }
    }

    #[test]
    fn changes_go_on_their_bars_and_beats() {
        let c = Changes {
            beat_unit: 4,
            tempos: vec![change(5, 3, 60.0, false), change(3, 1, 90.0, true)],
            meters: vec![meter(3, 6, 8)],
        };
        let m = map(48_000, 120.0, 4, &c);
        // Bars 1 and 2 are 4/4, then 6/8 bars of 2,880 ticks.
        let bar3 = m.bar_start(3);
        assert_eq!(bar3, Ticks(8 * PPQ));
        assert_eq!(m.bar_start(5), Ticks(bar3.0 + 2 * 2880));
        let changes: Vec<_> = m.tempo_changes().collect();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0], (bar3, Tempo::bpm(90), true));
        // Beat 3 of a 6/8 bar: two eighth notes in.
        assert_eq!(changes[1].0, Ticks(m.bar_start(5).0 + 2 * 480));
        assert_eq!(m.signature_at(m.bar_start(9)), TimeSignature::new(6, 8));
        // The glide ends at bar 3, the jump to 60 comes later.
        assert_eq!(m.tempo_at(m.bar_start(4)), Tempo::bpm(90));
        assert_eq!(m.tempo_at(Ticks(changes[1].0.0 + 1)), Tempo::bpm(60));
    }

    #[test]
    fn clashes_and_the_start_are_left_out() {
        let c = Changes {
            beat_unit: 8,
            tempos: vec![
                change(1, 1, 200.0, false),
                change(4, 1, 100.0, false),
                change(4, 1, 140.0, false),
                // Past the end of a 3-beat bar: its last beat.
                change(6, 9, 80.0, false),
            ],
            meters: vec![meter(1, 5, 4), meter(2, 3, 4), meter(2, 7, 8)],
        };
        let m = map(48_000, 120.0, 3, &c);
        assert_eq!(m.signature_at(Ticks(0)), TimeSignature::new(3, 8));
        assert_eq!(m.signature_at(m.bar_start(2)), TimeSignature::new(3, 4));
        let at: Vec<_> = m.tempo_changes().map(|(t, b, _)| (t, b)).collect();
        assert_eq!(
            at,
            vec![
                (m.bar_start(4), Tempo::bpm(100)),
                (Ticks(m.bar_start(6).0 + 2 * PPQ), Tempo::bpm(80)),
            ]
        );
        assert!(tempo_problem(&m, &c.tempos, 0).is_some());
        assert!(tempo_problem(&m, &c.tempos, 1).is_none());
        assert!(tempo_problem(&m, &c.tempos, 2).is_some());
        assert!(meter_problem(&c.meters, 0).is_some());
        assert!(meter_problem(&c.meters, 1).is_none());
        assert!(meter_problem(&c.meters, 2).is_some());
    }

    #[test]
    fn a_new_change_starts_at_the_tempo_already_there() {
        let mut c = Changes {
            tempos: vec![change(3, 1, 90.0, false)],
            ..Changes::default()
        };
        let m = map(48_000, 120.0, 4, &c);
        let mut w = TempoWindow::default();
        w.ask(&mut c, &m, Ask::Tempo(7, 2));
        w.ask(&mut c, &m, Ask::Meter(1));
        assert!(w.open);
        assert_eq!(c.tempos[1].bpm, 90.0);
        assert_eq!((c.tempos[1].bar, c.tempos[1].beat), (7, 2));
        assert_eq!(
            (c.meters[0].bar, c.meters[0].beats, c.meters[0].unit),
            (2, 4, 4)
        );
        // Nothing moves until the change is edited.
        let after = map(48_000, 120.0, 4, &c);
        assert_eq!(
            after.frames_at(after.bar_start(20)),
            m.frames_at(m.bar_start(20))
        );
        assert_eq!(after.frames_at(Ticks(0)), Frames(0));
    }

    fn app() -> egui_kittest::Harness<'static, App> {
        let mut h = egui_kittest::Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    #[test]
    fn changes_made_in_the_window_move_the_bars_and_are_saved() {
        use egui_kittest::kittest::Queryable;
        let mut h = app();
        h.get_by_label("Changes…").click();
        h.run();
        h.get_by_label("Add tempo change at bar 2").click();
        h.run();
        h.get_by_label("Add time signature change at bar 2").click();
        h.run();
        let before = h
            .state()
            .tempo_map()
            .frames_at(h.state().tempo_map().bar_start(4));
        {
            let app = h.state_mut();
            app.time.tempos[0].bpm = 60.0;
            app.time.meters[0].beats = 3;
        }
        h.get_by_label("Glide").click();
        h.run();
        let app = h.state();
        let m = app.tempo_map();
        // Bar 2 onwards is 3/4, and the tempo glides from 120 to 60
        // over bar 1, so bar 4 comes later than it did.
        assert_eq!(m.signature_at(m.bar_start(3)), TimeSignature::new(3, 4));
        assert_eq!(m.tempo_at(m.bar_start(2)), Tempo::bpm(60));
        assert!(m.frames_at(m.bar_start(4)) > before);
        // In the document, so it saves, merges and undoes.
        let p = app.proj.doc.get();
        assert_eq!(p.tempo_changes.len(), 1);
        assert!(p.tempo_changes[0].glide);
        assert_eq!(p.tempo_changes[0].bpm, 60.0);
        assert_eq!(p.meter_changes[0].beats, 3);

        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        assert!(!h.state().time.tempos[0].glide, "undo takes the glide back");
    }

    #[test]
    fn a_deleted_change_is_gone() {
        use egui_kittest::kittest::Queryable;
        let mut h = app();
        let m = h.state().tempo_map();
        h.state_mut().ask_tempo(Ask::Tempo(5, 1));
        h.state_mut().ask_tempo(Ask::Meter(3));
        h.state_mut().time.tempos[0].bpm = 90.0;
        h.run();
        assert_eq!(h.state().proj.doc.get().tempo_changes.len(), 1);
        h.get_by_label("Delete tempo change 1").click();
        h.run();
        h.get_by_label("Delete time signature change 1").click();
        h.run();
        let app = h.state();
        assert!(app.proj.doc.get().tempo_changes.is_empty());
        assert!(app.time.meters.is_empty());
        let now = app.tempo_map();
        assert_eq!(now.frames_at(now.bar_start(9)), m.frames_at(m.bar_start(9)));
    }
}
