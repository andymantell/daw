// SPDX-License-Identifier: GPL-3.0-or-later
//! The note editor (roadmap 3.1): one note clip's notes on a grid of
//! pitch against time, with a keyboard down the side and each note's
//! velocity in a lane below.
//!
//! Click an empty place to draw a note (as long as the last one drawn),
//! drag a note to move it (its pitch and its time, snapped to the
//! grid), drag its right edge to make it longer or shorter, drag in
//! the velocity lane to set how hard it is played, and drag across
//! empty space to select several. Delete removes the selection; a
//! right click removes one note. Quantise moves the selected notes (or
//! all of them) towards the grid by the strength given, with swing
//! delaying every second grid line; Transpose moves them in pitch.
//!
//! The lane below the grid shows the notes' velocities, or their chance
//! of playing, or a line: one of the clip's controllers (the mod wheel,
//! the pedal, the pitch wheel…), or the expression of the one note
//! selected (its pitch, pressure or timbre as it sounds). Click a line
//! to add a point, drag a point to move it, right-click it to remove
//! it. With a key set, rows off its scale are shaded; in scale mode
//! drawn and moved notes stay on the scale.

use std::collections::HashSet;

use daw_engine::time::{Frames, TempoMap, Ticks};
use daw_project::{Controller, Lane, Note, NoteClip, PPQ, Scale};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::theme;

/// The grid the editor snaps to and draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grid {
    Bar,
    Beat,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthTriplet,
}

impl Grid {
    pub const ALL: [Grid; 7] = [
        Grid::Bar,
        Grid::Beat,
        Grid::Eighth,
        Grid::Sixteenth,
        Grid::ThirtySecond,
        Grid::EighthTriplet,
        Grid::SixteenthTriplet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Grid::Bar => "Bar",
            Grid::Beat => "Beat",
            Grid::Eighth => "1/8",
            Grid::Sixteenth => "1/16",
            Grid::ThirtySecond => "1/32",
            Grid::EighthTriplet => "1/8 triplet",
            Grid::SixteenthTriplet => "1/16 triplet",
        }
    }

    /// The grid's step in ticks, at the time signature in force at
    /// `at` (a bar's length depends on it).
    pub(crate) fn ticks(self, map: &TempoMap, at: i64) -> i64 {
        let sig = map.signature_at(Ticks(at));
        match self {
            Grid::Bar => sig.bar_ticks(),
            Grid::Beat => sig.beat_ticks(),
            Grid::Eighth => PPQ / 2,
            Grid::Sixteenth => PPQ / 4,
            Grid::ThirtySecond => PPQ / 8,
            Grid::EighthTriplet => PPQ / 3,
            Grid::SixteenthTriplet => PPQ / 6,
        }
    }
}

/// The nearest grid line to `t` (ticks on the timeline), with the
/// lines counted from the start of the bar `t` is in.
fn snap(map: &TempoMap, grid: Grid, t: i64) -> i64 {
    let bar = map.bar_start(map.bar_beat(Ticks(t)).bar).0;
    let step = grid.ticks(map, t).max(1);
    bar + ((t - bar) as f64 / step as f64).round() as i64 * step
}

/// Where the grid line before or at `t` is.
fn floor_to(map: &TempoMap, grid: Grid, t: i64) -> i64 {
    let bar = map.bar_start(map.bar_beat(Ticks(t)).bar).0;
    let step = grid.ticks(map, t).max(1);
    bar + (t - bar).div_euclid(step) * step
}

/// Moves `t` towards its grid line by `strength` (0 to 1). With
/// `swing` (0 to 1), every second grid line is later by that much of a
/// step, as in a shuffle.
pub fn quantise_tick(map: &TempoMap, grid: Grid, t: i64, strength: f64, swing: f64) -> i64 {
    let bar = map.bar_start(map.bar_beat(Ticks(t)).bar).0;
    let step = grid.ticks(map, t).max(1) as f64;
    let rel = (t - bar) as f64;
    // Grid lines, with the odd ones pushed later by the swing.
    let line = |k: i64| bar as f64 + k as f64 * step + if k % 2 != 0 { swing * step } else { 0.0 };
    let k = (rel / step).floor() as i64;
    let candidates = [line(k - 1), line(k), line(k + 1), line(k + 2)];
    let target = candidates
        .iter()
        .copied()
        .min_by(|a, b| (a - t as f64).abs().total_cmp(&(b - t as f64).abs()))
        .unwrap_or(t as f64);
    (t as f64 + (target - t as f64) * strength.clamp(0.0, 1.0)).round() as i64
}

/// What the lane below the grid shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneView {
    Velocity,
    Chance,
    /// One of the clip's controller lanes.
    Controller(Controller),
    /// One kind of the selected note's expression ([`Note::PITCH`] and
    /// the others).
    Expression(&'static str),
}

impl LaneView {
    pub fn label(self) -> String {
        match self {
            LaneView::Velocity => "Velocity".into(),
            LaneView::Chance => "Chance".into(),
            LaneView::Controller(c) => c.label(),
            LaneView::Expression(Note::PITCH) => "Note pitch".into(),
            LaneView::Expression(Note::PRESSURE) => "Note pressure".into(),
            LaneView::Expression(_) => "Note timbre".into(),
        }
    }

    /// A line's lowest and highest values.
    fn range(self) -> (f64, f64) {
        match self {
            LaneView::Controller(c) => {
                let (lo, hi, _) = c.range();
                (lo, hi)
            }
            LaneView::Expression(Note::PITCH) => (-12.0, 12.0),
            _ => (0.0, 1.0),
        }
    }

