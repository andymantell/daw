// SPDX-License-Identifier: GPL-3.0-or-later
//! Timecode and hit points (roadmap 3.10): a timecode ruler and position
//! beside bars and beats, hit points on picture frames ("door slams"),
//! and a tempo fit tool that finds a tempo, or a tempo change, that lands
//! chosen hit points on beats or bars and says how close each one gets.
//!
//! Hit points are placed in time, not in bars, so they stay with the
//! picture when the tempo changes; the music moves to meet them. Each
//! one sits on the start of a picture frame.

use std::collections::HashMap;

use daw_engine::engine::Command;
use daw_engine::time::{Frames, TempoMap, Ticks};
use daw_engine::timecode::{Clock, Rate, Timecode};
use daw_project::{HitPoint, TempoChange, TimecodeSetup};
use eframe::egui::{
    self, Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Shape, Stroke, pos2, vec2,
};

use crate::App;
use crate::tempo::Changes;
use crate::theme::Look;

/// The line under the toolbar that holds the timecode and hit points.
pub(crate) const STRIP: f32 = 30.0;

/// The tempos the fit tool offers, at most.
const FITS: usize = 6;

/// The project's timecode setup as a clock.
pub(crate) fn clock(s: &TimecodeSetup) -> Clock {
    let rate = Rate::from_id(&s.rate).unwrap_or_default();
    let start = Timecode::parse(rate, &s.start).map_or(0, |t| t.label(rate));
    Clock {
        rate,
        start,
        offset: s.offset,
    }
}

/// A new hit point's name: "Hit 1", "Hit 2", … the first not taken.
fn hit_name(hits: &[HitPoint]) -> String {
    (1..)
        .map(|n| format!("Hit {n}"))
        .find(|n| !hits.iter().any(|h| &h.name == n))
        .expect("an unused name")
}

/// Adds a hit point at the start of the frame containing `at`, keeping
/// them in time order. Its id, or `None` if one is on that frame.
pub(crate) fn add_hit(
    hits: &mut Vec<HitPoint>,
    clock: &Clock,
    rate: u32,
    at: i64,
) -> Option<String> {
    let at = clock.snap(at.max(0), rate).max(0);
    if hits.iter().any(|h| h.at == at) {
        return None;
    }
    let id = daw_project::new_id();
    let h = HitPoint {
        id: id.clone(),
        at,
        name: hit_name(hits),
    };
    let i = hits.partition_point(|o| o.at <= at);
    hits.insert(i, h);
    Some(id)
}

/// Moves hit point `id` to the frame containing `at`. True if it moved.
pub(crate) fn move_hit(hits: &mut [HitPoint], clock: &Clock, rate: u32, id: &str, at: i64) -> bool {
    let at = clock.snap(at.max(0), rate).max(0);
    match hits.iter_mut().find(|h| h.id == id) {
        Some(h) if h.at != at => {
            h.at = at;
            true
        }
        _ => false,
    }
}

/// Hit points are moments in the picture: when the picture's start moves
/// on the timeline they move with it, and at another frame rate they go
/// on the start of their new frame.
pub(crate) fn follow_picture(
    was: &TimecodeSetup,
    now: &TimecodeSetup,
    hits: &mut [HitPoint],
    rate: u32,
) {
    let by = now.offset - was.offset;
    if by == 0 && was.rate == now.rate {
        return;
    }
    let c = clock(now);
    for h in hits.iter_mut() {
        h.at = c.snap((h.at + by).max(0), rate);
    }
}

/// What the timeline asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum HitAsk {
    /// A hit point here.
    Add(i64),
    /// Hit point `id` dragged to here.
    Move(String, i64),
    /// The window.
    Open,
}

// ---- The tempo fit tool ----

/// What a chosen hit point should land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Land {
    Beat,
    Bar,
}

impl Land {
    fn label(self) -> &'static str {
        match self {
            Land::Beat => "a beat",
            Land::Bar => "a bar",
        }
    }
}

/// What the fit may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum How {
    /// The song's tempo, from the start.
    Song,
    /// A tempo change at the start of this bar; the bars before it stay.
    FromBar(i64),
}

/// The song's time as it is, which a fit starts from.
pub(crate) struct Time<'a> {
    pub rate: u32,
    pub bpm: f64,
    pub beats: u32,
    pub changes: &'a Changes,
}

impl Time<'_> {
    /// The map with the tempo `bpm` put in as `how` says.
    fn with(&self, how: How, bpm: f64) -> TempoMap {
        match how {
            How::Song => crate::tempo::map(self.rate, bpm, self.beats, self.changes),
            How::FromBar(bar) => {
                let mut c = self.changes.clone();
                c.tempos.retain(|t| (t.bar, t.beat.max(1)) != (bar, 1));
                c.tempos.push(TempoChange {
                    id: String::new(),
                    bar,
                    beat: 1,
                    bpm,
                    glide: false,
                });
                crate::tempo::map(self.rate, self.bpm, self.beats, &c)
            }
        }
    }

    fn map(&self) -> TempoMap {
        crate::tempo::map(self.rate, self.bpm, self.beats, self.changes)
    }
}

