// SPDX-License-Identifier: GPL-3.0-or-later
//! Fixing a badly timed take (roadmap 2.1, first cut): finds where each
//! note starts in the selected clips, cuts them there, and moves each
//! piece toward the nearest grid line, all the way or part of it (the
//! strength). Where pieces now meet, the later one is drawn back over
//! the gap with a short crossfade, so nothing plays twice and nothing
//! drops out.
//!
//! On grouped tracks, such as the mics on a drum kit, the notes are
//! found on one track (or all of them together) and every track of the
//! group is cut and moved by the same amounts, so the mics stay in time
//! with each other. Pieces of a comp are left alone: the comp would make
//! them again from the takes.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use daw_engine::fade::{Curve, Fade};
use daw_engine::time::TempoMap;
use eframe::egui::{self, RichText, Sense, Stroke, pos2, vec2};

use crate::App;
use crate::groups::{self, LaneGroup};
use crate::timeline::{self, Clip, Grid, MIN_LEN};

/// How long the crossfade where two moved pieces meet lasts.
pub const JOIN_SECONDS: f64 = 0.005;

/// Two notes closer than this are taken as one.
pub const MIN_GAP_SECONDS: f64 = 0.05;

/// How long the level is held when following it, and how far back an
/// attack's rise is measured from.
const HOLD_SECONDS: f64 = 0.010;
const LOOK_BACK_SECONDS: f64 = 0.015;

/// How far below the loudest part of the audio a note may start and
/// still be found, in dB.
const FLOOR_DB: f32 = 40.0;

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-6).log10()
}

fn frames(rate: u32, secs: f64) -> usize {
    (rate as f64 * secs).round() as usize
}

/// How much the level must jump for a note to start, in dB, at
/// `sensitivity` from 0 (only clear, loud notes) to 1 (soft ones too).
pub fn rise_db(sensitivity: f32) -> f32 {
    16.0 - 12.0 * sensitivity.clamp(0.0, 1.0)
}

/// Where notes start in `samples` (mono, at `rate`), in order: the zero
/// crossing just before each sudden jump in level, so a cut there is
/// clean and keeps the whole attack.
pub fn attacks(samples: &[f32], rate: u32, sensitivity: f32) -> Vec<usize> {
    let hop = (rate as usize / 1000).max(1);
    let n = samples.len() / hop;
    if n == 0 {
        return Vec::new();
    }
    // The loudest sample in each millisecond, then held for a while so
    // a low note's slow wave reads as a steady level.
    let peak: Vec<f32> = (samples.chunks(hop))
        .take(n)
        .map(|c| c.iter().fold(0.0f32, |m, x| m.max(x.abs())))
        .collect();
    let hold = (frames(rate, HOLD_SECONDS) / hop).max(1);
    let env: Vec<f32> = (0..n)
        .map(|i| {
            peak[i.saturating_sub(hold - 1)..=i]
                .iter()
                .fold(0.0f32, |m, &x| m.max(x))
        })
        .collect();
    let loudest = env.iter().fold(0.0f32, |m, &x| m.max(x));
    if loudest <= 1e-6 {
        return Vec::new();
    }
    let floor = db(loudest) - FLOOR_DB;
    let rise = rise_db(sensitivity);
    let back = (frames(rate, LOOK_BACK_SECONDS) / hop).max(1);
    let gap = frames(rate, MIN_GAP_SECONDS) / hop;

    let mut out = Vec::new();
    let mut last: Option<usize> = None;
    for i in 0..n {
        if last.is_some_and(|l| i < l + gap) {
            continue;
        }
        let before = if i >= back { env[i - back] } else { 0.0 };
        if db(env[i]) < floor || db(env[i]) - db(before) < rise {
            continue;
        }
        last = Some(i);
        out.push(onset(samples, hop, back, i, before));
    }
    out
}

