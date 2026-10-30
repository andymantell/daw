// SPDX-License-Identifier: GPL-3.0-or-later
//! The pattern editor (roadmap 3.3): a drum pattern's lanes of steps as
//! a grid, one row per sound. Click a step to turn it on or off (and
//! pick it); right click picks it without changing it. Below the grid
//! are the picked lane's settings (its note, length, rate and
//! direction, and a Euclidean fill) and the picked step's (velocity,
//! pitch, chance, repeats, offset, gate and how much the velocity
//! varies).
//!
//! Steps are drawn as wide as they last, so lanes at different rates
//! line up in time. Patterns are played by note clips that link to
//! them (see `daw_project::Pattern`); the app keeps the patterns and
//! makes those clips' notes from them on every rebuild.

use daw_engine::time::{Frames, Ticks};
use daw_project::{
    DRUM_LANES, Direction, MAX_GATE, MAX_REPEATS, MAX_STEPS, MIN_GATE, Note, NoteClip, Pattern,
    PatternLane, StepRate,
};
use eframe::egui::{self, Align2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::theme;

#[derive(Default)]
pub struct Editor {
    /// The pattern being edited, by id.
    pub pattern: Option<String>,
    /// The clip it was opened from, for showing where it plays.
    pub clip: Option<String>,
    /// The picked lane and step.
    pub lane: usize,
    pub step: Option<usize>,
    /// The Euclidean fill's hits and turn, for the picked lane.
    hits: usize,
    rotate: usize,
}

/// Points across a sixteenth-note step, and a lane's height.
const SIXTEENTH: f32 = 24.0;
const ROW: f32 = 26.0;
/// The lane names' column.
const NAMES: f32 = 130.0;

impl Editor {
    /// Edits `pattern`, played by `clip`.
    pub fn edit(&mut self, pattern: &str, clip: Option<&str>) {
        if self.pattern.as_deref() != Some(pattern) {
            self.lane = 0;
            self.step = None;
        }
        self.pattern = Some(pattern.to_string());
        self.clip = clip.map(str::to_string);
    }

    /// The pattern's settings, its grid and the picked lane's and step's
    /// settings. `at` is where it is playing, in ticks into the
    /// content of the clip it was opened from. True if it changed.
    pub fn show(&mut self, ui: &mut egui::Ui, p: &mut Pattern, at: Option<i64>, bar: i64) -> bool {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Name");
            changed |= ui
                .add(egui::TextEdit::singleline(&mut p.name).desired_width(140.0))
                .accessible_name("Pattern name")
                .changed();
            ui.separator();
            ui.label("Length");
            let mut bars = (p.len as f64 / bar.max(1) as f64).max(0.25);
            if ui
                .add(
                    egui::DragValue::new(&mut bars)
                        .range(0.25..=16.0)
                        .speed(0.05)
                        .suffix(" bars"),
                )
                .accessible_name("Pattern length in bars")
                .on_hover_text("How long a new clip of it is")
                .changed()
            {
                p.len = ((bars * bar as f64).round() as i64).max(1);
                changed = true;
            }
            ui.label("Swing");
            let mut swing = (p.swing * 100.0) as f32;
            if ui
                .add(egui::Slider::new(&mut swing, 0.0..=100.0).suffix("%"))
                .accessible_name("Swing")
                .on_hover_text("Every second step of each lane is this much of half a step late")
                .changed()
            {
                p.swing = swing as f64 / 100.0;
                changed = true;
            }
            ui.separator();
            if ui
                .add_enabled(p.lanes.len() < 32, egui::Button::new("Add lane"))
                .on_hover_text("Another sound, with steps of its own")
                .clicked()
            {
                let used: Vec<u8> = p.lanes.iter().map(|l| l.pitch).collect();
                let (name, pitch) = DRUM_LANES
                    .iter()
                    .copied()
                    .chain([("Clap", 39), ("Low tom", 45), ("Ride", 51), ("Crash", 49)])
                    .find(|(_, n)| !used.contains(n))
                    .unwrap_or(("Lane", 60));
                let steps = p.lanes.first().map_or(16, |l| l.steps.len());
                p.lanes.push(PatternLane::new(
                    daw_project::new_id(),
                    name.into(),
                    pitch,
                    steps,
                ));
                self.lane = p.lanes.len() - 1;
                self.step = None;
                changed = true;
            }
        });
        ui.add_space(4.0);

        changed |= self.grid(ui, p, at);
        ui.add_space(6.0);
        self.lane = self.lane.min(p.lanes.len().saturating_sub(1));
        if p.lanes.is_empty() {
            ui.label("No lanes. Add one to give the pattern a sound.");
            return changed;
        }
        changed |= self.lane_settings(ui, p);
        changed |= self.step_settings(ui, p);
        changed
    }

    fn grid(&mut self, ui: &mut egui::Ui, p: &mut Pattern, at: Option<i64>) -> bool {
        let mut changed = false;
        let look = theme::look(ui.ctx());
        let tc = look.c().clone();
        let px_per_tick = SIXTEENTH / StepRate::Sixteenth.ticks() as f32;
        let longest = (p.lanes.iter())
            .map(|l| l.steps.len() as i64 * l.rate.ticks())
            .max()
            .unwrap_or(0)
            .max(p.len);
        let width = NAMES + longest as f32 * px_per_tick + 8.0;
        let height = ROW * p.lanes.len() as f32 + 4.0;
        egui::ScrollArea::horizontal()
            .id_salt("pattern-grid")
            .min_scrolled_height(height)
            .show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    vec2(
                        width.max(ui.available_width()),
                        ROW * p.lanes.len() as f32 + 4.0,
                    ),
                    Sense::hover(),
                );
                let painter = ui.painter_at(rect);
                let left = rect.left() + NAMES;
                let x_of = |t: i64| left + t as f32 * px_per_tick;
                let mut mute = None;
                for (li, lane) in p.lanes.iter().enumerate() {
                    let top = rect.top() + li as f32 * ROW;
                    // The name: pick the lane; the dot mutes it.
                    let name_r =
                        Rect::from_min_size(pos2(rect.left(), top), vec2(NAMES - 26.0, ROW - 2.0));
                    let r = ui.interact(name_r, ui.scope_id().with(("lane", li)), Sense::click());
                    r.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            true,
                            self.lane == li,
                            format!("{} lane", lane.name),
                        )
                    });
                    if r.clicked() {
                        self.lane = li;
                    }
                    if self.lane == li {
                        painter.rect_filled(name_r, 3.0, tc.selection);
                    }
                    painter.text(
                        name_r.left_center() + vec2(4.0, 0.0),
                        Align2::LEFT_CENTER,
                        &lane.name,
                        look.font(look.body),
                        if lane.muted { tc.text_dim } else { tc.text },
                    );
                    let m = Rect::from_center_size(
                        pos2(left - 13.0, top + ROW / 2.0 - 1.0),
                        vec2(18.0, 18.0),
                    );
                    let mr = ui.interact(m, ui.scope_id().with(("mute", li)), Sense::click());
                    mr.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::CheckBox,
                            true,
                            lane.muted,
                            format!("Mute {}", lane.name),
                        )
                    });
                    painter.text(
                        m.center(),
                        Align2::CENTER_CENTER,
                        "M",
                        look.font(look.small),
                        if lane.muted { tc.record } else { tc.text_dim },
                    );
                    if mr.clicked() {
                        mute = Some(li);
                    }
                }
                if let Some(li) = mute {
                    p.lanes[li].muted = !p.lanes[li].muted;
                    changed = true;
                }
                // The steps.
                let mut toggle = None;
                for li in 0..p.lanes.len() {
                    let lane = &p.lanes[li];
                    let top = rect.top() + li as f32 * ROW;
                    let len = lane.rate.ticks();
                    let playing = at.and_then(|t| p.step_at(li, t));
                    for (si, step) in lane.steps.iter().enumerate() {
                        let t0 = si as i64 * len;
                        let cell = Rect::from_x_y_ranges(
                            x_of(t0) + 1.0..=x_of(t0 + len) - 1.0,
                            top + 1.0..=top + ROW - 3.0,
                        );
                        let id = ui.scope_id().with(("step", li, si));
                        let r = ui.interact(cell, id, Sense::click());
                        r.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::Role::CheckBox,
                                true,
                                step.on,
                                format!("{} step {}", lane.name, si + 1),
                            )
                        });
                        let on_beat = t0 % daw_project::PPQ == 0;
                        let past = t0 >= p.len;
                        let bg = if on_beat { tc.raised } else { tc.lane };
                        painter.rect_filled(
                            cell,
                            3.0,
                            if past { bg.gamma_multiply(0.5) } else { bg },
                        );
                        if step.on {
                            let shade = 0.35 + 0.65 * step.velocity as f32 / 127.0;
                            let fill = if lane.muted {
                                tc.text_dim
                            } else {
                                tc.accent.gamma_multiply(shade)
                            };
                            if step.chance < 1.0 {
                                // Partly filled, from the bottom, by its chance.
                                let h = (cell.height() * step.chance as f32).max(3.0);
                                let part = Rect::from_x_y_ranges(
                                    cell.x_range(),
                                    cell.bottom() - h..=cell.bottom(),
                                );
                                painter.rect_stroke(
                                    cell,
                                    3.0,
                                    Stroke::new(1.0, fill),
                                    StrokeKind::Inside,
                                );
                                painter.rect_filled(part, 3.0, fill);
                            } else {
                                painter.rect_filled(cell, 3.0, fill);
                            }
                            // Ratchets: a tick per extra hit.
                            for k in 1..step.repeats {
                                let x = cell.left() + cell.width() * k as f32 / step.repeats as f32;
                                painter.line_segment(
                                    [pos2(x, cell.top() + 4.0), pos2(x, cell.bottom() - 4.0)],
                                    Stroke::new(1.0, tc.on_accent),
                                );
                            }
                        }
                        if playing == Some(si) {
                            painter.rect_stroke(
                                cell,
                                3.0,
                                Stroke::new(2.0, tc.playhead),
                                StrokeKind::Outside,
                            );
                        }
                        if self.lane == li && self.step == Some(si) {
                            painter.rect_stroke(
                                cell,
                                3.0,
                                Stroke::new(2.0, tc.text),
                                StrokeKind::Inside,
                            );
                        }
                        if r.clicked() {
                            toggle = Some((li, si));
                        }
                        if r.secondary_clicked() {
                            self.lane = li;
                            self.step = Some(si);
                        }
                    }
                }
                if let Some((li, si)) = toggle {
                    let s = &mut p.lanes[li].steps[si];
                    s.on = !s.on;
                    self.lane = li;
                    self.step = Some(si);
                    changed = true;
                }
                // Where the pattern ends: a new clip of it is this long.
                let x = x_of(p.len);
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.5, tc.marker),
                );
                if at.is_some() {
                    ui.ctx().request_repaint();
                }
            });
        changed
    }

    fn lane_settings(&mut self, ui: &mut egui::Ui, p: &mut Pattern) -> bool {
        let mut changed = false;
        let mut remove = false;
        let lane = &mut p.lanes[self.lane];
        ui.horizontal_wrapped(|ui| {
            ui.strong("Lane");
            changed |= ui
                .add(egui::TextEdit::singleline(&mut lane.name).desired_width(110.0))
                .accessible_name("Lane name")
                .changed();
            ui.label("Note");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut lane.pitch)
                        .range(0..=127)
                        .custom_formatter(|v, _| format!("{} ({v})", Note::pitch_name(v as u8))),
                )
                .accessible_name("Lane note")
                .on_hover_text("The note it plays: a drum machine's pad listens for it")
                .changed();
            ui.label("Steps");
            let mut n = lane.steps.len();
            if ui
                .add(egui::DragValue::new(&mut n).range(1..=MAX_STEPS))
                .accessible_name("Lane steps")
                .on_hover_text("How many steps before it starts again; lanes of different lengths drift against each other")
                .changed()
            {
                lane.set_len(n);
                self.step = self.step.filter(|&s| s < n);
                changed = true;
            }
            egui::ComboBox::from_id_salt("pattern-rate")
                .selected_text(lane.rate.label())
                .show_ui(ui, |ui| {
                    for r in StepRate::ALL {
                        changed |= ui.selectable_value(&mut lane.rate, r, r.label()).changed();
                    }
                })
                .response
                .accessible_name("Lane step length")
                .on_hover_text("How long each step lasts");
            egui::ComboBox::from_id_salt("pattern-direction")
                .selected_text(lane.direction.label())
                .show_ui(ui, |ui| {
                    for d in Direction::ALL {
                        changed |= ui.selectable_value(&mut lane.direction, d, d.label()).changed();
                    }
                })
                .response
                .accessible_name("Lane direction")
                .on_hover_text("Which way it goes through its steps");
            ui.separator();
            let steps = lane.steps.len();
            self.hits = self.hits.min(steps);
            ui.add(egui::DragValue::new(&mut self.hits).range(0..=steps).suffix(" hits"))
                .accessible_name("Fill hits");
            ui.add(egui::DragValue::new(&mut self.rotate).range(0..=steps.saturating_sub(1)).prefix("turned "))
                .accessible_name("Fill turn");
            if ui
                .button("Fill")
                .on_hover_text("Spread the hits as evenly as they go over the steps (a Euclidean rhythm)")
                .clicked()
            {
                lane.fill(self.hits, self.rotate);
                changed = true;
            }
            if ui.button("Clear").on_hover_text("Turn every step off").clicked() {
                lane.fill(0, 0);
                changed = true;
            }
            ui.separator();
            if ui.button("Remove lane").clicked() {
                remove = true;
            }
        });
        if remove {
            p.lanes.remove(self.lane);
            self.lane = self.lane.saturating_sub(1);
            self.step = None;
            changed = true;
        }
        changed
    }

    fn step_settings(&mut self, ui: &mut egui::Ui, p: &mut Pattern) -> bool {
        let mut changed = false;
        let Some(lane) = p.lanes.get_mut(self.lane) else {
            return false;
        };
        let Some(s) = self.step.and_then(|i| lane.steps.get_mut(i)) else {
            ui.label(
                egui::RichText::new(
                    "Click a step to turn it on or off; right click one to change it without.",
                )
                .weak(),
            );
            return false;
        };
        let k = self.step.unwrap_or(0) + 1;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Step {k}"));
            changed |= ui.checkbox(&mut s.on, "On").changed();
            ui.label("Velocity");
            changed |= ui
                .add(egui::DragValue::new(&mut s.velocity).range(1..=127))
                .accessible_name("Step velocity")
                .changed();
            ui.label("±");
            changed |= ui
                .add(egui::DragValue::new(&mut s.vary).range(0..=127))
                .accessible_name("Step velocity variance")
                .on_hover_text("Up to how far the velocity varies either way, hit to hit")
                .changed();
            ui.label("Pitch");
            changed |= ui
                .add(egui::DragValue::new(&mut s.pitch).range(-48..=48).suffix(" st"))
                .accessible_name("Step pitch")
                .on_hover_text("Semitones from the lane's note")
                .changed();
            ui.label("Chance");
            let mut chance = (s.chance * 100.0).round() as i32;
            if ui
                .add(egui::DragValue::new(&mut chance).range(0..=100).suffix("%"))
                .accessible_name("Step chance")
                .on_hover_text("How likely it is to play each time round")
                .changed()
            {
                s.chance = chance as f64 / 100.0;
                changed = true;
            }
            ui.label("Repeats");
            changed |= ui
                .add(egui::DragValue::new(&mut s.repeats).range(1..=MAX_REPEATS))
                .accessible_name("Step repeats")
                .on_hover_text("Hits in the step (a ratchet)")
                .changed();
            ui.label("Offset");
            let mut off = (s.offset * 100.0).round() as i32;
            if ui
                .add(egui::DragValue::new(&mut off).range(-50..=50).suffix("%"))
                .accessible_name("Step offset")
                .on_hover_text("Early or late, as a share of a step")
                .changed()
            {
                s.offset = off as f64 / 100.0;
                changed = true;
            }
            ui.label("Gate");
            let mut gate = (s.gate * 100.0).round() as i32;
            if ui
                .add(
                    egui::DragValue::new(&mut gate)
                        .range((MIN_GATE * 100.0) as i32..=(MAX_GATE * 100.0) as i32)
                        .suffix("%"),
                )
                .accessible_name("Step gate")
                .on_hover_text("How long each hit lasts, as a share of the step; past 100% it ties into the next")
                .changed()
            {
                s.gate = gate as f64 / 100.0;
                changed = true;
            }
        });
        changed
    }
}