/// Where a hit point lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Landing {
    /// The beat or bar line nearest to it.
    pub bar: i64,
    pub beat: i64,
    /// How far the hit point is after that line, in frames (negative
    /// before it).
    pub miss: i64,
    /// The fit moved it: false for one before the bar a tempo change
    /// starts on.
    pub moved: bool,
}

/// The nearest beat or bar line to `at`, and how far `at` is from it.
pub(crate) fn landing(map: &TempoMap, at: i64, land: Land) -> Landing {
    let t = map.ticks_at(Frames(at));
    let bb = map.bar_beat(t);
    let bar_start = map.bar_start(bb.bar);
    let (a, b) = match land {
        Land::Bar => (bar_start, map.bar_start(bb.bar + 1)),
        Land::Beat => {
            let beat = map.signature_at(bar_start).beat_ticks();
            let a = Ticks(bar_start.0 + (bb.beat - 1) * beat);
            (a, Ticks(a.0 + beat))
        }
    };
    let (fa, fb) = (map.frames_at(a).0, map.frames_at(b).0);
    let line = if at - fa <= fb - at { a } else { b };
    let bb = map.bar_beat(line);
    Landing {
        bar: bb.bar,
        beat: bb.beat,
        miss: at - map.frames_at(line).0,
        moved: true,
    }
}

/// A tempo the fit found, and where each chosen hit point lands with it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fit {
    pub bpm: f64,
    pub landings: Vec<Landing>,
    /// The biggest miss of the hit points it moves, in frames.
    pub worst: i64,
}

/// Finds the tempos from `lo` to `hi` that land `targets` (positions in
/// frames, and what each should land on) nearest their beats or bars,
/// best first. Tempos are tried a hundredth of a BPM apart (or whole BPM
/// only), and the best few are then tried to a thousandth. Ties go to
/// the tempo nearest the one already there.
pub(crate) fn fit(
    time: &Time,
    how: How,
    targets: &[(i64, Land)],
    (lo, hi): (f64, f64),
    whole: bool,
) -> Vec<Fit> {
    let (lo, hi) = (lo.clamp(10.0, 999.0), hi.clamp(10.0, 999.0));
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    // A hit point before the change's bar doesn't move.
    let from = match how {
        How::Song => 0,
        How::FromBar(bar) => {
            let m = time.map();
            m.frames_at(m.bar_start(bar)).0
        }
    };
    let now = match how {
        How::Song => time.bpm,
        How::FromBar(bar) => {
            let m = time.map();
            m.tempo_at(m.bar_start(bar)).as_f64()
        }
    };
    let moving: Vec<&(i64, Land)> = targets.iter().filter(|(at, _)| *at >= from).collect();
    if moving.is_empty() {
        return Vec::new();
    }
    let worst = |bpm: f64| {
        let m = time.with(how, bpm);
        (moving.iter())
            .map(|(at, land)| landing(&m, *at, *land).miss.abs())
            .max()
            .unwrap_or(0)
    };
    let step = if whole { 1.0 } else { 0.01 };
    let first = (lo / step).ceil() as i64;
    let last = (hi / step).floor() as i64;
    let tried: Vec<(f64, i64)> = (first..=last)
        .map(|i| {
            let bpm = (i as f64 * step * 1000.0).round() / 1000.0;
            (bpm, worst(bpm))
        })
        .collect();
    // Each valley's lowest point.
    let mut best: Vec<(f64, i64)> = Vec::new();
    for (i, &(bpm, w)) in tried.iter().enumerate() {
        let left = i.checked_sub(1).map_or(i64::MAX, |j| tried[j].1);
        let right = tried.get(i + 1).map_or(i64::MAX, |t| t.1);
        if w <= left && w <= right {
            best.push((bpm, w));
        }
    }
    // A millisecond is as close as anyone needs; nearer the tempo there
    // is better after that.
    let ms = (time.rate as i64 / 1000).max(1);
    let key = |&(bpm, w): &(f64, i64)| (w / ms, ((bpm - now).abs() * 1000.0) as i64);
    best.sort_by_key(key);
    let mut chosen: Vec<(f64, i64)> = Vec::new();
    for b in best {
        if chosen.len() == FITS {
            break;
        }
        if chosen.iter().all(|c| (c.0 - b.0).abs() >= 0.5) {
            chosen.push(b);
        }
    }
    if !whole {
        for c in &mut chosen {
            for k in -10..=10 {
                let bpm = ((c.0 * 1000.0).round() + k as f64) / 1000.0;
                if (lo..=hi).contains(&bpm) {
                    let w = worst(bpm);
                    if w < c.1 {
                        *c = (bpm, w);
                    }
                }
            }
        }
        chosen.sort_by_key(key);
    }
    chosen
        .into_iter()
        .map(|(bpm, worst)| {
            let m = time.with(how, bpm);
            let landings = (targets.iter())
                .map(|&(at, land)| Landing {
                    moved: at >= from,
                    ..landing(&m, at, land)
                })
                .collect();
            Fit {
                bpm,
                landings,
                worst,
            }
        })
        .collect()
}