/// The sample where the note found in hop `i` starts: the first one
/// well above the level before it, moved back to the zero crossing
/// before that.
fn onset(samples: &[f32], hop: usize, back: usize, i: usize, before: f32) -> usize {
    // Back as far as the level it rose from, which the note's start
    // can't be before.
    let from = i.saturating_sub(back) * hop;
    let to = ((i + 6) * hop).min(samples.len());
    let top = samples[i * hop..to]
        .iter()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    let over = (before * 1.41).max(top * 0.1).min(top * 0.9);
    let first = (from..to)
        .find(|&s| samples[s].abs() >= over)
        .unwrap_or(i * hop);
    // Back down the rising wave to where it crossed zero, or stopped
    // rising, at most 2 ms back.
    let lo = first.saturating_sub(2 * hop);
    let mut s = first;
    while s > lo
        && samples[s - 1] != 0.0
        && samples[s - 1].signum() == samples[s].signum()
        && samples[s - 1].abs() < samples[s].abs()
    {
        s -= 1;
    }
    s
}

/// A cut at timeline frame `at`, and how far the audio after it moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub at: i64,
    pub by: i64,
}

/// The cuts and moves that take notes starting at `attacks` (timeline
/// frames) `strength` of the way (0 to 1) to the nearest line of `grid`
/// (which isn't `Auto`). A note that would land on or just after the
/// one before it stays with that one, uncut.
pub fn moves(map: &TempoMap, attacks: &[i64], grid: Grid, strength: f32) -> Vec<Move> {
    let gap = frames(map.sample_rate(), MIN_GAP_SECONDS) as i64;
    let mut sorted = attacks.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<Move> = Vec::new();
    for at in sorted {
        let target = timeline::snap(map, at, grid);
        let by = ((target - at) as f64 * strength.clamp(0.0, 1.0) as f64).round() as i64;
        let by = by.max(-at);
        if out.last().is_some_and(|m| at + by < m.at + m.by + gap) {
            continue;
        }
        out.push(Move { at, by });
    }
    out
}

/// Notes found on several tracks merged into one list: those closer
/// than [`MIN_GAP_SECONDS`] to an earlier one are the same note.
pub fn merge(rate: u32, lists: &[Vec<i64>]) -> Vec<i64> {
    let gap = frames(rate, MIN_GAP_SECONDS) as i64;
    let mut all: Vec<i64> = lists.iter().flatten().copied().collect();
    all.sort_unstable();
    let mut out: Vec<i64> = Vec::new();
    for a in all {
        if out.last().is_none_or(|&l| a >= l + gap) {
            out.push(a);
        }
    }
    out
}

/// `clip` cut at the moves inside it, each piece moved, and the pieces
/// joined again with crossfades `join` frames long. A note right at the
/// clip's start moves the first piece. The outer fades stay at the
/// outer ends.
pub fn fix_clip(clip: &Clip, moves: &[Move], join: usize) -> Vec<Clip> {
    let min = MIN_LEN as i64;
    let lead = (moves.iter())
        .find(|m| (m.at - clip.start).abs() <= min)
        .map_or(0, |m| m.by);
    let cuts: Vec<&Move> = (moves.iter())
        .filter(|m| m.at > clip.start + min && m.at < clip.end() - min)
        .collect();
    let mut pieces: Vec<(Clip, i64)> = Vec::new();
    let mut rest = clip.clone();
    let mut by = lead;
    for m in cuts {
        if let Some((a, b)) = rest.split(m.at) {
            pieces.push((a, by));
            rest = b;
            by = m.by;
        }
    }
    pieces.push((rest, by));

    let mut out: Vec<Clip> = Vec::with_capacity(pieces.len());
    for (mut p, by) in pieces {
        let by = by.max(-p.start);
        p.start += by;
        if let Some(a) = out.last_mut() {
            // Never past the piece before's start.
            if p.start < a.start + 2 * min {
                p.start = a.start + 2 * min;
            }
            if p.start < a.end() {
                a.trim_end(p.start);
            }
            // Drawn back over the gap and the crossfade, with the audio
            // before its note: no further back than the piece before
            // starts in the file, so no earlier note plays twice.
            let lo = p.start - (p.offset as i64 - a.offset as i64) + min;
            let want = (a.end() - join as i64).max(lo);
            if want < p.start {
                p.trim_start(want);
            }
            let overlap = (a.end() - p.start).max(0) as usize;
            if overlap > 0 {
                a.fade_out = Fade::new(overlap.min(a.len - a.fade_in.len), Curve::EqualPower);
                p.fade_in = Fade::new(overlap.min(p.len - p.fade_out.len), Curve::EqualPower);
            }
        }
        out.push(p);
    }
    out
}