impl crate::App {
    /// Makes the notes of every clip that plays a pattern.
    pub(crate) fn fill_patterns(&mut self) {
        for c in self.tracks.iter_mut().flat_map(|t| t.note_clips.iter_mut()) {
            daw_project::fill_pattern_clip(&self.patterns, c);
        }
    }

    /// A new drum pattern, a bar long, and a clip playing it on track
    /// `lane` from frame `at` (snapped by the caller). The clip's key.
    pub(crate) fn add_pattern_clip(&mut self, lane: usize, at: i64) -> Option<String> {
        if !self.tracks.get(lane)?.kind.plays_notes() {
            return None;
        }
        let map = self.tempo_map();
        let start = map.ticks_at(Frames(at.max(0)));
        let bar = map.signature_at(start).bar_ticks();
        let n = self.patterns.len() + 1;
        let pat = Pattern::drums(daw_project::new_id(), format!("Pattern {n}"), bar);
        let mut c = NoteClip::new(daw_project::new_id(), pat.name.clone(), start.0, pat.len);
        c.pattern = Some(pat.id.clone());
        let key = c.id.clone();
        self.pattern_editor.edit(&pat.id, Some(&key));
        self.patterns.push(pat);
        self.tracks[lane].note_clips.push(c);
        self.tracks[lane].note_clips.sort_by_key(|c| c.start);
        self.timeline.note_sel = Some(key.clone());
        self.workspace.open(crate::workspace::Pane::Patterns);
        self.rebuild();
        Some(key)
    }