/// A miss in milliseconds and picture frames, such as "3 ms late (0.1
/// frames)".
fn miss_text(miss: i64, rate: u32, fps: f64) -> String {
    let ms = miss as f64 * 1000.0 / rate as f64;
    let frames = miss.abs() as f64 * fps / rate as f64;
    if ms.abs() < 0.5 {
        return "on it".into();
    }
    let side = if miss > 0 { "late" } else { "early" };
    format!("{:.0} ms {side} ({frames:.2} frames)", ms.abs())
}

/// The biggest miss, either way.
fn worst_text(worst: i64, rate: u32, fps: f64) -> String {
    let ms = worst as f64 * 1000.0 / rate as f64;
    if ms < 0.5 {
        return "none, all on".into();
    }
    let frames = worst as f64 * fps / rate as f64;
    format!("{ms:.0} ms ({frames:.2} frames)")
}

fn bpm_text(bpm: f64) -> String {
    let s = format!("{bpm:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ---- The window ----

/// The Timecode and hit points window, and the fit tool's choices.
pub(crate) struct HitPointsWindow {
    pub(crate) open: bool,
    /// What each chosen hit point should land on, by id. Not saved: a
    /// fit is a one-off.
    lands: HashMap<String, Land>,
    /// Fit a tempo change from a bar, not the song's tempo.
    from_bar: bool,
    bar: i64,
    range: (f64, f64),
    whole: bool,
    /// The last fit, and what it was for, so it can be shown until the
    /// hit points or choices change.
    fits: Option<(How, Vec<String>, Vec<Fit>)>,
    /// Timecode being typed, by hit point id ("" for the start).
    typing: HashMap<String, String>,
}

impl Default for HitPointsWindow {
    fn default() -> HitPointsWindow {
        HitPointsWindow {
            open: false,
            lands: HashMap::new(),
            from_bar: false,
            bar: 2,
            range: (60.0, 180.0),
            whole: false,
            fits: None,
            typing: HashMap::new(),
        }
    }
}

/// What the window asked the app to do.
#[derive(Default)]
struct Asks {
    seek: Option<i64>,
    /// Use this tempo, as found.
    tempo: Option<(How, f64)>,
    add: bool,
    changed: bool,
}

/// A timecode field: shows `tc`, and gives back what was typed once it
/// reads as timecode and the field is left or Enter pressed. Red while
/// it doesn't read.
fn tc_field(
    ui: &mut egui::Ui,
    typing: &mut HashMap<String, String>,
    key: &str,
    rate: Rate,
    tc: Timecode,
    name: &str,
) -> Option<Timecode> {
    let mut text = typing.get(key).cloned().unwrap_or_else(|| tc.to_string());
    let parsed = Timecode::parse(rate, &text);
    let mut edit = egui::TextEdit::singleline(&mut text)
        .desired_width(96.0)
        .font(egui::TextStyle::Monospace);
    if parsed.is_none() {
        edit = edit.text_color(crate::theme::look(ui.ctx()).c().error);
    }
    let r = ui.add(edit).accessible_name(name);
    if r.changed() {
        typing.insert(key.to_string(), text.clone());
    }
    if r.lost_focus() {
        typing.remove(key);
        return Timecode::parse(rate, &text).filter(|t| *t != tc);
    }
    None
}

impl HitPointsWindow {
    #[allow(clippy::too_many_arguments)]
    fn window(
        &mut self,
        ctx: &egui::Context,
        setup: &mut TimecodeSetup,
        hits: &mut Vec<HitPoint>,
        time: &Time,
        playhead: i64,
        may_edit: bool,
    ) -> Asks {
        let mut asks = Asks::default();
        if !self.open {
            return asks;
        }
        let rate = time.rate;
        let mut open = self.open;
        egui::Window::new("Timecode and hit points")
            .open(&mut open)
            .default_pos([360.0, 140.0])
            .default_width(560.0)
            .default_height(660.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.add_enabled_ui(may_edit, |ui| {
                    ui.scope(|ui| {
                        let was = setup.clone();
                        self.timecode_part(ui, setup, rate, playhead, &mut asks);
                        follow_picture(&was, setup, hits, rate);
                        ui.separator();
                        self.hits_part(ui, setup, hits, time, &mut asks);
                        ui.separator();
                        self.fit_part(ui, setup, hits, time, &mut asks);
                    });
                });
            });
        self.open = open;
        asks
    }

    fn timecode_part(
        &mut self,
        ui: &mut egui::Ui,
        setup: &mut TimecodeSetup,
        rate: u32,
        playhead: i64,
        asks: &mut Asks,
    ) {
        ui.heading("Timecode");
        asks.changed |= ui
            .checkbox(
                &mut setup.shown,
                "Show timecode on the ruler and in the transport bar",
            )
            .changed();
        let c = clock(setup);
        ui.horizontal(|ui| {
            ui.label("Frame rate");
            egui::ComboBox::from_id_salt("timecode rate")
                .selected_text(format!("{} fps", c.rate.label()))
                .show_ui(ui, |ui| {
                    for r in Rate::ALL {
                        if ui
                            .selectable_label(r == c.rate, format!("{} fps", r.label()))
                            .clicked()
                        {
                            // The start keeps its hours, minutes and
                            // seconds, written the new rate's way.
                            let start = Timecode::from_label(c.rate, c.start);
                            let fixed = Timecode {
                                frames: start.frames.min(r.nominal() - 1),
                                drop_frame: r.drop_frame(),
                                ..start
                            };
                            let fixed = Timecode::parse(r, &fixed.to_string())
                                .unwrap_or(Timecode { frames: 2, ..fixed });
                            setup.rate = r.id().into();
                            setup.start = fixed.to_string();
                            asks.changed = true;
                        }
                    }
                })
                .response
                .accessible_name("Frame rate");
        });
        ui.horizontal(|ui| {
            ui.label("Start timecode");
            let start = Timecode::from_label(c.rate, c.start);
            if let Some(t) = tc_field(ui, &mut self.typing, "", c.rate, start, "Start timecode") {
                setup.start = t.to_string();
                asks.changed = true;
            }
            ui.label("at");
            let mut secs = setup.offset as f64 / rate as f64;
            if ui
                .add(
                    egui::DragValue::new(&mut secs)
                        .speed(0.01)
                        .range(-36_000.0..=36_000.0)
                        .max_decimals(3)
                        .suffix(" s"),
                )
                .accessible_name("Where the start timecode falls")
                .changed()
            {
                setup.offset = (secs * rate as f64).round() as i64;
                asks.changed = true;
            }
            if ui
                .button("At the playhead")
                .on_hover_text("Put the start timecode where the playhead is")
                .clicked()
            {
                setup.offset = playhead;
                asks.changed = true;
            }
        });
        ui.label(
            RichText::new(format!(
                "The playhead is at {}. Line these up with the video's own timecode so the \
                 times match the edit.",
                c.timecode(playhead, rate)
            ))
            .weak(),
        );
    }

    fn hits_part(
        &mut self,
        ui: &mut egui::Ui,
        setup: &TimecodeSetup,
        hits: &mut Vec<HitPoint>,
        time: &Time,
        asks: &mut Asks,
    ) {
        let rate = time.rate;
        let c = clock(setup);
        let map = time.map();
        ui.heading("Hit points");
        ui.label(
            "Moments in the picture to write to. They stay put in time when the tempo changes; \
             drag them on the ruler, or type a timecode.",
        );
        if hits.is_empty() {
            ui.label(RichText::new("None yet.").weak());
        }
        let mut remove = None;
        let mut moved = None;
        for (i, h) in hits.iter_mut().enumerate() {
            let n = i + 1;
            ui.horizontal(|ui| {
                asks.changed |= ui
                    .add(egui::TextEdit::singleline(&mut h.name).desired_width(140.0))
                    .accessible_name(format!("Hit point {n} name"))
                    .changed();
                let tc = c.timecode(h.at, rate);
                if let Some(t) = tc_field(
                    ui,
                    &mut self.typing,
                    &h.id,
                    c.rate,
                    tc,
                    &format!("Hit point {n} timecode"),
                ) && let Some(at) = c.position(t, rate)
                {
                    moved = Some((h.id.clone(), at));
                }
                let frame = c.frame_at(h.at, rate);
                if ui
                    .small_button("◀")
                    .on_hover_text("A frame earlier")
                    .accessible_name(format!("Hit point {n} a frame earlier"))
                    .clicked()
                {
                    moved = Some((h.id.clone(), c.frame_start(frame - 1, rate)));
                }
                if ui
                    .small_button("▶")
                    .on_hover_text("A frame later")
                    .accessible_name(format!("Hit point {n} a frame later"))
                    .clicked()
                {
                    moved = Some((h.id.clone(), c.frame_start(frame + 1, rate)));
                }
                ui.label(
                    RichText::new(map.bar_beat(map.ticks_at(Frames(h.at))).to_string()).weak(),
                );
                let mut land = self.lands.get(&h.id).copied();
                egui::ComboBox::from_id_salt(("land", &h.id))
                    .width(80.0)
                    .selected_text(land.map_or("Not fitted", Land::label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut land, None, "Not fitted");
                        ui.selectable_value(&mut land, Some(Land::Beat), Land::Beat.label());
                        ui.selectable_value(&mut land, Some(Land::Bar), Land::Bar.label());
                    })
                    .response
                    .on_hover_text("For the tempo fit: what this hit point should land on")
                    .accessible_name(format!("Hit point {n} lands on"));
                match land {
                    Some(l) => self.lands.insert(h.id.clone(), l),
                    None => self.lands.remove(&h.id),
                };
                if ui
                    .small_button("Go")
                    .on_hover_text("Move the playhead here")
                    .accessible_name(format!("Go to hit point {n}"))
                    .clicked()
                {
                    asks.seek = Some(h.at);
                }
                if ui
                    .small_button("Delete")
                    .accessible_name(format!("Delete hit point {n}"))
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some((id, at)) = moved {
            asks.changed |= move_hit(hits, &c, rate, &id, at);
            hits.sort_by_key(|h| h.at);
        }
        if let Some(i) = remove {
            let h = hits.remove(i);
            self.lands.remove(&h.id);
            asks.changed = true;
        }
        if ui.button("Add hit point at the playhead").clicked() {
            asks.add = true;
        }
    }

    fn fit_part(
        &mut self,
        ui: &mut egui::Ui,
        setup: &TimecodeSetup,
        hits: &[HitPoint],
        time: &Time,
        asks: &mut Asks,
    ) {
        ui.heading("Tempo fit");
        ui.label(
            "Finds a tempo that lands the hit points you choose (\"lands on\" above) on beats \
             or bars, and shows how close each gets.",
        );
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.from_bar, false, "The song's tempo");
            ui.radio_value(&mut self.from_bar, true, "A tempo change from bar");
            ui.add_enabled(
                self.from_bar,
                egui::DragValue::new(&mut self.bar).range(2..=9999),
            )
            .accessible_name("Tempo change bar");
        });
        ui.horizontal(|ui| {
            ui.label("Between");
            ui.add(
                egui::DragValue::new(&mut self.range.0)
                    .range(20.0..=300.0)
                    .suffix(" BPM"),
            )
            .accessible_name("Slowest tempo");
            ui.label("and");
            ui.add(
                egui::DragValue::new(&mut self.range.1)
                    .range(20.0..=300.0)
                    .suffix(" BPM"),
            )
            .accessible_name("Fastest tempo");
            ui.checkbox(&mut self.whole, "Whole BPM only");
        });
        let how = if self.from_bar {
            How::FromBar(self.bar.max(2))
        } else {
            How::Song
        };
        let chosen: Vec<&HitPoint> = (hits.iter())
            .filter(|h| self.lands.contains_key(&h.id))
            .collect();
        let ids: Vec<String> = chosen.iter().map(|h| h.id.clone()).collect();
        if chosen.is_empty() {
            ui.label(RichText::new("Choose what at least one hit point lands on.").weak());
        }
        if ui
            .add_enabled(!chosen.is_empty(), egui::Button::new("Find tempos"))
            .clicked()
        {
            let targets: Vec<(i64, Land)> =
                (chosen.iter()).map(|h| (h.at, self.lands[&h.id])).collect();
            let fits = fit(time, how, &targets, self.range, self.whole);
            self.fits = Some((how, ids.clone(), fits));
        }
        let Some((fit_how, fit_ids, fits)) = &self.fits else {
            return;
        };
        if *fit_how != how || *fit_ids != ids {
            ui.label(RichText::new("The choices changed: find tempos again.").weak());
            return;
        }
        if fits.is_empty() {
            ui.label("None of the chosen hit points is after that bar, so a change there can't move them.");
            return;
        }
        let fps = clock(setup).rate.as_f64();
        for (k, f) in fits.iter().enumerate() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong(format!("{} BPM", bpm_text(f.bpm)));
                ui.label(format!(
                    "furthest off: {}",
                    worst_text(f.worst, time.rate, fps)
                ));
                let what = match how {
                    How::Song => "Use as the song's tempo".to_string(),
                    How::FromBar(b) => format!("Add as a tempo change at bar {b}"),
                };
                if ui
                    .button(what)
                    .accessible_name(format!("Use tempo {}", k + 1))
                    .clicked()
                {
                    asks.tempo = Some((how, f.bpm));
                }
            });
            for (h, l) in chosen.iter().zip(&f.landings) {
                let on = if l.beat == 1 {
                    format!("bar {}", l.bar)
                } else {
                    format!("bar {} beat {}", l.bar, l.beat)
                };
                let text = if l.moved {
                    format!(
                        "    {}: {on}, {}",
                        h.name,
                        miss_text(l.miss, time.rate, fps)
                    )
                } else {
                    format!("    {}: before the change, stays at {on}", h.name)
                };
                ui.label(RichText::new(text).small());
            }
        }
    }
}