/// What fixing the timing of some clips would do, for showing first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Clips to change by track position, and the moves for each.
    pub clips: Vec<(usize, u64, Vec<Move>)>,
    /// Notes found, and how many will move.
    pub notes: usize,
    /// The largest and average distance moved, in frames.
    pub most: i64,
    pub average: i64,
    /// Comp pieces left alone.
    pub skipped_comp: usize,
}

/// The settings in the Fix timing window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub grid: Grid,
    /// 0 leaves notes where they are; 1 puts them on the grid.
    pub strength: f32,
    pub sensitivity: f32,
    /// The track to find notes on, by position; `None` for all of a
    /// group's tracks together.
    pub guide: Option<usize>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            grid: Grid::Part(2),
            strength: 1.0,
            sensitivity: 0.6,
            guide: None,
        }
    }
}

/// The units fixed together: each active edit group's tracks, or one
/// track on its own, with the clips in `clips` on them.
fn units(groups: &[LaneGroup], clips: &[(usize, &Clip)]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    for &(lane, _) in clips {
        let unit = groups::group_of(groups, lane).map_or(vec![lane], |g| g.lanes.clone());
        if !out.contains(&unit) {
            out.push(unit);
        }
    }
    out
}

/// The plan for `chosen` (clips by track) with their grouped partners
/// already in, given where notes start in each clip (timeline frames).
pub fn plan(
    map: &TempoMap,
    groups: &[LaneGroup],
    chosen: &[(usize, &Clip)],
    notes_of: &dyn Fn(&Clip) -> Vec<i64>,
    s: Settings,
) -> Plan {
    let mut p = Plan::default();
    let (mut sum, mut count) = (0, 0);
    let (fix, comp): (Vec<_>, Vec<_>) = chosen.iter().partition(|(_, c)| !c.comp);
    p.skipped_comp = comp.len();
    for unit in units(groups, &fix) {
        let here: Vec<&(usize, &Clip)> = fix.iter().filter(|(l, _)| unit.contains(l)).collect();
        // The guide track if it's in this unit and has clips here, or
        // every track of it.
        let guided = s.guide.filter(|g| here.iter().any(|(l, _)| l == g));
        let lists: Vec<Vec<i64>> = (here.iter())
            .filter(|(l, c)| guided.is_none_or(|g| *l == g) && !c.muted)
            .map(|(_, c)| notes_of(c))
            .collect();
        let found = merge(map.sample_rate(), &lists);
        let ms = moves(map, &found, s.grid, s.strength);
        p.notes += found.len();
        let moved: Vec<i64> = ms.iter().map(|m| m.by.abs()).filter(|&b| b > 0).collect();
        p.most = p.most.max(moved.iter().copied().max().unwrap_or(0));
        sum += moved.iter().sum::<i64>();
        count += moved.len() as i64;
        for (l, c) in here {
            let mine: Vec<Move> = (ms.iter())
                .filter(|m| m.at >= c.start - MIN_LEN as i64 && m.at < c.end())
                .copied()
                .collect();
            if mine.iter().any(|m| m.by != 0) {
                p.clips.push((*l, c.id, mine));
            }
        }
    }
    p.average = sum / count.max(1);
    p
}