    /// The note clip picked on the timeline, if it plays a pattern.
    pub(crate) fn selected_pattern_clip(&self) -> Option<String> {
        let key = self.timeline.note_sel.as_deref()?;
        (self.tracks.iter())
            .flat_map(|t| &t.note_clips)
            .find(|c| c.id == key && c.pattern.is_some())
            .map(|c| c.id.clone())
    }

    /// A new pattern clip at the bar the playhead is in, on the picked
    /// note clip's track, else the first instrument track, else a new
    /// one.
    pub(crate) fn add_pattern_at_playhead(&mut self) {
        let picked = (self.timeline.note_sel.as_deref()).and_then(|k| {
            self.tracks
                .iter()
                .position(|t| t.note_clips.iter().any(|c| c.id == k))
        });
        let lane = match picked.or_else(|| self.tracks.iter().position(|t| t.kind.plays_notes())) {
            Some(l) => l,
            None => {
                let n = self.tracks.iter().filter(|t| t.kind.plays_notes()).count() + 1;
                let t = crate::Track::instrument(format!("Instrument {n}"));
                if let Some(i) = &t.instrument {
                    self.pending_instruments.push((i.id, i.make(self.rate)));
                }
                self.tracks.push(t);
                self.tracks.len() - 1
            }
        };
        let map = self.tempo_map();
        let ph = map.ticks_at(Frames(self.ctl.status().position().0));
        let bar = map.frames_at(map.bar_start(map.bar_beat(ph).bar)).0;
        self.add_pattern_clip(lane, bar);
    }