    fn text(self, v: f64) -> String {
        match self {
            LaneView::Expression(Note::PITCH) => format!("{v:+.2} semitones"),
            LaneView::Expression(_) => format!("{:.0}%", v * 100.0),
            _ => format!("{v:.0}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Body,
    End,
    Velocity,
}

struct Drag {
    zone: Zone,
    /// The note held, by id.
    anchor: String,
    /// Timeline tick and pitch under the pointer when pressed.
    from_tick: i64,
    from_pitch: i32,
    from_y: f32,
    /// The notes held, as they were.
    originals: Vec<Note>,
    moved: bool,
}

/// Dragging across empty space to select.
struct BoxSelect {
    from: Pos2,
}

pub struct Editor {
    /// The clip being edited, by key.
    pub clip: Option<String>,
    /// Zoom: points per beat (quarter note).
    px_per_beat: f32,
    /// Beats (quarter notes) at the left edge, on the timeline.
    scroll: f64,
    /// Row height in points, and the pitch at the top row.
    row_h: f32,
    top_pitch: f32,
    pub grid: Grid,
    pub snap: bool,
    /// Quantise strength and swing, in percent.
    pub strength: f32,
    pub swing: f32,
    /// How long a drawn note is, in ticks: as the last one drawn or
    /// resized.
    draw_len: i64,
    last_velocity: u8,
    pub selected: HashSet<String>,
    drag: Option<Drag>,
    boxing: Option<BoxSelect>,
    /// Scroll to show the clip when it is next drawn.
    show_clip: bool,
    /// What the lane below shows, and the point of its line being
    /// dragged.
    pub lane: LaneView,
    line_drag: Option<usize>,
    /// The key the clip is in, and whether notes are kept on it; set by
    /// the pane before each draw.
    pub scale: Option<Scale>,
    pub scale_mode: bool,
}

impl Default for Editor {
    fn default() -> Editor {
        Editor {
            clip: None,
            px_per_beat: 60.0,
            scroll: 0.0,
            row_h: 12.0,
            top_pitch: 84.0,
            grid: Grid::Sixteenth,
            snap: true,
            strength: 100.0,
            swing: 0.0,
            draw_len: PPQ / 4,
            last_velocity: 100,
            selected: HashSet::new(),
            drag: None,
            boxing: None,
            show_clip: false,
            lane: LaneView::Velocity,
            line_drag: None,
            scale: None,
            scale_mode: false,
        }
    }
}

/// The keyboard's width, in points, and the velocity lane's height.
const KEYS: f32 = 44.0;
const VELOCITY: f32 = 80.0;
const RULER: f32 = 18.0;

/// The shortest a note can be made by dragging.
const MIN_LEN: i64 = PPQ / 32;

impl Editor {
    /// Edits this clip, scrolled into view.
    pub fn edit(&mut self, key: &str) {
        if self.clip.as_deref() != Some(key) {
            self.selected.clear();
        }
        self.clip = Some(key.to_string());
        self.show_clip = true;
    }

    /// Moves the selected notes (or all, with none selected) towards
    /// the grid.
    pub fn quantise(&self, clip: &mut NoteClip, map: &TempoMap) -> bool {
        let mut changed = false;
        let origin = clip.start - clip.offset;
        let (strength, swing) = (self.strength as f64 / 100.0, self.swing as f64 / 100.0);
        for n in &mut clip.notes {
            if !self.selected.is_empty() && !self.selected.contains(&n.id) {
                continue;
            }
            let at = quantise_tick(map, self.grid, origin + n.start, strength, swing) - origin;
            if at != n.start && at >= 0 {
                n.start = at;
                changed = true;
            }
        }
        if changed {
            clip.sort_notes();
        }
        changed
    }

    /// Moves the selected notes (or all) by `semitones`.
    pub fn transpose(&self, clip: &mut NoteClip, semitones: i32) -> bool {
        let mut changed = false;
        for n in &mut clip.notes {
            if !self.selected.is_empty() && !self.selected.contains(&n.id) {
                continue;
            }
            if let Ok(p) = u8::try_from(n.pitch as i32 + semitones)
                && p <= 127
            {
                n.pitch = p;
                changed = true;
            }
        }
        if changed {
            clip.sort_notes();
        }
        changed
    }

    /// Removes the selected notes.
    pub fn delete_selected(&mut self, clip: &mut NoteClip) -> bool {
        let before = clip.notes.len();
        clip.notes.retain(|n| !self.selected.contains(&n.id));
        self.selected.clear();
        clip.notes.len() != before
    }

    /// The editor for `clip`. Returns true if the notes changed.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        clip: &mut NoteClip,
        map: &TempoMap,
        playhead: i64,
        playing: bool,
    ) -> bool {
        let mut changed = false;
        // Toolbar.
        ui.horizontal_wrapped(|ui| {
            ui.label("Grid");
            egui::ComboBox::from_id_salt("notes-grid")
                .selected_text(self.grid.label())
                .show_ui(ui, |ui| {
                    for g in Grid::ALL {
                        ui.selectable_value(&mut self.grid, g, g.label());
                    }
                })
                .response
                .accessible_name("Note grid");
            ui.toggle_value(&mut self.snap, "Snap")
                .on_hover_text("Snap drawn and moved notes to the grid");
            ui.separator();
            ui.label("Strength");
            ui.add(
                egui::DragValue::new(&mut self.strength)
                    .range(0.0..=100.0)
                    .suffix("%"),
            )
            .accessible_name("Quantise strength")
            .on_hover_text("How far towards the grid quantising moves notes");
            ui.label("Swing");
            ui.add(
                egui::DragValue::new(&mut self.swing)
                    .range(0.0..=100.0)
                    .suffix("%"),
            )
            .accessible_name("Quantise swing")
            .on_hover_text("Every second grid line is this much of a step later");
            if ui
                .button("Quantise")
                .on_hover_text("Move the selected notes (or all) towards the grid")
                .clicked()
            {
                changed |= self.quantise(clip, map);
            }
            ui.separator();
            ui.label("Transpose");
            for (by, label) in [(-12, "−12"), (-1, "−1"), (1, "+1"), (12, "+12")] {
                if ui
                    .button(label)
                    .accessible_name(format!("Transpose {by:+} semitones"))
                    .clicked()
                {
                    changed |= self.transpose(clip, by);
                }
            }
            ui.separator();
            ui.label("Lane");
            let mut views = vec![LaneView::Velocity, LaneView::Chance];
            views.extend(Controller::COMMON.map(LaneView::Controller));
            views.extend(
                (clip.controllers.iter())
                    .filter_map(|l| Controller::from_param(&l.param))
                    .map(LaneView::Controller)
                    .filter(|v| !views_has(&Controller::COMMON, *v)),
            );
            views.extend(Note::EXPRESSIONS.map(LaneView::Expression));
            egui::ComboBox::from_id_salt("notes-lane")
                .selected_text(self.lane.label())
                .width(170.0)
                .show_ui(ui, |ui| {
                    for v in views {
                        let used = match v {
                            LaneView::Controller(c) => clip
                                .controllers
                                .iter()
                                .any(|l| l.param == c.param() && !l.points.is_empty()),
                            _ => false,
                        };
                        let text = if used {
                            format!("{} •", v.label())
                        } else {
                            v.label()
                        };
                        ui.selectable_value(&mut self.lane, v, text);
                    }
                    ui.menu_button("Other controller", |ui| {
                        for n in 0..120u8 {
                            let v = LaneView::Controller(Controller::Cc(n));
                            ui.selectable_value(&mut self.lane, v, v.label());
                        }
                    });
                })
                .response
                .accessible_name("Lane below the notes");
            if let LaneView::Expression(kind) = self.lane
                && self.selected.len() == 1
            {
                let (centre, depth) = match kind {
                    Note::PITCH => (0.0, 0.3),
                    Note::PRESSURE => (0.7, 0.3),
                    _ => (0.5, 0.4),
                };
                if ui
                    .button("Wobble")
                    .on_hover_text(
                        "Fill the note's line with a wobble, two a beat: vibrato on its pitch",
                    )
                    .clicked()
                    && let Some(n) = (clip.notes.iter_mut()).find(|n| self.selected.contains(&n.id))
                {
                    n.expression_mut(kind).points =
                        daw_project::noteops::wobble(n.len, 2.0, centre, depth);
                    changed = true;
                }
                if ui
                    .button("Clear line")
                    .clicked()
                    && let Some(n) = (clip.notes.iter_mut()).find(|n| self.selected.contains(&n.id))
                {
                    n.expression.retain(|l| l.param != kind);
                    changed = true;
                }
            }
            ui.separator();
            if ui.button("Select all").clicked() {
                self.selected = clip.notes.iter().map(|n| n.id.clone()).collect();
            }
            if ui
                .add_enabled(!self.selected.is_empty(), egui::Button::new("Delete"))
                .clicked()
            {
                changed |= self.delete_selected(clip);
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} notes{}",
                    clip.notes.len(),
                    if self.selected.is_empty() {
                        String::new()
                    } else {
                        format!(", {} selected", self.selected.len())
                    }
                ))
                .weak(),
            );
        });