/// The Fix timing window, and the notes found so far.
#[derive(Default)]
pub(crate) struct Timing {
    pub open: bool,
    pub settings: Settings,
    /// Where notes start in a file at a sensitivity, in its frames.
    found: HashMap<(PathBuf, u32), Vec<usize>>,
    pub message: Option<String>,
}

impl Timing {
    /// Where notes start in `c`, in timeline frames, reading its file
    /// the first time.
    fn notes_of(&mut self, c: &Clip) -> Result<Vec<i64>, String> {
        let key = (
            c.source.path.clone(),
            (self.settings.sensitivity * 1000.0) as u32,
        );
        if !self.found.contains_key(&key) {
            let samples = c.source.read_mono(c.source.frames)?;
            let a = attacks(&samples, c.source.rate, self.settings.sensitivity);
            self.found.insert(key.clone(), a);
        }
        let end = c.offset + c.len;
        Ok(self.found[&key]
            .iter()
            .filter(|&&s| s >= c.offset && s < end)
            .map(|&s| c.start + (s - c.offset) as i64)
            .collect())
    }
}

impl App {
    /// The groups that move together: none while group editing is off.
    fn timing_groups(&self) -> Vec<LaneGroup> {
        if self.timeline.group_edit {
            self.lane_groups()
        } else {
            Vec::new()
        }
    }

    /// The selected clips and those grouped with them, by track.
    fn timing_clips(&mut self) -> Vec<(usize, u64)> {
        let groups = self.timing_groups();
        let selected = self.timeline.selected.clone();
        let ids = self.with_song(|_, song| groups::with_partners(song, &groups, selected));
        (self.tracks.iter().enumerate())
            .flat_map(|(l, t)| t.clips.iter().map(move |c| (l, c.id)))
            .filter(|(_, id)| ids.contains(id))
            .collect()
    }