    /// Makes note clip `key`'s notes its own, no longer following its
    /// pattern.
    pub(crate) fn pattern_to_notes(&mut self, key: &str) {
        self.fill_patterns();
        if let Some(c) = (self.tracks.iter_mut())
            .flat_map(|t| t.note_clips.iter_mut())
            .find(|c| c.id == key)
        {
            c.pattern = None;
            for n in &mut c.notes {
                n.id = daw_project::new_id();
            }
        }
        self.rebuild();
    }

    /// Opens the pattern editor on the pattern note clip `key` plays.
    pub(crate) fn edit_pattern_of(&mut self, key: &str) {
        let pat = (self.tracks.iter())
            .flat_map(|t| &t.note_clips)
            .find(|c| c.id == key)
            .and_then(|c| c.pattern.clone());
        if let Some(pat) = pat {
            self.pattern_editor.edit(&pat, Some(key));
            self.workspace.open(crate::workspace::Pane::Patterns);
        }
    }

    /// Gives note clip `key` a copy of its pattern of its own.
    pub(crate) fn make_pattern_unique(&mut self, key: &str) {
        let Some(clip) = (self.tracks.iter_mut())
            .flat_map(|t| t.note_clips.iter_mut())
            .find(|c| c.id == key)
        else {
            return;
        };
        let Some(p) =
            (clip.pattern.as_ref()).and_then(|id| self.patterns.iter().find(|p| &p.id == id))
        else {
            return;
        };
        let copy = p.copy(format!("{} copy", p.name));
        clip.pattern = Some(copy.id.clone());
        self.pattern_editor.edit(&copy.id, Some(key));
        self.patterns.push(copy);
        self.rebuild();
    }