        let size = vec2(ui.available_width(), ui.available_height().max(160.0));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, "Note editor grid"));
        let look = theme::look(ui.ctx());
        let tc = look.c();
        let painter = ui.painter_at(rect);
        let origin = clip.start - clip.offset;
        let grid_rect = Rect::from_min_max(
            pos2(rect.left() + KEYS, rect.top() + RULER),
            pos2(rect.right(), rect.bottom() - VELOCITY),
        );
        let vel_rect = Rect::from_min_max(
            pos2(rect.left() + KEYS, rect.bottom() - VELOCITY),
            rect.right_bottom(),
        );
        if self.show_clip {
            self.show_clip = false;
            self.scroll = clip.start as f64 / PPQ as f64;
            let fit = grid_rect.width() as f64 / (clip.len as f64 / PPQ as f64).max(1.0);
            self.px_per_beat = (fit as f32).clamp(8.0, 200.0);
            if let (Some(lo), Some(hi)) = (
                clip.notes.iter().map(|n| n.pitch).min(),
                clip.notes.iter().map(|n| n.pitch).max(),
            ) {
                let rows = (grid_rect.height() / self.row_h) as i32;
                let mid = (lo as i32 + hi as i32) / 2;
                self.top_pitch = (mid + rows / 2).clamp(12, 127) as f32;
            }
        }
        let ppb = self.px_per_beat as f64;
        // Wheel: scroll pitch, Shift (or sideways) scrolls time, Ctrl
        // zooms time about the pointer.
        if let Some(p) = resp.hover_pos() {
            let (zoom, scroll, shift) =
                ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers.shift));
            if zoom != 1.0 {
                let at = self.scroll + (p.x - grid_rect.left()) as f64 / ppb;
                self.px_per_beat = (self.px_per_beat * zoom).clamp(4.0, 400.0);
                self.scroll =
                    (at - (p.x - grid_rect.left()) as f64 / self.px_per_beat as f64).max(0.0);
            } else if scroll != egui::Vec2::ZERO {
                if shift {
                    self.scroll = (self.scroll - (scroll.y + scroll.x) as f64 / ppb).max(0.0);
                } else {
                    self.scroll = (self.scroll - scroll.x as f64 / ppb).max(0.0);
                    self.top_pitch = (self.top_pitch + scroll.y / self.row_h).clamp(12.0, 127.0);
                }
            }
        }
        let ppb = self.px_per_beat as f64;
        let scroll = self.scroll;
        let (row_h, top_pitch) = (self.row_h, self.top_pitch);
        let x_of = move |t: i64| grid_rect.left() + ((t as f64 / PPQ as f64 - scroll) * ppb) as f32;
        let tick_of = move |x: f32| {
            ((scroll + (x - grid_rect.left()) as f64 / ppb) * PPQ as f64).round() as i64
        };
        let y_of = move |pitch: f32| grid_rect.top() + (top_pitch - pitch) * row_h;
        let pitch_of = move |y: f32| (top_pitch - (y - grid_rect.top()) / row_h).ceil() as i32;
        let snap_to = |t: i64, modifiers: egui::Modifiers| {
            if self.snap && !modifiers.alt {
                snap(map, self.grid, t)
            } else {
                t
            }
        };
        let floor_snap = |t: i64, modifiers: egui::Modifiers| {
            if self.snap && !modifiers.alt {
                floor_to(map, self.grid, t)
            } else {
                t
            }
        };

        // Background: rows (black keys darker), the clip's span, grid.
        painter.rect_filled(rect, 0.0, tc.panel);
        let span = Rect::from_x_y_ranges(
            x_of(clip.start).max(grid_rect.left())..=x_of(clip.end()).min(grid_rect.right()),
            grid_rect.y_range(),
        );
        painter.rect_filled(span, 0.0, tc.lane);
        let first_pitch = pitch_of(grid_rect.bottom()).max(0);
        let last_pitch = pitch_of(grid_rect.top()).min(127);
        for pitch in first_pitch..=last_pitch {
            let y = y_of(pitch as f32);
            let r = Rect::from_x_y_ranges(grid_rect.x_range(), y..=y + row_h);
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            match self.scale {
                // Off the key's scale: shaded; its root: marked.
                Some(sc) if !sc.contains(pitch as u8) => {
                    painter.rect_filled(r.intersect(grid_rect), 0.0, tc.shadow.gamma_multiply(0.5));
                }
                Some(sc) if pitch as u8 % 12 == sc.root => {
                    painter.rect_filled(
                        r.intersect(grid_rect),
                        0.0,
                        tc.accent.gamma_multiply(0.08),
                    );
                }
                Some(_) => {}
                None if black => {
                    painter.rect_filled(
                        r.intersect(grid_rect),
                        0.0,
                        tc.shadow.gamma_multiply(0.35),
                    );
                }
                None => {}
            }
            if pitch % 12 == 0 {
                painter.line_segment(
                    [
                        pos2(grid_rect.left(), y + row_h),
                        pos2(grid_rect.right(), y + row_h),
                    ],
                    Stroke::new(1.0, tc.line),
                );
            }
            // The keyboard.
            let key = Rect::from_x_y_ranges(rect.left()..=rect.left() + KEYS - 2.0, y..=y + row_h);
            if key.intersects(Rect::from_min_max(
                rect.left_top() + vec2(0.0, RULER),
                rect.left_bottom() - vec2(0.0, VELOCITY),
            )) {
                let k = key.intersect(Rect::from_x_y_ranges(rect.x_range(), grid_rect.y_range()));
                painter.rect_filled(
                    k,
                    0.0,
                    if black {
                        tc.text
                    } else {
                        Color32::WHITE.gamma_multiply(0.9)
                    },
                );
                if pitch % 12 == 0 && row_h >= 9.0 {
                    painter.text(
                        k.right_center() - vec2(3.0, 0.0),
                        Align2::RIGHT_CENTER,
                        Note::pitch_name(pitch as u8),
                        look.font(look.small),
                        Color32::BLACK,
                    );
                }
            }
        }
        // Grid lines and the ruler: bars and beats, then the grid.
        let from_tick = tick_of(grid_rect.left()).max(0);
        let to_tick = tick_of(grid_rect.right());
        let step = self.grid.ticks(map, from_tick).max(1);
        let fine = step as f64 / PPQ as f64 * ppb >= 6.0;
        let mut t = floor_to(map, self.grid, from_tick);
        while t <= to_tick {
            let bb = map.bar_beat(Ticks(t));
            let x = x_of(t);
            let bar = t == map.bar_start(bb.bar).0;
            let beat =
                bar || (t - map.bar_start(bb.bar).0) % map.signature_at(Ticks(t)).beat_ticks() == 0;
            if bar || beat || fine {
                let (w, c) = if bar {
                    (1.5, tc.text_dim)
                } else if beat {
                    (1.0, tc.line)
                } else {
                    (0.5, tc.line.gamma_multiply(0.5))
                };
                painter.line_segment(
                    [pos2(x, grid_rect.top()), pos2(x, vel_rect.bottom())],
                    Stroke::new(w, c),
                );
            }
            if bar {
                painter.text(
                    pos2(x + 3.0, rect.top() + 2.0),
                    Align2::LEFT_TOP,
                    bb.bar.to_string(),
                    look.font(look.small),
                    tc.text,
                );
            }
            t += step;
        }
        painter.line_segment(
            [
                pos2(rect.left(), vel_rect.top()),
                pos2(rect.right(), vel_rect.top()),
            ],
            Stroke::new(1.0, tc.line),
        );
        painter.text(
            pos2(rect.left() + 4.0, vel_rect.top() + 4.0),
            Align2::LEFT_TOP,
            match self.lane {
                LaneView::Velocity => "Velocity".to_string(),
                LaneView::Chance => "Chance".to_string(),
                LaneView::Controller(Controller::Cc(n)) => format!("CC {n}"),
                LaneView::Controller(Controller::PitchBend) => "Bend".to_string(),
                LaneView::Controller(Controller::Pressure) => "Press.".to_string(),
                LaneView::Expression(k) => {
                    let mut k = k.to_string();
                    k[..1].make_ascii_uppercase();
                    k
                }
            },
            look.font(look.small),
            tc.text_dim,
        );

        // Notes.
        let note_rect = |n: &Note| -> Rect {
            let x0 = x_of(origin + n.start);
            let x1 = x_of(origin + n.end()).max(x0 + 2.0);
            let y = y_of(n.pitch as f32);
            Rect::from_x_y_ranges(x0..=x1, y + 1.0..=y + row_h - 1.0)
        };
        let bars = matches!(self.lane, LaneView::Velocity | LaneView::Chance);
        let chance = self.lane == LaneView::Chance;
        let vel_rect_of = |n: &Note| -> Rect {
            let x0 = x_of(origin + n.start);
            let k = if chance {
                n.chance as f32
            } else {
                n.velocity as f32 / 127.0
            };
            let h = (vel_rect.height() - 6.0) * k;
            Rect::from_x_y_ranges(x0..=x0 + 6.0, vel_rect.bottom() - h..=vel_rect.bottom())
        };
        let hit = |pos: Pos2| -> Option<(String, Zone)> {
            if vel_rect.contains(pos) && !bars {
                return None;
            }
            if vel_rect.contains(pos) {
                // Selected notes first, so their bars can be dragged
                // through others.
                let order = (clip.notes.iter().filter(|n| self.selected.contains(&n.id)))
                    .chain(clip.notes.iter().filter(|n| !self.selected.contains(&n.id)));
                for n in order {
                    if vel_rect_of(n).expand2(vec2(2.0, 0.0)).contains(pos) {
                        return Some((n.id.clone(), Zone::Velocity));
                    }
                }
                return None;
            }
            if !grid_rect.contains(pos) {
                return None;
            }
            for n in clip.notes.iter().rev() {
                let r = note_rect(n);
                if r.contains(pos) {
                    let zone = if r.right() - pos.x < 5.0 && r.width() > 10.0 {
                        Zone::End
                    } else {
                        Zone::Body
                    };
                    return Some((n.id.clone(), zone));
                }
            }
            None
        };
        let modifiers = ui.input(|i| i.modifiers);
        // Everything hit is worked out first, so the notes can change
        // afterwards.
        let hover_hit = resp.hover_pos().and_then(hit);
        let press_pos = ui.input(|i| i.pointer.press_origin());
        let press_hit = press_pos.and_then(hit);
        let pointer_pos = resp.interact_pointer_pos();
        let pointer_hit = pointer_pos.and_then(hit);
        if let Some(p) = resp.hover_pos()
            && self.drag.is_none()
        {
            match hover_hit.clone() {
                Some((_, Zone::End)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                Some((_, Zone::Velocity)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical),
                Some((_, Zone::Body)) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                None if grid_rect.contains(p) => ui.ctx().set_cursor_icon(CursorIcon::Crosshair),
                None => {}
            }
        }
        if resp.drag_started()
            && let Some(p) = press_pos
        {
            match press_hit.clone() {
                Some((id, zone)) => {
                    if !self.selected.contains(&id) {
                        if !modifiers.shift {
                            self.selected.clear();
                        }
                        self.selected.insert(id.clone());
                    }
                    let originals: Vec<Note> = (clip.notes.iter())
                        .filter(|n| {
                            if zone == Zone::End {
                                n.id == id
                            } else {
                                self.selected.contains(&n.id)
                            }
                        })
                        .cloned()
                        .collect();
                    self.drag = Some(Drag {
                        zone,
                        anchor: id,
                        from_tick: tick_of(p.x),
                        from_pitch: pitch_of(p.y),
                        from_y: p.y,
                        originals,
                        moved: false,
                    });
                }
                None if grid_rect.contains(p) => {
                    if !modifiers.shift {
                        self.selected.clear();
                    }
                    self.boxing = Some(BoxSelect { from: p });
                }
                None => {}
            }
        }
        if let Some(d) = &mut self.drag
            && let Some(p) = resp.interact_pointer_pos()
        {
            let anchor = d.originals.iter().find(|n| n.id == d.anchor).cloned();
            if let Some(anchor) = anchor {
                match d.zone {
                    Zone::Body => {
                        let dt = tick_of(p.x) - d.from_tick;
                        let dp = pitch_of(p.y) - d.from_pitch;
                        // The held note lands on the grid; the rest keep
                        // their distance from it.
                        let target = snap_to(origin + anchor.start + dt, modifiers) - origin;
                        let delta_t = target - anchor.start;
                        let lowest = d.originals.iter().map(|n| n.start).min().unwrap_or(0);
                        let delta_t = delta_t.max(-lowest);
                        for o in &d.originals {
                            if let Some(n) = clip.notes.iter_mut().find(|n| n.id == o.id) {
                                let mut pitch = (o.pitch as i32 + dp).clamp(0, 127) as u8;
                                if let Some(sc) = self.scale.filter(|_| self.scale_mode && dp != 0)
                                {
                                    pitch = sc.nearest(pitch);
                                }
                                let start = o.start + delta_t;
                                if (n.start, n.pitch) != (start, pitch) {
                                    n.start = start;
                                    n.pitch = pitch;
                                    d.moved = true;
                                    changed = true;
                                }
                            }
                        }
                    }
                    Zone::End => {
                        let end = snap_to(
                            origin + anchor.end() + (tick_of(p.x) - d.from_tick),
                            modifiers,
                        ) - origin;
                        let len = (end - anchor.start).max(MIN_LEN);
                        if let Some(n) = clip.notes.iter_mut().find(|n| n.id == anchor.id)
                            && n.len != len
                        {
                            n.len = len;
                            self.draw_len = len;
                            d.moved = true;
                            changed = true;
                        }
                    }
                    Zone::Velocity if chance => {
                        let dk = ((d.from_y - p.y) / (vel_rect.height() - 6.0)) as f64;
                        for o in &d.originals {
                            if let Some(n) = clip.notes.iter_mut().find(|n| n.id == o.id) {
                                let c = ((o.chance + dk) * 100.0).round().clamp(0.0, 100.0) / 100.0;
                                if n.chance != c {
                                    n.chance = c;
                                    d.moved = true;
                                    changed = true;
                                }
                            }
                        }
                    }
                    Zone::Velocity => {
                        let dv =
                            ((d.from_y - p.y) / (vel_rect.height() - 6.0) * 127.0).round() as i32;
                        for o in &d.originals {
                            if let Some(n) = clip.notes.iter_mut().find(|n| n.id == o.id) {
                                let v = (o.velocity as i32 + dv).clamp(1, 127) as u8;
                                if n.velocity != v {
                                    n.velocity = v;
                                    self.last_velocity = v;
                                    d.moved = true;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(b) = &self.boxing
            && let Some(p) = resp.interact_pointer_pos()
        {
            let r = Rect::from_two_pos(b.from, p);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, tc.accent), StrokeKind::Outside);
            painter.rect_filled(r, 0.0, tc.accent.gamma_multiply(0.15));
        }
        if resp.drag_stopped() {
            if let Some(d) = self.drag.take()
                && d.moved
            {
                clip.sort_notes();
                changed = true;
            }
            if let Some(b) = self.boxing.take()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let r = Rect::from_two_pos(b.from, p);
                for n in &clip.notes {
                    if r.intersects(note_rect(n)) {
                        self.selected.insert(n.id.clone());
                    }
                }
            }
        }
        if resp.clicked()
            && let Some(p) = pointer_pos
        {
            match pointer_hit.clone() {
                Some((id, _)) => {
                    if modifiers.shift {
                        if !self.selected.remove(&id) {
                            self.selected.insert(id);
                        }
                    } else {
                        self.selected.clear();
                        self.selected.insert(id);
                    }
                }
                None if grid_rect.contains(p) => {
                    // Draw a note here, on the scale in scale mode.
                    let mut pitch = pitch_of(p.y).clamp(0, 127) as u8;
                    if let Some(sc) = self.scale.filter(|_| self.scale_mode) {
                        pitch = sc.nearest(pitch);
                    }
                    let start = floor_snap(tick_of(p.x), modifiers) - origin;
                    if start >= 0 {
                        let n =
                            Note::new(start, self.draw_len.max(MIN_LEN), pitch, self.last_velocity);
                        self.selected.clear();
                        self.selected.insert(n.id.clone());
                        clip.notes.push(n);
                        clip.sort_notes();
                        changed = true;
                    }
                }
                None => {}
            }
        }
        if resp.secondary_clicked()
            && let Some((id, _)) = pointer_hit
        {
            clip.notes.retain(|n| n.id != id);
            self.selected.remove(&id);
            changed = true;
        }
        // Keys, while the pointer is over the editor.
        if resp.hovered() {
            let (del, all) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.modifiers.command && i.key_pressed(egui::Key::A),
                )
            });
            if del && !self.selected.is_empty() && !ui.ctx().egui_wants_keyboard_input() {
                changed |= self.delete_selected(clip);
            }
            if all {
                self.selected = clip.notes.iter().map(|n| n.id.clone()).collect();
            }
        }

        // Draw the notes and their velocities.
        let colour = tc.accent;
        let grid_painter = painter.with_clip_rect(grid_rect);
        let vel_painter = painter.with_clip_rect(vel_rect);
        for n in &clip.notes {
            let r = note_rect(n);
            let selected = self.selected.contains(&n.id);
            let inside = n.start >= clip.offset && n.start < clip.offset + clip.len;
            let shade = (0.45 + 0.55 * n.velocity as f32 / 127.0).min(1.0);
            let fill = if inside {
                colour.gamma_multiply(shade)
            } else {
                tc.text_dim.gamma_multiply(0.5)
            };
            grid_painter.rect_filled(r, 2.0, fill);
            if selected {
                grid_painter.rect_stroke(r, 2.0, Stroke::new(1.5, tc.text), StrokeKind::Inside);
            }
            if r.width() > 28.0 && row_h >= 10.0 {
                grid_painter.text(
                    r.left_center() + vec2(3.0, 0.0),
                    Align2::LEFT_CENTER,
                    Note::pitch_name(n.pitch),
                    look.font(look.small),
                    Color32::WHITE,
                );
            }
            if bars {
                let v = vel_rect_of(n);
                vel_painter.rect_filled(v, 1.0, fill);
                if selected {
                    vel_painter.rect_stroke(v, 1.0, Stroke::new(1.5, tc.text), StrokeKind::Outside);
                }
            }
            if n.expression.iter().any(|l| !l.points.is_empty()) && r.width() > 6.0 {
                // A mark on notes that change as they sound.
                grid_painter.line_segment(
                    [
                        r.left_top() + vec2(2.0, 2.0),
                        r.right_top() + vec2(-2.0, 2.0),
                    ],
                    Stroke::new(1.0, Color32::WHITE.gamma_multiply(0.7)),
                );
            }
        }
        // A line in the lane: a controller of the clip, or the selected
        // note's expression.
        if !bars {
            let (snap_on, grid) = (self.snap && !modifiers.alt, self.grid);
            let snap_line = move |t: i64| if snap_on { snap(map, grid, t) } else { t };
            let view = self.lane;
            let geo = LineGeo {
                rect: vel_rect,
                range: view.range(),
                x_of: &x_of,
                tick_of: &tick_of,
                snap: &snap_line,
            };
            match view {
                LaneView::Controller(c) => {
                    let param = c.param();
                    let at = clip.controllers.iter().position(|l| l.param == param);
                    let mut lane =
                        at.map_or_else(|| Lane::new(param), |i| clip.controllers[i].clone());
                    let (_, _, rest) = c.range();
                    let span = (clip.offset, clip.offset + clip.len);
                    let moved = edit_line(
                        ui,
                        &resp,
                        &mut lane,
                        &geo,
                        origin,
                        span,
                        rest,
                        &mut self.line_drag,
                        &vel_painter,
                        |v| view.text(v),
                    );
                    if moved {
                        changed = true;
                        match at {
                            Some(i) => clip.controllers[i] = lane,
                            None => clip.controllers.push(lane),
                        }
                        clip.controllers.retain(|l| !l.points.is_empty());
                    }
                }
                LaneView::Expression(kind) => {
                    let one = (self.selected.len() == 1)
                        .then(|| self.selected.iter().next().cloned())
                        .flatten()
                        .and_then(|id| clip.notes.iter().position(|n| n.id == id));
                    match one {
                        Some(i) => {
                            let n = &clip.notes[i];
                            let mut lane = n
                                .expression
                                .iter()
                                .find(|l| l.param == kind)
                                .cloned()
                                .unwrap_or_else(|| Lane::new(kind));
                            let rest = if kind == Note::PITCH {
                                0.0
                            } else if kind == Note::TIMBRE {
                                0.5
                            } else {
                                1.0
                            };
                            let base = origin + n.start;
                            let moved = edit_line(
                                ui,
                                &resp,
                                &mut lane,
                                &geo,
                                base,
                                (0, n.len),
                                rest,
                                &mut self.line_drag,
                                &vel_painter,
                                |v| view.text(v),
                            );
                            if moved {
                                changed = true;
                                let n = &mut clip.notes[i];
                                *n.expression_mut(kind) = lane;
                                n.expression.retain(|l| !l.points.is_empty());
                            }
                        }
                        None => {
                            vel_painter.text(
                                vel_rect.center(),
                                Align2::CENTER_CENTER,
                                "Select one note to shape it as it sounds",
                                look.font(look.small),
                                tc.text_dim,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        // The playhead.
        let ph = map.ticks_at(Frames(playhead)).0;
        let x = x_of(ph);
        if x >= grid_rect.left() && x <= grid_rect.right() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.5, tc.playhead),
            );
        }
        if playing {
            ui.ctx().request_repaint();
        }
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, tc.line), StrokeKind::Inside);
        if changed {
            clip.sort_notes();
        }
        changed
    }
}

fn views_has(common: &[Controller], v: LaneView) -> bool {
    matches!(v, LaneView::Controller(c) if common.contains(&c))
}

/// Where a line lane is drawn, and how ticks and points meet.
struct LineGeo<'a> {
    rect: Rect,
    /// The values at the bottom and the top.
    range: (f64, f64),
    x_of: &'a dyn Fn(i64) -> f32,
    tick_of: &'a dyn Fn(f32) -> i64,
    snap: &'a dyn Fn(i64) -> i64,
}