    /// What Fix timing would do to the selection now.
    fn timing_plan(&mut self) -> Plan {
        let map = self.tempo_map();
        let groups = self.timing_groups();
        let chosen = self.timing_clips();
        // Find the notes first: reading files needs the window's cache.
        let mut notes: HashMap<u64, Vec<i64>> = HashMap::new();
        self.timing.message = None;
        for &(l, id) in &chosen {
            let Some(c) = self.tracks[l].clips.iter().find(|c| c.id == id) else {
                continue;
            };
            if c.comp {
                continue;
            }
            match self.timing.notes_of(c) {
                Ok(n) => {
                    notes.insert(id, n);
                }
                Err(e) => self.timing.message = Some(e),
            }
        }
        let clips: Vec<(usize, &Clip)> = (chosen.iter())
            .filter_map(|&(l, id)| {
                self.tracks[l]
                    .clips
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| (l, c))
            })
            .collect();
        let lookup = |c: &Clip| notes.get(&c.id).cloned().unwrap_or_default();
        plan(&map, &groups, &clips, &lookup, self.timing.settings)
    }

    /// Cuts and moves the clips as `p` says, selecting the pieces.
    fn apply_timing(&mut self, p: &Plan) {
        let join = frames(self.rate, JOIN_SECONDS);
        let mut pieces = HashSet::new();
        for (l, id, ms) in &p.clips {
            let clips = &mut self.tracks[*l].clips;
            let Some(i) = clips.iter().position(|c| c.id == *id) else {
                continue;
            };
            let new = fix_clip(&clips[i], ms, join);
            pieces.extend(new.iter().map(|c| c.id));
            clips.splice(i..=i, new);
        }
        self.timeline.selected = pieces;
        self.rebuild();
    }

    pub(crate) fn timing_window(&mut self, ctx: &egui::Context) {
        if !self.timing.open {
            return;
        }
        let may_edit = self.may_edit_song();
        let chosen = self.timing_clips();
        let lanes: Vec<usize> = {
            let mut l: Vec<usize> = chosen.iter().map(|(l, _)| *l).collect();
            l.dedup();
            l
        };
        let names: Vec<(usize, String)> = (lanes.iter())
            .map(|&l| (l, self.tracks[l].name.clone()))
            .collect();
        if self
            .timing
            .settings
            .guide
            .is_some_and(|g| !lanes.contains(&g))
        {
            self.timing.settings.guide = None;
        }
        let plan = if chosen.is_empty() {
            Plan::default()
        } else {
            self.timing_plan()
        };
        let map = self.tempo_map();
        let rate = self.rate;
        let ms = |f: i64| (f as f64 * 1000.0 / rate as f64).round() as i64;
        let mut open = true;
        let mut fix = false;
        let t = &mut self.timing;
        egui::Window::new("Fix timing")
            .open(&mut open)
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    "Finds where each note starts in the selected clips, cuts there, and \
                     moves the notes toward the grid. Grouped tracks move together.",
                );
                if chosen.is_empty() {
                    ui.label(RichText::new("Select a clip to fix first.").weak());
                    return;
                }
                let s = &mut t.settings;
                egui::Grid::new("timing-settings")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Move notes to");
                        egui::ComboBox::from_id_salt("timing-grid")
                            .selected_text(s.grid.label())
                            .show_ui(ui, |ui| {
                                for g in Grid::ALL.into_iter().filter(|g| *g != Grid::Auto) {
                                    ui.selectable_value(&mut s.grid, g, g.label());
                                }
                            })
                            .response
                            .accessible_name("Grid to move notes to");
                        ui.end_row();
                        ui.label("Strength");
                        ui.add(
                            egui::Slider::new(&mut s.strength, 0.0..=1.0)
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                                .custom_parser(|t| {
                                    t.trim_end_matches('%')
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .map(|v| v / 100.0)
                                }),
                        )
                        .on_hover_text("How far toward the grid: 100% puts notes right on it")
                        .accessible_name("Strength");
                        ui.end_row();
                        ui.label("Sensitivity");
                        ui.add(
                            egui::Slider::new(&mut s.sensitivity, 0.0..=1.0)
                                .step_by(0.05)
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                        )
                        .on_hover_text("Higher finds softer notes too")
                        .accessible_name("Sensitivity");
                        ui.end_row();
                        if names.len() > 1 {
                            ui.label("Find notes on");
                            let shown = (s.guide)
                                .and_then(|g| names.iter().find(|(l, _)| *l == g))
                                .map_or("All the tracks together", |(_, n)| n.as_str());
                            egui::ComboBox::from_id_salt("timing-guide")
                                .selected_text(shown)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut s.guide,
                                        None,
                                        "All the tracks together",
                                    );
                                    for (l, n) in &names {
                                        ui.selectable_value(&mut s.guide, Some(*l), n);
                                    }
                                })
                                .response
                                .accessible_name("Track to find notes on");
                            ui.end_row();
                        }
                    });
                preview(ui, &map, &plan, s.grid);
                let moving: usize = (plan.clips.iter())
                    .map(|(_, _, m)| m.iter().filter(|m| m.by != 0).count())
                    .max()
                    .unwrap_or(0);
                ui.label(format!(
                    "Found {} notes. {} clips change; notes move by up to {} ms, {} ms on \
                     average.",
                    plan.notes,
                    plan.clips.len(),
                    ms(plan.most),
                    ms(plan.average),
                ));
                if plan.skipped_comp > 0 {
                    ui.label(
                        RichText::new(
                            "Comp pieces are left alone: flatten the comp first to fix them.",
                        )
                        .weak(),
                    );
                }
                if let Some(m) = &t.message {
                    ui.colored_label(ui.visuals().warn_fg_color, m);
                }
                ui.add_enabled_ui(may_edit && moving > 0, |ui| {
                    if ui
                        .button("Fix timing")
                        .on_hover_text("Cut at each note and move it. Undo puts it back")
                        .clicked()
                    {
                        fix = true;
                    }
                });
                if !may_edit {
                    ui.label(
                        RichText::new("You can comment on this project, not change it.").weak(),
                    );
                }
            });
        self.timing.open = open;
        if fix {
            self.apply_timing(&plan);
        }
    }
}