// ---- On the timeline ----

/// Draws the strip under the toolbar: timecode along it if `clock` is
/// set, and the hit points as diamonds with their names. `r` is the
/// part in line with the lanes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_strip(
    painter: &Painter,
    r: Rect,
    clock: Option<&Clock>,
    rate: u32,
    hits: &[HitPoint],
    x_of: &dyn Fn(i64) -> f32,
    frame_of: &dyn Fn(f32) -> i64,
    look: &Look,
) {
    let c = look.c();
    painter.rect_filled(r, 0.0, c.panel);
    painter.line_segment(
        [r.left_bottom(), r.right_bottom()],
        Stroke::new(look.line, c.line),
    );
    let painter = painter.with_clip_rect(r);
    let font = FontId::new(look.small, look.numerals_font().family);
    if let Some(clock) = clock {
        let ppf = x_of(clock.frame_start(1, rate)) - x_of(clock.frame_start(0, rate));
        let n = clock.rate.nominal();
        let step = [
            1,
            2,
            5,
            10,
            n,
            2 * n,
            5 * n,
            10 * n,
            30 * n,
            60 * n,
            300 * n,
            600 * n,
            3600 * n,
        ]
        .into_iter()
        .find(|&s| s as f32 * ppf >= 90.0)
        .unwrap_or(3600 * n);
        // Lines on whole steps of the picture's own count.
        let base = clock.rate.index_of(clock.start).unwrap_or(0);
        let first = base + clock.frame_at(frame_of(r.left()).max(0), rate);
        let mut index = first.div_euclid(step) * step;
        for _ in 0..400 {
            let at = clock.frame_start(index - base, rate);
            let x = x_of(at);
            if x > r.right() {
                break;
            }
            if x >= r.left() - 80.0 {
                painter.line_segment(
                    [pos2(x, r.bottom() - 6.0), pos2(x, r.bottom())],
                    Stroke::new(look.line, c.text_dim),
                );
                painter.text(
                    pos2(x + 3.0, r.bottom() - 2.0),
                    Align2::LEFT_BOTTOM,
                    clock.timecode(at, rate).to_string(),
                    font.clone(),
                    c.text_dim,
                );
            }
            index += step;
        }
    }
    for h in hits {
        let x = x_of(h.at);
        if x < r.left() - 120.0 || x > r.right() + 6.0 {
            continue;
        }
        let y = r.top() + 8.0;
        painter.add(diamond(pos2(x, y), 5.0, c.accent));
        let g = painter.layout_no_wrap(h.name.clone(), look.font(look.small), c.text);
        let lr = Rect::from_min_size(
            pos2(x + 7.0, y - g.size().y / 2.0),
            g.size() + vec2(4.0, 0.0),
        );
        painter.rect_filled(lr, 2.0, c.panel.gamma_multiply(0.9));
        painter.galley(lr.min + vec2(2.0, 0.0), g, c.text);
    }
}