impl LineGeo<'_> {
    fn y_of(&self, v: f64) -> f32 {
        let (lo, hi) = self.range;
        let k = ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        self.rect.bottom() - 3.0 - k * (self.rect.height() - 6.0)
    }

    fn value_of(&self, y: f32) -> f64 {
        let (lo, hi) = self.range;
        let k = ((self.rect.bottom() - 3.0 - y) / (self.rect.height() - 6.0)).clamp(0.0, 1.0);
        lo + (hi - lo) * k as f64
    }
}

/// Draws and edits a line lane whose points are at `base + at` ticks on
/// the timeline, kept within `span` (in the points' own ticks); `rest`
/// is the value shown where it has no points. Returns whether the line
/// changed.
#[allow(clippy::too_many_arguments)]
fn edit_line(
    ui: &egui::Ui,
    resp: &egui::Response,
    lane: &mut Lane,
    g: &LineGeo<'_>,
    base: i64,
    span: (i64, i64),
    rest: f64,
    dragging: &mut Option<usize>,
    painter: &egui::Painter,
    text: impl Fn(f64) -> String,
) -> bool {
    let look = theme::look(ui.ctx());
    let tc = look.c();
    let mut changed = false;
    let pos_of = |p: &daw_project::AutoPoint| pos2((g.x_of)(base + p.at), g.y_of(p.value));
    let near = |lane: &Lane, pos: Pos2| -> Option<usize> {
        (lane.points.iter()).position(|p| pos_of(p).distance(pos) <= 6.0)
    };
    let at_of = |x: f32| ((g.snap)((g.tick_of)(x)) - base).clamp(span.0, span.1);
    let press = ui
        .input(|i| i.pointer.press_origin())
        .filter(|p| g.rect.contains(*p));
    if resp.drag_started()
        && let Some(p) = press
    {
        *dragging = Some(match near(lane, p) {
            Some(i) => i,
            None => {
                changed = true;
                lane.put(at_of(p.x), g.value_of(p.y))
            }
        });
    }
    if let Some(i) = *dragging
        && i < lane.points.len()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let (at, value) = (at_of(p.x), g.value_of(p.y));
        let old = lane.points[i];
        if (old.at, old.value) != (at, value) {
            lane.points.remove(i);
            *dragging = Some(lane.put(at, value));
            changed = true;
        }
        painter.text(
            p + vec2(8.0, -8.0),
            Align2::LEFT_BOTTOM,
            text(value),
            look.font(look.small),
            tc.text,
        );
    }
    if resp.drag_stopped() {
        *dragging = None;
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos().filter(|p| g.rect.contains(*p))
        && near(lane, p).is_none()
    {
        lane.put(at_of(p.x), g.value_of(p.y));
        changed = true;
    }
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos().filter(|p| g.rect.contains(*p))
        && let Some(i) = near(lane, p)
    {
        lane.points.remove(i);
        changed = true;
    }
    // The line, level before the first point and after the last.
    let (x0, x1) = ((g.x_of)(base + span.0), (g.x_of)(base + span.1));
    if lane.points.is_empty() {
        let y = g.y_of(rest);
        painter.line_segment(
            [pos2(x0, y), pos2(x1, y)],
            Stroke::new(1.0, tc.text_dim.gamma_multiply(0.5)),
        );
    } else {
        let mut line: Vec<Pos2> = Vec::with_capacity(lane.points.len() + 2);
        line.push(pos2(x0, g.y_of(lane.points[0].value)));
        line.extend(lane.points.iter().map(pos_of));
        line.push(pos2(x1, g.y_of(lane.points[lane.points.len() - 1].value)));
        painter.add(egui::Shape::line(line, Stroke::new(1.5, tc.accent)));
        for p in &lane.points {
            painter.circle_filled(pos_of(p), 3.0, tc.accent);
        }
    }
    if let Some(h) = resp.hover_pos().filter(|p| g.rect.contains(*p))
        && near(lane, h).is_some()
    {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    changed
}

impl crate::App {
    /// The note editor pane: the clip chosen on the timeline, or one
    /// picked here.
    pub(crate) fn notes_editor_pane(&mut self, ui: &mut egui::Ui) {
        // Which clips there are, to pick one.
        let clips: Vec<(String, String)> = (self.tracks.iter())
            .flat_map(|t| {
                t.note_clips
                    .iter()
                    .map(move |c| (c.id.clone(), format!("{} · {}", t.name, c.name)))
            })
            .collect();
        if let Some(key) = &self.timeline.note_sel
            && self.notes_editor.clip.as_deref() != Some(key.as_str())
            && clips.iter().any(|(k, _)| k == key)
        {
            self.notes_editor.edit(&key.clone());
        }
        ui.horizontal(|ui| {
            ui.label("Clip");
            let current = (self.notes_editor.clip.as_ref())
                .and_then(|k| clips.iter().find(|(c, _)| c == k))
                .map_or("none", |(_, n)| n.as_str());
            let mut pick = None;
            egui::ComboBox::from_id_salt("notes-clip")
                .selected_text(current)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (k, n) in &clips {
                        if ui
                            .selectable_label(self.notes_editor.clip.as_ref() == Some(k), n)
                            .clicked()
                        {
                            pick = Some(k.clone());
                        }
                    }
                })
                .response
                .accessible_name("Note clip to edit");
            if let Some(k) = pick {
                self.notes_editor.edit(&k);
                self.timeline.note_sel = Some(k);
            }
        });
        if clips.is_empty() {
            ui.label(
                "No note clips yet. Add an instrument track, then double-click it on the \
                 timeline to make one, or play a MIDI keyboard with Record on.",
            );
            return;
        }
        if self.notes_editor.clip.is_some() && self.note_tools_bar(ui) {
            return;
        }
        self.notes_editor.scale = self.editing_scale();
        self.notes_editor.scale_mode = self.note_tools.scale_mode;
        let map = self.tempo_map();
        let playhead = self.ctl.status().position().0;
        let playing = self.ctl.status().playing();
        let Some(key) = self.notes_editor.clip.clone() else {
            ui.label("Pick a clip above, or double-click one on the timeline.");
            return;
        };
        let editor = &mut self.notes_editor;
        let clip = (self.tracks.iter_mut())
            .flat_map(|t| t.note_clips.iter_mut())
            .find(|c| c.id == key);
        let Some(clip) = clip else {
            editor.clip = None;
            return;
        };
        if editor.show(ui, clip, &map, playhead, playing) {
            self.rebuild();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::time::{Tempo, TimeSignature};

    fn map() -> TempoMap {
        TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON)
    }

    #[test]
    fn quantising_moves_towards_the_grid_by_the_strength() {
        let m = map();
        // 30 ticks after a sixteenth: all the way back, or half way.
        assert_eq!(
            quantise_tick(&m, Grid::Sixteenth, PPQ / 4 + 30, 1.0, 0.0),
            PPQ / 4
        );
        assert_eq!(
            quantise_tick(&m, Grid::Sixteenth, PPQ / 4 + 30, 0.5, 0.0),
            PPQ / 4 + 15
        );
        // Nearer the next line goes forward.
        assert_eq!(quantise_tick(&m, Grid::Beat, PPQ - 10, 1.0, 0.0), PPQ);
    }

    #[test]
    fn swing_delays_every_second_grid_line() {
        let m = map();
        // Eighths with full swing: the second eighth of each beat sits
        // two thirds of the way through, as in a shuffle, at 66%.
        let swung = quantise_tick(&m, Grid::Eighth, PPQ / 2, 1.0, 0.3333);
        assert!((swung - (PPQ / 2 + PPQ / 6)).abs() <= 2, "{swung}");
        // The beat itself does not move.
        assert_eq!(quantise_tick(&m, Grid::Eighth, PPQ, 1.0, 0.5), PPQ);
    }

    #[test]
    fn transpose_and_delete_act_on_the_selection_or_everything() {
        let mut e = Editor::default();
        let mut c = NoteClip::new("c".into(), "C".into(), 0, PPQ * 4);
        c.notes = vec![Note::new(0, PPQ, 60, 100), Note::new(PPQ, PPQ, 64, 100)];
        let first = c.notes[0].id.clone();
        assert!(e.transpose(&mut c, 12));
        assert_eq!(
            c.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            [72, 76]
        );
        e.selected.insert(first);
        assert!(e.transpose(&mut c, -1));
        assert_eq!(
            c.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            [71, 76]
        );
        assert!(e.delete_selected(&mut c));
        assert_eq!(c.notes.len(), 1);
        assert_eq!(c.notes[0].pitch, 76);
    }

    #[test]
    fn quantise_keeps_notes_at_the_clips_start() {
        let e = Editor {
            grid: Grid::Beat,
            ..Editor::default()
        };
        let mut c = NoteClip::new("c".into(), "C".into(), PPQ * 4, PPQ * 4);
        c.notes = vec![
            Note::new(PPQ / 8, PPQ, 60, 100),
            Note::new(PPQ * 2 - 40, PPQ, 62, 100),
        ];
        assert!(e.quantise(&mut c, &map()));
        assert_eq!(
            c.notes.iter().map(|n| n.start).collect::<Vec<_>>(),
            [0, PPQ * 2]
        );
    }
}