/// The selection's notes against the grid: a tick where each starts, a
/// line where it goes.
fn preview(ui: &mut egui::Ui, map: &TempoMap, plan: &Plan, grid: Grid) {
    let all: Vec<Move> = {
        let mut m: Vec<Move> = plan
            .clips
            .iter()
            .flat_map(|(_, _, m)| m.iter().copied())
            .collect();
        m.sort_by_key(|m| m.at);
        m.dedup();
        m
    };
    let (Some(first), Some(last)) = (all.first(), all.last()) else {
        return;
    };
    let pad = map.sample_rate() as i64 / 4;
    let (a, b) = (
        first.at.min(first.at + first.by) - pad,
        last.at.max(last.at + last.by) + pad,
    );
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let v = ui.visuals();
    painter.rect_filled(rect, 2.0, v.extreme_bg_color);
    let x = |f: i64| rect.left() + (f - a) as f32 / (b - a).max(1) as f32 * rect.width();
    // Grid lines, unless too close to see.
    let mut g = timeline::snap(map, a.max(0), grid);
    let mut n = 0;
    while g < b && n < 400 {
        if g >= a {
            painter.line_segment(
                [pos2(x(g), rect.top()), pos2(x(g), rect.bottom())],
                Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
            );
        }
        g = timeline::step(map, g, 1, grid);
        n += 1;
    }
    let from = v.warn_fg_color;
    let to = v.selection.stroke.color;
    for m in &all {
        let (x0, x1) = (x(m.at), x(m.at + m.by));
        painter.line_segment(
            [pos2(x0, rect.top() + 6.0), pos2(x0, rect.bottom() - 6.0)],
            Stroke::new(1.5, from),
        );
        if m.by != 0 {
            let y = rect.center().y;
            painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.0, to));
            painter.circle_filled(pos2(x1, y), 2.5, to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::time::{Tempo, TimeSignature};

    const RATE: u32 = 48_000;

    /// Plucked notes starting at `at` (frames), each a decaying tone.
    fn notes(len: usize, at: &[usize], level: &[f32]) -> Vec<f32> {
        let mut s = vec![0.0f32; len];
        for (k, &a) in at.iter().enumerate() {
            for (i, x) in s[a..].iter_mut().enumerate() {
                let t = i as f32 / RATE as f32;
                *x += level[k] * (2.0 * std::f32::consts::PI * 196.0 * t).sin() * (-t / 0.12).exp();
            }
        }
        s
    }

    #[test]
    fn finds_each_note_within_a_millisecond_of_its_start() {
        let at = [4_800, 30_000, 52_000, 81_000];
        let s = notes(100_000, &at, &[0.5, 0.4, 0.6, 0.5]);
        let found = attacks(&s, RATE, 0.5);
        assert_eq!(found.len(), at.len(), "{found:?}");
        for (f, a) in found.iter().zip(at) {
            assert!((*f as i64 - a as i64).abs() <= 48, "{f} for {a}");
        }
    }

    #[test]
    fn silence_and_a_steady_tone_have_no_new_notes() {
        assert!(attacks(&vec![0.0; 48_000], RATE, 1.0).is_empty());
        let tone: Vec<f32> = (0..96_000)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 82.0 * i as f32 / RATE as f32).sin())
            .collect();
        // Only where it begins.
        assert_eq!(attacks(&tone, RATE, 1.0).len(), 1);
    }

    #[test]
    fn soft_notes_need_more_sensitivity() {
        // A soft note over the ring of a loud one.
        let s = notes(60_000, &[1_000, 20_000], &[0.8, 0.12]);
        assert_eq!(attacks(&s, RATE, 0.0).len(), 1);
        assert_eq!(attacks(&s, RATE, 1.0).len(), 2);
    }

    fn map() -> TempoMap {
        // 120 BPM: a beat is 24,000 frames, half a beat 12,000.
        TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON)
    }

    #[test]
    fn notes_move_toward_the_grid_by_the_strength() {
        let m = map();
        let at = [13_000, 22_000, 37_000];
        let full = moves(&m, &at, Grid::Part(2), 1.0);
        assert_eq!(
            full,
            vec![
                Move {
                    at: 13_000,
                    by: -1_000
                },
                Move {
                    at: 22_000,
                    by: 2_000
                },
                Move {
                    at: 37_000,
                    by: -1_000
                },
            ]
        );
        let half = moves(&m, &at, Grid::Part(2), 0.5);
        assert_eq!(
            half[1],
            Move {
                at: 22_000,
                by: 1_000
            }
        );
        assert!(moves(&m, &at, Grid::Part(2), 0.0).iter().all(|m| m.by == 0));
    }

    #[test]
    fn two_notes_heading_for_one_line_are_not_cut_apart() {
        // A flam: both nearest 24,000.
        let ms = moves(&map(), &[23_000, 25_000], Grid::Part(2), 1.0);
        assert_eq!(
            ms,
            vec![Move {
                at: 23_000,
                by: 1_000
            }]
        );
    }

    #[test]
    fn notes_found_on_several_tracks_merge_when_close() {
        let m = merge(RATE, &[vec![1_000, 30_000], vec![1_300, 60_000]]);
        assert_eq!(m, vec![1_000, 30_000, 60_000]);
    }

    #[test]
    fn a_fixed_clip_plays_its_notes_on_the_grid_without_doubling_any() {
        let c = timeline::tests::clip(0, 48_000);
        let join = frames(RATE, JOIN_SECONDS);
        let ms = [
            Move {
                at: 13_000,
                by: -1_000,
            },
            Move {
                at: 22_000,
                by: 2_000,
            },
        ];
        let out = fix_clip(&c, &ms, join);
        assert_eq!(out.len(), 3);
        // Each note's audio now plays at its grid line.
        let at = |p: &Clip, src: usize| p.start + (src as i64 - p.offset as i64);
        assert_eq!(at(&out[1], 13_000), 12_000);
        assert_eq!(at(&out[2], 22_000), 24_000);
        for w in out.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            // They meet with a short crossfade and no gap.
            assert_eq!(a.end() - b.start, join as i64);
            assert_eq!((a.fade_out.len, b.fade_in.len), (join, join));
        }
        // No piece plays past where the next note starts in the file,
        // so no note is heard twice, and no piece reaches back before
        // the note before it.
        assert!(out[0].offset + out[0].len <= 13_000);
        assert!(out[1].offset + out[1].len <= 22_000);
        assert!(out[2].offset >= 13_000);
        // The outer ends are where they were.
        assert_eq!(out[0].start, 0);
        assert_eq!(out[2].end(), 48_000 + 2_000);
        // Nothing moved, nothing cut.
        assert_eq!(fix_clip(&c, &[], join).len(), 1);
    }

    #[test]
    fn grouped_tracks_are_cut_and_moved_alike_from_one_guide() {
        let m = map();
        let kick = timeline::tests::clip(0, 48_000);
        let room = timeline::tests::clip(0, 48_000);
        let groups = vec![LaneGroup {
            name: "Drums".into(),
            lanes: vec![0, 1],
        }];
        let (kid, rid) = (kick.id, room.id);
        let chosen = vec![(0, &kick), (1, &room)];
        // Only the kick mic hears a clear note at 13,000; the room mic
        // hears it late and smeared.
        let notes = |c: &Clip| {
            if c.id == kid {
                vec![13_000]
            } else {
                vec![13_400, 30_000]
            }
        };
        let s = Settings {
            guide: Some(0),
            ..Settings::default()
        };
        let p = plan(&m, &groups, &chosen, &notes, s);
        assert_eq!(p.notes, 1);
        let mv = vec![Move {
            at: 13_000,
            by: -1_000,
        }];
        assert_eq!(p.clips, vec![(0, kid, mv.clone()), (1, rid, mv)]);
        // With no guide, both tracks' notes count.
        let p = plan(&m, &groups, &chosen, &notes, Settings::default());
        assert_eq!(p.notes, 2);
    }

    #[test]
    fn comp_pieces_are_left_alone() {
        let mut c = timeline::tests::clip(0, 48_000);
        c.comp = true;
        let p = plan(
            &map(),
            &[],
            &[(0, &c)],
            &|_| vec![13_000],
            Settings::default(),
        );
        assert!(p.clips.is_empty());
        assert_eq!(p.skipped_comp, 1);
    }

    /// A file of `notes` as a clip at the timeline's start.
    pub(crate) fn take(len: usize, at: &[usize]) -> Clip {
        let dir = std::env::temp_dir().join(format!("daw-timing-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.wav", timeline::next_id()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for x in notes(len, at, &vec![0.5; at.len()]) {
            w.write_sample(x).unwrap();
        }
        w.finalize().unwrap();
        let src = daw_engine::stream::StreamSource::open(&path).unwrap();
        Clip::new(src, path, 0, Some(0))
    }

    #[test]
    fn a_sloppy_take_plays_on_the_grid_once_fixed() {
        // Eighths at 120 BPM are 12,000 frames apart: played up to 40 ms
        // early or late.
        let played = [12_000 + 1_900, 24_000 - 1_500, 36_000 + 700, 48_000 - 1_900];
        let mut t = Timing::default();
        let c = take(60_000, &played);
        let found = t.notes_of(&c).unwrap();
        assert_eq!(found.len(), 4, "{found:?}");
        let ms = moves(&map(), &found, Grid::Part(2), 1.0);
        let out = fix_clip(&c, &ms, frames(RATE, JOIN_SECONDS));
        assert_eq!(out.len(), 5);
        for (k, p) in out[1..].iter().enumerate() {
            // Where the note's first sample now plays.
            let heard = p.start + (played[k] as i64 - p.offset as i64);
            let grid = 12_000 * (k as i64 + 1);
            assert!(
                (heard - grid).abs() <= 48,
                "note {k} at {heard}, not {grid}"
            );
        }
    }

    mod in_the_app {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;

        use super::*;
        use crate::commands::Cmd;
        use crate::{App, Track};

        #[test]
        fn fixing_a_take_cuts_it_at_its_notes_and_undo_puts_it_back() {
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let c = take(60_000, &[13_900, 22_500, 36_700, 46_100]);
            let id = c.id;
            let ctx = h.ctx.clone();
            let app = h.state_mut();
            assert_eq!(app.rate, RATE);
            app.tracks.push(Track::new("Guitar".into()));
            app.tracks[0].clips.push(c);
            app.timeline.selected.insert(id);
            app.rebuild();
            app.run(&ctx, Cmd::FixTiming);
            h.run();
            assert!(h.query_all_by_label("Strength").next().is_some());
            h.state_mut().sync_doc(&ctx, true);
            h.get_by_role_and_label(egui::accesskit::Role::Button, "Fix timing")
                .click();
            h.run();
            let app = h.state();
            assert_eq!(app.tracks[0].clips.len(), 5);
            assert_eq!(app.timeline.selected.len(), 5);
            assert_eq!(app.project().tracks[0].clips.len(), 5);
            h.state_mut().sync_doc(&ctx, true);
            h.state_mut().undo(&ctx);
            h.run();
            assert_eq!(h.state().tracks[0].clips.len(), 1);
        }
    }
}