/// A diamond centred on `p`: hit points' mark, unlike markers' flags.
pub(crate) fn diamond(p: Pos2, r: f32, fill: Color32) -> Shape {
    Shape::convex_polygon(
        vec![
            pos2(p.x, p.y - r),
            pos2(p.x + r, p.y),
            pos2(p.x, p.y + r),
            pos2(p.x - r, p.y),
        ],
        fill,
        Stroke::NONE,
    )
}

/// The hit point whose diamond is under `pos`, if any.
pub(crate) fn hit_at(hits: &[HitPoint], x_of: &dyn Fn(i64) -> f32, pos: Pos2) -> Option<usize> {
    hits.iter()
        .enumerate()
        .map(|(i, h)| (i, (x_of(h.at) - pos.x).abs()))
        .filter(|(_, d)| *d <= 6.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

// ---- In the app ----

impl App {
    /// The clock, if timecode is shown.
    pub(crate) fn shown_clock(&self) -> Option<Clock> {
        self.timecode.shown.then(|| clock(&self.timecode))
    }

    pub(crate) fn hit_points_window(&mut self, ctx: &egui::Context) {
        let playhead = self.ctl.status().position().0;
        let may_edit = self.may_edit_song();
        let time = Time {
            rate: self.rate,
            bpm: self.tempo_bpm,
            beats: self.beats_per_bar,
            changes: &self.time,
        };
        let asks = self.hits_ui.window(
            ctx,
            &mut self.timecode,
            &mut self.hit_points,
            &time,
            playhead,
            may_edit,
        );
        if let Some(f) = asks.seek {
            self.send(Command::Seek(Frames(f)));
        }
        if asks.add {
            self.add_hit_point(playhead);
        }
        if let Some((how, bpm)) = asks.tempo {
            self.use_tempo(how, bpm);
        }
    }

    /// A hit point at `at`, on the start of its frame.
    pub(crate) fn add_hit_point(&mut self, at: i64) {
        if !self.may_edit_song() {
            return;
        }
        let c = clock(&self.timecode);
        add_hit(&mut self.hit_points, &c, self.rate, at);
    }

    /// The timeline asked for a hit point, or the window.
    pub(crate) fn ask_hit(&mut self, ask: HitAsk) {
        match ask {
            HitAsk::Add(at) => self.add_hit_point(at),
            HitAsk::Move(id, at) => {
                if self.may_edit_song() {
                    let c = clock(&self.timecode);
                    move_hit(&mut self.hit_points, &c, self.rate, &id, at);
                    self.hit_points.sort_by_key(|h| h.at);
                }
            }
            HitAsk::Open => self.hits_ui.open = true,
        }
    }

    /// Puts in a tempo the fit found.
    fn use_tempo(&mut self, how: How, bpm: f64) {
        if !self.may_edit_song() {
            return;
        }
        match how {
            How::Song => self.tempo_bpm = bpm,
            How::FromBar(bar) => {
                self.time
                    .tempos
                    .retain(|t| (t.bar, t.beat.max(1)) != (bar, 1));
                self.time.tempos.push(TempoChange {
                    id: daw_project::new_id(),
                    bar,
                    beat: 1,
                    bpm,
                    glide: false,
                });
                self.time.tempos.sort_by_key(|t| (t.bar, t.beat));
            }
        }
        self.time_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn secs(s: f64) -> i64 {
        (s * RATE as f64).round() as i64
    }

    #[test]
    fn hit_points_sit_on_frames_in_time_order() {
        let c = Clock {
            rate: Rate::Fps25,
            ..Clock::default()
        };
        let mut hits = Vec::new();
        // 1920 samples a frame at 25 fps.
        add_hit(&mut hits, &c, RATE, 10_000).unwrap();
        add_hit(&mut hits, &c, RATE, 3_000).unwrap();
        assert!(add_hit(&mut hits, &c, RATE, 9_700).is_none(), "same frame");
        assert_eq!(hits.iter().map(|h| h.at).collect::<Vec<_>>(), [1920, 9600]);
        assert_eq!(hits[1].name, "Hit 1");
        assert_eq!(hits[0].name, "Hit 2");
        let id = hits[0].id.clone();
        assert!(move_hit(&mut hits, &c, RATE, &id, 5_000));
        assert_eq!(hits[0].at, 3840);
        assert!(!move_hit(&mut hits, &c, RATE, &id, 4_000), "same frame");
    }

    #[test]
    fn hit_points_follow_the_picture() {
        let was = TimecodeSetup::default();
        let c = clock(&was);
        let mut hits = Vec::new();
        add_hit(&mut hits, &c, RATE, secs(1.0)).unwrap();
        // The picture starts half a second later: so does the hit.
        let later = TimecodeSetup {
            offset: secs(0.5),
            ..was.clone()
        };
        follow_picture(&was, &later, &mut hits, RATE);
        assert_eq!(hits[0].at, secs(1.5));
        // At 23.976 fps it goes on the start of the frame it is in.
        let film = TimecodeSetup {
            rate: "23.976".into(),
            ..later.clone()
        };
        follow_picture(&later, &film, &mut hits, RATE);
        let fc = clock(&film);
        assert_eq!(hits[0].at, fc.snap(secs(1.5), RATE));
        assert_eq!(fc.snap(hits[0].at, RATE), hits[0].at);
        assert!(hits[0].at <= secs(1.5) && hits[0].at > secs(1.5) - 2002);
    }

    #[test]
    fn landing_finds_the_nearest_beat_or_bar() {
        let changes = Changes::default();
        let t = Time {
            rate: RATE,
            bpm: 120.0,
            beats: 4,
            changes: &changes,
        };
        let m = t.map();
        // Half a second a beat. 2.6 s is bar 2 beat 2, 0.1 s late.
        let l = landing(&m, secs(2.6), Land::Beat);
        assert_eq!((l.bar, l.beat, l.miss), (2, 2, secs(0.1)));
        // 2.9 s is nearer beat 3, 0.1 s early.
        let l = landing(&m, secs(2.9), Land::Beat);
        assert_eq!((l.bar, l.beat, l.miss), (2, 3, -secs(0.1)));
        // As bars (2 s each): 2.9 s is 0.9 s after bar 2; 3.1 s is 0.9
        // before bar 3.
        let l = landing(&m, secs(2.9), Land::Bar);
        assert_eq!((l.bar, l.beat, l.miss), (2, 1, secs(0.9)));
        let l = landing(&m, secs(3.1), Land::Bar);
        assert_eq!((l.bar, l.beat, l.miss), (3, 1, -secs(0.9)));
    }

    #[test]
    fn a_fit_lands_hit_points_on_bars() {
        let changes = Changes::default();
        let t = Time {
            rate: RATE,
            bpm: 120.0,
            beats: 4,
            changes: &changes,
        };
        // Hits 8 s and 16 s in: at 120 BPM bars 5 and 9 exactly, and at
        // 90 BPM (bars of 2.667 s) bars 4 and 7.
        let targets = [(secs(8.0), Land::Bar), (secs(16.0), Land::Bar)];
        let fits = fit(&t, How::Song, &targets, (60.0, 180.0), false);
        assert!(!fits.is_empty());
        let best = &fits[0];
        // The nearest exact fit to the tempo there is now.
        assert_eq!(best.bpm, 120.0);
        assert_eq!(best.worst, 0);
        assert_eq!(
            best.landings.iter().map(|l| l.bar).collect::<Vec<_>>(),
            [5, 9]
        );
        assert!(fits.iter().any(|f| f.bpm == 90.0 && f.worst == 0));
        // Sorted, best first.
        let ms = RATE as i64 / 1000;
        assert!(fits.windows(2).all(|w| w[0].worst / ms <= w[1].worst / ms));
    }

    #[test]
    fn an_awkward_hit_gets_a_tempo_to_a_thousandth() {
        let changes = Changes::default();
        let t = Time {
            rate: RATE,
            bpm: 100.0,
            beats: 4,
            changes: &changes,
        };
        // 7.37 s on a beat: beat n at 60n/bpm, so bpm = 60n/7.37, e.g.
        // n = 12 gives 97.693 BPM.
        let targets = [(secs(7.37), Land::Beat)];
        let fits = fit(&t, How::Song, &targets, (95.0, 99.0), false);
        let best = &fits[0];
        assert!((best.bpm - 97.693).abs() < 0.0015, "{}", best.bpm);
        // Well under a millisecond.
        assert!(best.worst < RATE as i64 / 1000, "{}", best.worst);
        // Whole BPM only is coarser.
        let whole = fit(&t, How::Song, &targets, (95.0, 99.0), true);
        assert_eq!(whole[0].bpm.fract(), 0.0);
        assert!(whole[0].worst >= best.worst);
    }

    #[test]
    fn a_tempo_change_leaves_the_bars_before_it() {
        let changes = Changes::default();
        let t = Time {
            rate: RATE,
            bpm: 120.0,
            beats: 4,
            changes: &changes,
        };
        // Bar 3 starts at 4 s. A hit at 3 s is before it; one at 9 s
        // should land on a bar after it: 5 s for two bars is 96 BPM.
        let targets = [(secs(3.0), Land::Beat), (secs(9.0), Land::Bar)];
        let fits = fit(&t, How::FromBar(3), &targets, (90.0, 100.0), false);
        let best = &fits[0];
        assert!((best.bpm - 96.0).abs() < 0.001, "{}", best.bpm);
        assert!(!best.landings[0].moved);
        assert!(best.landings[1].moved);
        assert_eq!(best.landings[1].bar, 5);
        assert!(best.landings[1].miss.abs() <= 1);
        // A change after every chosen hit can't move any.
        assert!(fit(&t, How::FromBar(9), &targets, (90.0, 100.0), false).is_empty());
    }

    #[test]
    fn misses_read_in_milliseconds_and_frames() {
        assert_eq!(miss_text(20, RATE, 25.0), "on it");
        assert_eq!(miss_text(480, RATE, 25.0), "10 ms late (0.25 frames)");
        assert_eq!(miss_text(-960, RATE, 25.0), "20 ms early (0.50 frames)");
        assert_eq!(worst_text(960, RATE, 25.0), "20 ms (0.50 frames)");
        assert_eq!(bpm_text(120.0), "120");
        assert_eq!(bpm_text(97.6930), "97.693");
        assert_eq!(bpm_text(96.5), "96.5");
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
    fn a_fit_found_in_the_window_sets_the_tempo_and_is_saved() {
        use egui_kittest::kittest::Queryable;
        let mut h = app();
        let rate = h.state().rate;
        let at = |s: f64| (s * rate as f64).round() as i64;
        {
            let a = h.state_mut();
            a.timecode.shown = true;
            a.add_hit_point(at(8.0));
            a.add_hit_point(at(16.0));
            let ids: Vec<String> = a.hit_points.iter().map(|h| h.id.clone()).collect();
            for id in ids {
                a.hits_ui.lands.insert(id, Land::Bar);
            }
            a.tempo_bpm = 100.0;
            a.hits_ui.open = true;
        }
        h.run();
        h.get_by_label("Find tempos").click();
        h.run();
        // Bars land on 8 s and 16 s at 90 or 120; 90 is nearer 100.
        h.get_by_label("Use tempo 1").scroll_to_me();
        h.run();
        h.get_by_label("Use tempo 1").click();
        h.run();
        h.run();
        let a = h.state();
        assert_eq!(a.tempo_bpm, 90.0);
        let p = a.proj.doc.get();
        assert_eq!(p.tempo_bpm, 90.0);
        assert_eq!(p.hit_points.len(), 2);
        assert!(p.timecode.shown);
        // The hit points didn't move: they stay with the picture.
        assert_eq!(p.hit_points[0].at, at(8.0));
    }
}