#[cfg(test)]
mod app_tests {
    use std::time::{Duration, Instant};

    use daw_audio_io::SilentInput;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use crate::workspace::Pane;
    use crate::{App, Audio};
    use daw_project::{NoteClip, PPQ};
    use eframe::egui;

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        let app = h.state_mut();
        let eng = app.replace_engine();
        app.audio = Audio::Silent(
            daw_audio_io::start_silent_with(app.rate, 256, eng, SilentInput::None),
            String::new(),
        );
        h.run();
        h
    }

    fn until(h: &mut Harness<'_, App>, secs: u64, what: impl Fn(&App) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            h.step();
            if what(h.state()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn an_instrument_track_gets_a_clip_and_a_drawn_note_that_sounds() {
        let mut h = harness();
        h.get_by_label("Or add an instrument track for notes")
            .click();
        h.run();
        {
            let app = h.state_mut();
            assert_eq!(app.tracks.len(), 1);
            assert!(app.tracks[0].kind.plays_notes());
            assert!(app.tracks[0].instrument.is_some());
            let clip = NoteClip::new("clip1".into(), "Notes 1".into(), 0, PPQ * 4);
            app.tracks[0].note_clips.push(clip);
            app.notes_editor.edit("clip1");
            app.workspace.open(Pane::NotesEditor);
            app.rebuild();
        }
        h.run();
        // Click in the grid: a note appears there.
        let grid = h.get_by_label("Note editor grid").rect();
        let at = egui::pos2(
            grid.left() + 44.0 + 80.0,
            grid.top() + 18.0 + (grid.height() - 18.0 - 64.0) / 2.0,
        );
        h.input_mut().events.push(egui::Event::PointerMoved(at));
        h.step();
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        let notes = &h.state().tracks[0].note_clips[0].notes;
        assert_eq!(notes.len(), 1, "a click draws one note");
        let drawn = notes[0].clone();
        assert_eq!(drawn.start % (PPQ / 4), 0, "snapped to the sixteenth grid");
        // Transposing acts on it.
        h.get_by_label("Transpose +12 semitones").click();
        h.run();
        assert_eq!(
            h.state().tracks[0].note_clips[0].notes[0].pitch,
            drawn.pitch + 12
        );
        // Quantise with strength and swing leaves a note on the grid.
        h.get_by_label("Quantise").click();
        h.run();
        assert_eq!(
            h.state().tracks[0].note_clips[0].notes[0].start,
            drawn.start
        );
        // Playing from the note's start makes a sound.
        {
            let app = h.state_mut();
            let map = app.tempo_map();
            let at = map.frames_at(daw_engine::time::Ticks(drawn.start)).0;
            app.send(daw_engine::engine::Command::Seek(daw_engine::time::Frames(
                at,
            )));
            app.toggle_play();
        }
        assert!(
            until(&mut h, 5, |a| a.master_meter.0 > 0.001),
            "the synth is heard"
        );
        h.state_mut().toggle_play();
    }

    #[test]
    fn midi_files_go_out_and_come_back_as_tracks() {
        let mut h = harness();
        let dir = std::env::temp_dir().join(format!("daw-midi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.mid");
        {
            let app = h.state_mut();
            let mut t = crate::Track::instrument("Keys".into());
            let mut clip = NoteClip::new("c".into(), "Notes".into(), PPQ * 4, PPQ * 4);
            clip.notes.push(daw_project::Note::new(PPQ, PPQ, 64, 90));
            t.note_clips.push(clip);
            app.tracks.push(t);
            app.write_midi(&path);
            assert!(app.message.as_deref().unwrap_or("").starts_with("Exported"));
            app.tracks.clear();
            app.import_midi(&path);
            assert_eq!(app.tracks.len(), 1);
            assert_eq!(app.tracks[0].name, "Keys");
            let notes = &app.tracks[0].note_clips[0].notes;
            assert_eq!(notes.len(), 1);
            // Placed on the timeline: a bar in, plus the beat in the clip.
            assert_eq!(notes[0].start, PPQ * 5);
            assert_eq!(notes[0].pitch, 64);
        }
        h.run();
        let _ = std::fs::remove_dir_all(dir);
    }
}