    /// How many note clips play pattern `id`.
    fn pattern_uses(&self, id: &str) -> usize {
        (self.tracks.iter())
            .flat_map(|t| &t.note_clips)
            .filter(|c| c.pattern.as_deref() == Some(id))
            .count()
    }

    /// The pattern editor pane.
    pub(crate) fn pattern_editor_pane(&mut self, ui: &mut egui::Ui) {
        // Following the note clip picked on the timeline.
        if let Some(key) = self.timeline.note_sel.clone()
            && self.pattern_editor.clip.as_deref() != Some(key.as_str())
            && let Some(pat) = (self.tracks.iter())
                .flat_map(|t| &t.note_clips)
                .find(|c| c.id == key)
                .and_then(|c| c.pattern.clone())
        {
            self.pattern_editor.edit(&pat, Some(&key));
        }
        if self.patterns.is_empty() {
            ui.label(
                "No patterns yet. Right click an instrument track on the timeline and choose \
                 Add drum pattern here.",
            );
            return;
        }
        let names: Vec<(String, String)> = (self.patterns.iter())
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let current = self.pattern_editor.pattern.clone();
        let uses = current.as_deref().map_or(0, |id| self.pattern_uses(id));
        let mut delete = false;
        ui.horizontal(|ui| {
            ui.label("Pattern");
            let shown = (current.as_ref())
                .and_then(|k| names.iter().find(|(id, _)| id == k))
                .map_or("none", |(_, n)| n.as_str());
            let mut pick = None;
            egui::ComboBox::from_id_salt("pattern-pick")
                .selected_text(shown)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for (id, n) in &names {
                        if ui
                            .selectable_label(current.as_ref() == Some(id), n)
                            .clicked()
                        {
                            pick = Some(id.clone());
                        }
                    }
                })
                .response
                .accessible_name("Pattern to edit");
            if let Some(id) = pick {
                self.pattern_editor.edit(&id, None);
            }
            if current.is_some() {
                ui.label(
                    egui::RichText::new(match uses {
                        0 => "not used by any clip".to_string(),
                        1 => "played by 1 clip".to_string(),
                        n => format!("played by {n} linked clips"),
                    })
                    .weak(),
                );
                delete = ui
                    .add_enabled(uses == 0, egui::Button::new("Delete pattern"))
                    .on_disabled_hover_text("Clips still play it")
                    .clicked();
            }
        });
        let Some(id) = current else {
            ui.label("Pick a pattern above, or double-click a pattern clip on the timeline.");
            return;
        };
        if delete {
            self.patterns.retain(|p| p.id != id);
            self.pattern_editor.pattern = None;
            return;
        }
        // Where it plays: in the clip it was opened from, or the first
        // that plays it.
        let map = self.tempo_map();
        let status = self.ctl.status();
        let at = status
            .playing()
            .then(|| map.ticks_at(Frames(status.position().0)).0);
        let clip_key = self.pattern_editor.clip.clone();
        let at = at.and_then(|t| {
            let c = (self.tracks.iter())
                .flat_map(|t| &t.note_clips)
                .filter(|c| c.pattern.as_deref() == Some(id.as_str()) && !c.muted)
                .find(|c| clip_key.is_none() || clip_key.as_deref() == Some(c.id.as_str()))?;
            (t >= c.start && t < c.end()).then(|| t - c.start + c.offset)
        });
        let bar = map.signature_at(Ticks(0)).bar_ticks();
        let Some(p) = self.patterns.iter_mut().find(|p| p.id == id) else {
            self.pattern_editor.pattern = None;
            return;
        };
        if self.pattern_editor.show(ui, p, at, bar) {
            self.rebuild();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use daw_audio_io::SilentInput;
    use daw_project::PPQ;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use crate::workspace::Pane;
    use crate::{App, Audio};

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

    fn kicks(app: &App, clip: usize) -> usize {
        (app.tracks[0].note_clips[clip].notes.iter())
            .filter(|n| n.pitch == 36)
            .count()
    }

    #[test]
    fn a_pattern_clip_plays_its_steps_and_linked_clips_follow_edits() {
        let mut h = harness();
        h.get_by_label("Or add an instrument track for notes")
            .click();
        h.run();
        let key = h
            .state_mut()
            .add_pattern_clip(0, 0)
            .expect("a pattern clip");
        h.run();
        assert!(h.state().workspace.layout.is_open(Pane::Patterns));
        assert_eq!(h.state().patterns.len(), 1);
        assert_eq!(
            kicks(h.state(), 0),
            2,
            "the starting beat has two kicks a bar"
        );

        // A linked copy after it, as the timeline's Duplicate makes.
        {
            let app = h.state_mut();
            let mut c = app.tracks[0].note_clips[0].clone();
            c.id = daw_project::new_id();
            c.start = c.end();
            app.tracks[0].note_clips.push(c);
            app.rebuild();
        }
        assert_eq!(kicks(h.state(), 1), 2);

        // Clicking a step in the editor turns it on in both clips.
        h.get_by_label("Kick step 5").click();
        h.run();
        assert!(h.state().patterns[0].lanes[0].steps[4].on);
        assert_eq!(kicks(h.state(), 0), 3);
        assert_eq!(kicks(h.state(), 1), 3, "the linked clip follows");

        // Made unique, the first clip keeps its own copy.
        h.state_mut().make_pattern_unique(&key);
        h.run();
        assert_eq!(h.state().patterns.len(), 2);
        h.get_by_label("Kick step 5").click();
        h.run();
        assert_eq!(kicks(h.state(), 0), 2);
        assert_eq!(kicks(h.state(), 1), 3, "the other clip is left as it was");

        // The pattern is saved with the project and comes back.
        let p = h.state().project();
        assert_eq!(p.patterns.len(), 2);
        let mut doc = daw_project::ProjectDoc::new(9);
        doc.set(&p);
        let back = doc.get();
        assert_eq!(back.patterns, p.patterns);
        assert_eq!(
            back.tracks[0].note_clips[1].notes.len(),
            p.tracks[0].note_clips[1].notes.len()
        );

        // And it plays.
        h.state_mut().toggle_play();
        assert!(
            until(&mut h, 5, |a| a.master_meter.0 > 0.001),
            "the pattern is heard"
        );
        h.state_mut().toggle_play();
        assert_eq!(h.state().tracks[0].note_clips[0].len, PPQ * 4);
    }
}
