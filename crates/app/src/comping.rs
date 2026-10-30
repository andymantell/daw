// SPDX-License-Identifier: GPL-3.0-or-later
//! Take lanes and comping (roadmap 2.2). Each pass of a loop recording
//! is a take of its own, and the Take lanes pane shows a track's takes
//! one above another, in line with the timeline. Dragging across a take
//! makes it play there: the best bits of each take, put together, are
//! the comp. Where two parts of a comp meet they crossfade by
//! themselves. A track can keep several comps of the same takes and
//! switch between them, and flatten the one it keeps into plain clips.
//!
//! The comp that plays is made of ordinary clips cut from the takes and
//! marked [`Clip::comp`], so the mix, export and freezing need nothing
//! new, and the pieces can be edited on the timeline like other clips.
//! The takes stay whole, muted under the pieces. Each comp keeps its
//! parts (which take, from where to where) so another can be chosen;
//! the one playing reads its parts back from its pieces, so trimming,
//! moving or deleting a piece on the timeline changes the comp.

use daw_engine::fade::{Curve, Fade};
use daw_project::{Comp, CompPart};
use eframe::egui::{self, Align2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::timeline::{self, Clip, HEADER, MIN_LEN};

/// How long the crossfade is where two parts of a comp meet.
pub const COMP_CROSSFADE_SECONDS: f64 = timeline::CROSSFADE_SECONDS;

/// The fade at a cut end of a piece where no other part meets it, so it
/// doesn't click.
pub const EDGE_FADE_SECONDS: f64 = 0.005;

fn frames(rate: u32, secs: f64) -> usize {
    (rate as f64 * secs).round() as usize
}

/// In time order, with touching parts of the same take joined.
fn tidy(mut parts: Vec<CompPart>) -> Vec<CompPart> {
    parts.retain(|p| p.end > p.start);
    parts.sort_by_key(|p| (p.start, p.end));
    let mut out: Vec<CompPart> = Vec::with_capacity(parts.len());
    for p in parts {
        match out.last_mut() {
            Some(l) if l.take == p.take && p.start <= l.end => l.end = l.end.max(p.end),
            _ => out.push(p),
        }
    }
    out
}

/// The parts with `[start, end)` taken out.
fn cut(parts: &[CompPart], start: i64, end: i64) -> Vec<CompPart> {
    let mut out = Vec::new();
    for &p in parts {
        if p.end <= start || p.start >= end {
            out.push(p);
            continue;
        }
        if p.start < start {
            out.push(CompPart { end: start, ..p });
        }
        if p.end > end {
            out.push(CompPart { start: end, ..p });
        }
    }
    out
}

/// Take `take` plays in `[start, end)`, over whatever played there.
pub fn swipe(parts: &mut Vec<CompPart>, take: u32, start: i64, end: i64) {
    if end <= start {
        return;
    }
    let mut out = cut(parts, start, end);
    out.push(CompPart { take, start, end });
    *parts = tidy(out);
}

/// Nothing plays in `[start, end)`.
pub fn clear(parts: &mut Vec<CompPart>, start: i64, end: i64) {
    *parts = tidy(cut(parts, start, end));
}

/// The part playing at frame `at`.
pub fn part_at(parts: &[CompPart], at: i64) -> Option<CompPart> {
    parts.iter().copied().find(|p| p.start <= at && at < p.end)
}

/// The track's takes (not the comp's pieces), by number, in order.
pub fn takes(clips: &[Clip]) -> Vec<usize> {
    let mut t: Vec<usize> = (clips.iter())
        .filter(|c| !c.comp)
        .filter_map(|c| c.take)
        .collect();
    t.sort_unstable();
    t.dedup();
    t
}

/// Where each take plays now, without a comp: the takes not muted,
/// later clips over earlier ones, as the mix has them.
pub fn as_playing(clips: &[Clip]) -> Vec<CompPart> {
    let mut parts = Vec::new();
    for c in clips.iter().filter(|c| !c.comp && !c.muted) {
        if let Some(t) = c.take {
            swipe(&mut parts, t as u32, c.start, c.end());
        }
    }
    parts
}

/// Every take where it was recorded, the newest on top: the comp a
/// fresh one starts as.
pub fn newest_on_top(clips: &[Clip]) -> Vec<CompPart> {
    let mut all: Vec<&Clip> = clips
        .iter()
        .filter(|c| !c.comp && c.take.is_some())
        .collect();
    all.sort_by_key(|c| c.take);
    let mut parts = Vec::new();
    for c in all {
        swipe(&mut parts, c.take.unwrap_or(0) as u32, c.start, c.end());
    }
    parts
}

/// The comp the pieces on the track make: each piece from its start to
/// its end, with a crossfaded join put back at the middle of the
/// overlap.
pub fn parts_of(clips: &[Clip]) -> Vec<CompPart> {
    let mut pieces: Vec<&Clip> = clips
        .iter()
        .filter(|c| c.comp && c.take.is_some())
        .collect();
    pieces.sort_by_key(|c| (c.start, c.id));
    let mut parts = Vec::new();
    for (i, c) in pieces.iter().enumerate() {
        let (mut s, mut e) = (c.start, c.end());
        if let Some(p) = i.checked_sub(1).map(|j| pieces[j])
            && p.end() > c.start
            && p.end() < c.end()
            && p.start < c.start
        {
            s = (c.start + p.end()) / 2;
        }
        if let Some(n) = pieces.get(i + 1)
            && n.start < c.end()
            && n.start > c.start
            && n.end() > c.end()
        {
            e = (n.start + c.end()) / 2;
        }
        swipe(&mut parts, c.take.unwrap_or(0) as u32, s, e);
    }
    parts
}

/// Makes the comp play: the takes are muted, and pieces of them play
/// where `parts` say, crossfading where they meet. Pieces made before
/// go.
pub fn apply(clips: &mut Vec<Clip>, parts: &[CompPart], rate: u32) {
    clips.retain(|c| !c.comp);
    for c in clips.iter_mut().filter(|c| c.take.is_some()) {
        c.muted = true;
    }
    let edge = frames(rate, EDGE_FADE_SECONDS);
    let half = (frames(rate, COMP_CROSSFADE_SECONDS) / 2) as i64;
    let mut pieces: Vec<Clip> = Vec::new();
    for p in parts {
        for c in clips.iter().filter(|c| c.take == Some(p.take as usize)) {
            let (s, e) = (c.start.max(p.start), c.end().min(p.end));
            if e - s < MIN_LEN as i64 {
                continue;
            }
            let mut piece = c.clone();
            piece.id = timeline::next_id();
            piece.key = daw_project::new_id();
            piece.comp = true;
            piece.muted = false;
            if s > c.start {
                piece.trim_start(s);
                piece.fade_in = Fade::new(0, Curve::EqualPower);
                piece.set_fade(false, edge);
            }
            if e < c.end() {
                piece.trim_end(e);
                piece.fade_out = Fade::new(0, Curve::EqualPower);
                piece.set_fade(true, edge);
            }
            pieces.push(piece);
        }
    }
    pieces.sort_by_key(|c| c.start);
    // Where two meet, each reaches into the other by the same amount (as
    // far as both have audio), so the join stays where it was.
    for i in 1..pieces.len() {
        let (before, after) = pieces.split_at_mut(i);
        let (a, b) = (&mut before[i - 1], &mut after[0]);
        if b.start != a.end() {
            continue;
        }
        let room_a = (a.source.frames - a.offset - a.len) as i64;
        let room_b = (b.offset as i64).min(b.start);
        let reach = half.min(room_a).min(room_b);
        if reach <= 0 {
            continue;
        }
        let join = a.end();
        a.trim_end(join + reach);
        b.trim_start(join - reach);
        let overlap = (2 * reach) as usize;
        a.fade_out.curve = Curve::EqualPower;
        b.fade_in.curve = Curve::EqualPower;
        a.set_fade(true, overlap);
        b.set_fade(false, overlap);
    }
    clips.extend(pieces);
}

/// A track's takes and comps, to change together.
pub struct Takes<'a> {
    pub clips: &'a mut Vec<Clip>,
    pub comps: &'a mut Vec<Comp>,
    /// The comp that plays, by id.
    pub comp: &'a mut Option<String>,
    pub rate: u32,
}

impl Takes<'_> {
    /// The comp that plays.
    pub fn active(&self) -> Option<usize> {
        let id = self.comp.as_ref()?;
        self.comps.iter().position(|c| &c.id == id)
    }

    /// The parts that play now, comped or not.
    pub fn parts(&self) -> Vec<CompPart> {
        match self.active() {
            Some(_) => parts_of(self.clips),
            None => as_playing(self.clips),
        }
    }

    /// Reads the playing comp's parts back from its pieces, which may
    /// have been edited on the timeline.
    pub fn sync(&mut self) {
        if let Some(i) = self.active() {
            self.comps[i].parts = parts_of(self.clips);
        }
    }

    fn play(&mut self, i: usize) {
        *self.comp = Some(self.comps[i].id.clone());
        apply(self.clips, &self.comps[i].parts, self.rate);
    }

    /// The first "Comp n" not taken.
    fn next_name(&self) -> String {
        (1..)
            .map(|n| format!("Comp {n}"))
            .find(|name| !self.comps.iter().any(|c| &c.name == name))
            .expect("there is always a free number")
    }

    /// Adds a comp of `parts` and plays it. Returns its index.
    fn add(&mut self, parts: Vec<CompPart>) -> usize {
        let name = self.next_name();
        self.comps.push(Comp {
            id: daw_project::new_id(),
            name,
            parts,
        });
        let i = self.comps.len() - 1;
        self.play(i);
        i
    }

    /// A new comp: of what plays now if no comp does, or else of every
    /// take, the newest on top.
    pub fn new_comp(&mut self) -> usize {
        self.sync();
        let parts = match self.active() {
            Some(_) => newest_on_top(self.clips),
            None => as_playing(self.clips),
        };
        self.add(parts)
    }

    /// A copy of the comp that plays, to try changes on.
    pub fn duplicate(&mut self) -> Option<usize> {
        self.sync();
        let i = self.active()?;
        let parts = self.comps[i].parts.clone();
        Some(self.add(parts))
    }

    /// Plays comp `which`, or the takes as recorded (the newest on top)
    /// for `None`.
    pub fn choose(&mut self, which: Option<usize>) {
        self.sync();
        match which {
            Some(i) if i < self.comps.len() => self.play(i),
            _ => {
                let newest = timeline::take_count(self.clips).checked_sub(1);
                self.choose_take(newest);
            }
        }
    }

    /// Plays one take where it was recorded, as the take list does,
    /// leaving the comp (which is kept).
    pub fn choose_take(&mut self, take: Option<usize>) {
        self.sync();
        *self.comp = None;
        self.clips.retain(|c| !c.comp);
        timeline::choose_take(self.clips, take);
    }

    pub fn rename(&mut self, i: usize, name: &str) {
        let name = name.trim();
        if let Some(c) = self.comps.get_mut(i)
            && !name.is_empty()
        {
            c.name = name.to_string();
        }
    }

    /// Keeps the comp that plays as ordinary clips and lets the takes
    /// and comps go, for a tidy track. Undo brings them back.
    pub fn flatten(&mut self) -> bool {
        self.sync();
        let Some(i) = self.active() else {
            return false;
        };
        let name = self.comps[i].name.clone();
        self.clips.retain(|c| c.comp || c.take.is_none());
        for c in self.clips.iter_mut().filter(|c| c.comp) {
            c.label = format!("{name}, take {}", c.take.map_or(0, |t| t + 1));
            c.take = None;
            c.comp = false;
            c.muted = false;
        }
        self.comps.clear();
        *self.comp = None;
        true
    }

    pub fn delete(&mut self, i: usize) {
        if i >= self.comps.len() {
            return;
        }
        let playing = self.active() == Some(i);
        self.comps.remove(i);
        if playing {
            *self.comp = None;
            self.choose(None);
        }
    }

    /// Take `take` plays in `[start, end)`, starting a comp of what
    /// plays now if none does.
    pub fn swipe(&mut self, take: usize, start: i64, end: i64) {
        self.sync();
        let i = match self.active() {
            Some(i) => i,
            None => self.add(as_playing(self.clips)),
        };
        swipe(&mut self.comps[i].parts, take as u32, start, end);
        self.play(i);
    }

    /// Nothing plays in `[start, end)` of the comp.
    pub fn clear(&mut self, start: i64, end: i64) {
        self.sync();
        if let Some(i) = self.active() {
            clear(&mut self.comps[i].parts, start, end);
            self.play(i);
        }
    }

    /// Take `take` plays in the part at `at`: the part of the comp (or
    /// of what plays) there, or else the whole of the take's clip there.
    pub fn use_at(&mut self, take: usize, at: i64) -> bool {
        let range = part_at(&self.parts(), at)
            .map(|p| (p.start, p.end))
            .or_else(|| {
                (self.clips.iter())
                    .find(|c| !c.comp && c.take == Some(take) && c.start <= at && at < c.end())
                    .map(|c| (c.start, c.end()))
            });
        match range {
            Some((s, e)) => {
                self.swipe(take, s, e);
                true
            }
            None => false,
        }
    }

    /// Adds a take just recorded, on top. On a comped track it goes into
    /// the comp over the time it covers, as it would play without one.
    pub fn add_take(&mut self, clip: Clip) {
        self.sync();
        let (start, end) = (clip.start, clip.end());
        timeline::add_take(self.clips, clip);
        if let Some(i) = self.active() {
            let take = self.clips.last().and_then(|c| c.take).unwrap_or(0);
            swipe(&mut self.comps[i].parts, take as u32, start, end);
            self.play(i);
        }
    }

    /// After takes are taken off the track: where the comp played one,
    /// the newest take left there plays instead.
    pub fn takes_gone(&mut self) {
        let Some(i) = self.active() else {
            return;
        };
        let left = takes(self.clips);
        let base = newest_on_top(self.clips);
        let mut parts = Vec::new();
        // As last played: the pieces of the takes gone may be gone too.
        for p in std::mem::take(&mut self.comps[i].parts) {
            if left.contains(&(p.take as usize)) {
                parts.push(p);
                continue;
            }
            for b in &base {
                let (s, e) = (b.start.max(p.start), b.end.min(p.end));
                if s < e {
                    parts.push(CompPart {
                        take: b.take,
                        start: s,
                        end: e,
                    });
                }
            }
        }
        self.comps[i].parts = tidy(parts);
        self.play(i);
    }
}

impl crate::Track {
    pub(crate) fn takes(&mut self, rate: u32) -> Takes<'_> {
        Takes {
            clips: &mut self.clips,
            comps: &mut self.comps,
            comp: &mut self.comp,
            rate,
        }
    }

    /// The name of the comp that plays, if one does.
    pub(crate) fn comp_name(&self) -> Option<&str> {
        let id = self.comp.as_ref()?;
        self.comps
            .iter()
            .find(|c| &c.id == id)
            .map(|c| c.name.as_str())
    }
}

/// Height of each lane in the pane.
const ROW: f32 = 46.0;

/// The Take lanes pane's own state.
#[derive(Default)]
pub(crate) struct TakesPane {
    /// The track shown, by key.
    track: Option<String>,
    /// The track of the timeline's selection when last looked, so a new
    /// selection brings its track up.
    followed: Option<String>,
    /// A drag across a take: its number, and where it started and is
    /// now, in frames.
    swiping: Option<(usize, i64, i64)>,
    /// Where the comp row was right-clicked, in frames.
    menu_at: Option<i64>,
    /// The comp being renamed, and the name so far.
    renaming: Option<(usize, String)>,
}

/// What the pane asks of the app.
enum Ask {
    Swipe(usize, i64, i64),
    UseAt(usize, i64),
    Clear(i64, i64),
    Choose(Option<usize>),
    New,
    Duplicate,
    Delete(usize),
    Flatten,
    Rename(usize, String),
}

impl crate::App {
    /// The Take lanes pane: the takes of one track, one lane each, to
    /// comp from.
    pub(crate) fn takes_pane(&mut self, ui: &mut egui::Ui) {
        let rate = self.rate;
        let map = self.tempo_map();
        let playhead = self.ctl.status().position().0;
        // The track of the timeline's selection, when it changes.
        let selected = (self.tracks.iter())
            .find(|t| {
                t.clips
                    .iter()
                    .any(|c| self.timeline.selected.contains(&c.id))
            })
            .map(|t| t.key.clone());
        let pane = &mut self.takes_ui;
        if selected.is_some() && selected != pane.followed {
            pane.track = selected.clone();
        }
        pane.followed = selected;
        if !(pane.track.as_ref()).is_some_and(|k| self.tracks.iter().any(|t| &t.key == k)) {
            pane.track = (self.tracks.iter())
                .find(|t| !takes(&t.clips).is_empty())
                .or(self.tracks.first())
                .map(|t| t.key.clone());
        }
        let Some(ti) =
            (pane.track.as_ref()).and_then(|k| self.tracks.iter().position(|t| &t.key == k))
        else {
            ui.label("Add a track and record on it to see its takes here.");
            return;
        };

        let mut ask = None;
        ui.horizontal(|ui| {
            let mut pick = ti;
            egui::ComboBox::from_id_salt("takes-track")
                .selected_text(&self.tracks[ti].name)
                .show_ui(ui, |ui| {
                    for (i, t) in self.tracks.iter().enumerate() {
                        ui.selectable_value(&mut pick, i, &t.name);
                    }
                })
                .response
                .accessible_name("Track whose takes are shown");
            if pick != ti {
                self.takes_ui.track = Some(self.tracks[pick].key.clone());
            }
            let t = &self.tracks[ti];
            let now = t.comps.iter().position(|c| Some(&c.id) == t.comp.as_ref());
            let mut choice = now;
            egui::ComboBox::from_id_salt("takes-comp")
                .selected_text(t.comp_name().unwrap_or("As recorded"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, "As recorded")
                        .on_hover_text("No comp: the newest take plays");
                    for (i, c) in t.comps.iter().enumerate() {
                        ui.selectable_value(&mut choice, Some(i), &c.name);
                    }
                })
                .response
                .accessible_name("Comp that plays");
            if choice != now {
                ask = Some(Ask::Choose(choice));
            }
            let has_takes = !takes(&t.clips).is_empty();
            if ui
                .add_enabled(has_takes, egui::Button::new("New comp"))
                .on_hover_text("Start another comp of these takes")
                .clicked()
            {
                ask = Some(Ask::New);
            }
            if let Some(i) = now {
                if ui
                    .button("Duplicate")
                    .on_hover_text("Copy this comp, to try changes on")
                    .clicked()
                {
                    ask = Some(Ask::Duplicate);
                }
                match &mut self.takes_ui.renaming {
                    Some((j, name)) if *j == i => {
                        let r = ui.add(
                            egui::TextEdit::singleline(name)
                                .desired_width(110.0)
                                .hint_text("Comp name"),
                        );
                        r.request_focus();
                        if r.lost_focus() {
                            if !ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                                ask = Some(Ask::Rename(i, name.clone()));
                            }
                            self.takes_ui.renaming = None;
                        }
                    }
                    _ => {
                        if ui.button("Rename").clicked() {
                            self.takes_ui.renaming = Some((i, t.comps[i].name.clone()));
                        }
                    }
                }
                if ui
                    .button("Delete")
                    .on_hover_text("Delete this comp; the takes stay")
                    .clicked()
                {
                    ask = Some(Ask::Delete(i));
                }
                if ui
                    .button("Flatten")
                    .on_hover_text(
                        "Keep this comp as plain clips, and let the takes and other \
                         comps go. Undo brings them back",
                    )
                    .clicked()
                {
                    ask = Some(Ask::Flatten);
                }
            }
            ui.label(
                egui::RichText::new("Drag across a take to use it there")
                    .color(ui.visuals().weak_text_color()),
            );
        });

        let t = &self.tracks[ti];
        let numbers = takes(&t.clips);
        if numbers.is_empty() {
            ui.label(
                "No takes on this track yet. Record with Loop on, and each pass \
                 becomes a take of its own, stacked here.",
            );
            self.act(ti, ask);
            return;
        }
        let comped = t.comp.is_some();
        let parts = if comped {
            parts_of(&t.clips)
        } else {
            as_playing(&t.clips)
        };
        // The comp on top, then the takes, the newest first.
        let rows: Vec<Option<usize>> = std::iter::once(None)
            .chain(numbers.iter().rev().map(|&n| Some(n)))
            .collect();

        let look = crate::theme::look(ui.ctx());
        let tc = look.c().clone();
        let colour = tc.track(ti);
        let (scroll, pps) = self.timeline.x_view();
        let (grid, snapping) = (self.timeline.grid, self.timeline.snap);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let size = vec2(ui.available_width(), rows.len() as f32 * ROW);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let left = rect.left() + HEADER;
                // The lanes, not their names, which hold buttons.
                let resp = ui.interact(
                    Rect::from_min_max(pos2(left, rect.top()), rect.max),
                    ui.scope_id().with("take-lanes"),
                    Sense::click_and_drag(),
                );
                let x_of = |f: i64| left + ((f as f64 / rate as f64 - scroll) * pps as f64) as f32;
                let frame_of = |x: f32| {
                    ((scroll + (x - left) as f64 / pps as f64) * rate as f64).round() as i64
                };
                let snapped = |f: i64| {
                    let f = f.max(0);
                    if snapping {
                        timeline::snap(&map, f, grid.at_zoom(&map, f, pps as f64))
                    } else {
                        f
                    }
                };
                let row_at = |y: f32| {
                    let r = ((y - rect.top()) / ROW).floor();
                    (r >= 0.0).then(|| rows.get(r as usize).copied()).flatten()
                };
                let painter = ui.painter_at(rect);
                let body = |i: usize| {
                    Rect::from_min_max(
                        pos2(left, rect.top() + i as f32 * ROW),
                        pos2(rect.right(), rect.top() + (i + 1) as f32 * ROW),
                    )
                };

                // Dragging across a take, or clicking one.
                let pos = resp.interact_pointer_pos();
                if resp.drag_started()
                    && let Some(p) = pos
                    && p.x >= left
                    && let Some(Some(take)) = row_at(p.y)
                {
                    let f = snapped(frame_of(p.x));
                    self.takes_ui.swiping = Some((take, f, f));
                }
                if let (Some((_, _, to)), Some(p)) = (&mut self.takes_ui.swiping, pos) {
                    *to = snapped(frame_of(p.x));
                }
                if resp.drag_stopped()
                    && let Some((take, a, b)) = self.takes_ui.swiping.take()
                {
                    let (a, b) = (a.min(b), a.max(b));
                    if b - a >= MIN_LEN as i64 {
                        ask = Some(Ask::Swipe(take, a, b));
                    }
                }
                if resp.clicked()
                    && let Some(p) = pos
                    && p.x >= left
                    && let Some(Some(take)) = row_at(p.y)
                {
                    ask = Some(Ask::UseAt(take, frame_of(p.x)));
                }
                if resp.secondary_clicked()
                    && let Some(p) = pos
                {
                    self.takes_ui.menu_at =
                        (comped && p.x >= left && row_at(p.y) == Some(None)).then(|| frame_of(p.x));
                }
                let menu_part = self.takes_ui.menu_at.and_then(|f| part_at(&parts, f));
                resp.context_menu(|ui| match menu_part {
                    Some(p) => {
                        if ui
                            .button(format!("Leave Take {} out here", p.take + 1))
                            .clicked()
                        {
                            ask = Some(Ask::Clear(p.start, p.end));
                            ui.close();
                        }
                    }
                    None => {
                        ui.label("Right-click a part of the comp to leave it out.");
                    }
                });

                for (i, row) in rows.iter().enumerate() {
                    let b = body(i);
                    let head =
                        Rect::from_min_max(pos2(rect.left(), b.top()), pos2(left, b.bottom()));
                    painter.rect_filled(head, 0.0, tc.panel);
                    painter.rect_filled(b, 0.0, tc.lane);
                    painter.line_segment(
                        [
                            pos2(rect.left(), b.bottom()),
                            pos2(rect.right(), b.bottom()),
                        ],
                        Stroke::new(look.line, tc.line),
                    );
                    let inner = |s: i64, e: i64| {
                        Rect::from_x_y_ranges(
                            x_of(s).max(left)..=x_of(e).min(rect.right()),
                            b.top() + 3.0..=b.bottom() - 3.0,
                        )
                    };
                    match row {
                        None => {
                            let name = t.comp_name().unwrap_or("As recorded");
                            painter.text(
                                head.left_center() + vec2(8.0, 0.0),
                                Align2::LEFT_CENTER,
                                name,
                                look.font(look.body),
                                tc.text,
                            );
                            for p in &parts {
                                if x_of(p.end) < left || x_of(p.start) > rect.right() {
                                    continue;
                                }
                                let r = inner(p.start, p.end);
                                painter.rect_filled(r, look.clip_radius, tc.clip_fill.of(colour));
                                painter.rect_stroke(
                                    r,
                                    look.clip_radius,
                                    Stroke::new(look.line, tc.clip_border.of(colour)),
                                    StrokeKind::Inside,
                                );
                                painter.with_clip_rect(r).text(
                                    r.left_center() + vec2(6.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    format!("Take {}", p.take + 1),
                                    look.font(look.small),
                                    tc.clip_text.of(colour),
                                );
                            }
                        }
                        Some(n) => {
                            let r = ui.put(
                                head.shrink2(vec2(6.0, 8.0)),
                                egui::Button::new(format!("Take {}", n + 1)),
                            );
                            let r = r
                                .on_hover_text("Use this take at the playhead")
                                .accessible_name(format!("Use take {} at the playhead", n + 1));
                            if r.clicked() {
                                ask = Some(Ask::UseAt(*n, playhead));
                            }
                            for c in t.clips.iter().filter(|c| !c.comp && c.take == Some(*n)) {
                                if x_of(c.end()) < left || x_of(c.start) > rect.right() {
                                    continue;
                                }
                                let r = inner(c.start, c.end());
                                painter.rect_stroke(
                                    r,
                                    look.clip_radius,
                                    Stroke::new(
                                        look.line,
                                        tc.clip_border.of(colour).gamma_multiply(0.5),
                                    ),
                                    StrokeKind::Inside,
                                );
                                // Lit where the comp uses it.
                                let lit: Vec<(i64, i64)> = (parts.iter())
                                    .filter(|p| p.take as usize == *n)
                                    .map(|p| (p.start.max(c.start), p.end.min(c.end())))
                                    .filter(|(s, e)| s < e)
                                    .collect();
                                for &(s, e) in &lit {
                                    painter.rect_filled(
                                        inner(s, e),
                                        look.clip_radius,
                                        tc.clip_fill.of(colour),
                                    );
                                }
                                if let Some(w) = self.timeline.wave(&c.source) {
                                    let mid = r.center().y;
                                    let half = (r.height() - 6.0) / 2.0;
                                    let mut cols = (Vec::new(), Vec::new());
                                    let mut x = r.left().floor();
                                    while x < r.right() {
                                        let (f0, f1) = (frame_of(x), frame_of(x + 1.0));
                                        let src = |f: i64| {
                                            (f - c.start + c.offset as i64)
                                                .clamp(c.offset as i64, (c.offset + c.len) as i64)
                                                as usize
                                        };
                                        let (a, b) = (src(f0), src(f1));
                                        if b > a {
                                            let (lo, hi) = w.range(a, b);
                                            let col = (
                                                x + 0.5,
                                                mid - hi.min(1.0) * half,
                                                mid - lo.max(-1.0) * half - 0.5,
                                            );
                                            if lit.iter().any(|&(s, e)| s <= f0 && f0 < e) {
                                                cols.0.push(col);
                                            } else {
                                                cols.1.push(col);
                                            }
                                        }
                                        x += 1.0;
                                    }
                                    timeline::draw_wave(
                                        &painter,
                                        &look,
                                        &cols.1,
                                        mid,
                                        colour.gamma_multiply(0.35),
                                    );
                                    timeline::draw_wave(&painter, &look, &cols.0, mid, colour);
                                }
                            }
                            if let Some((take, a, b)) = self.takes_ui.swiping
                                && take == *n
                            {
                                let r = inner(a.min(b), a.max(b));
                                painter.rect_filled(r, 0.0, tc.selection.gamma_multiply(0.6));
                                painter.rect_stroke(
                                    r,
                                    0.0,
                                    Stroke::new(1.0, tc.accent),
                                    StrokeKind::Inside,
                                );
                            }
                        }
                    }
                }
                let x = x_of(playhead);
                if x >= left && x <= rect.right() {
                    painter.line_segment(
                        [pos2(x, rect.top()), pos2(x, rect.bottom())],
                        Stroke::new(1.5, tc.playhead),
                    );
                }
                if self.takes_ui.swiping.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
            });
        self.act(ti, ask);
    }

    /// Does what the pane asked, on track `ti`.
    fn act(&mut self, ti: usize, ask: Option<Ask>) {
        let Some(ask) = ask else {
            return;
        };
        let rate = self.rate;
        let mut tk = self.tracks[ti].takes(rate);
        match ask {
            Ask::Swipe(take, a, b) => tk.swipe(take, a, b),
            Ask::UseAt(take, at) => {
                tk.use_at(take, at);
            }
            Ask::Clear(a, b) => tk.clear(a, b),
            Ask::Choose(which) => tk.choose(which),
            Ask::New => {
                tk.new_comp();
            }
            Ask::Duplicate => {
                tk.duplicate();
            }
            Ask::Delete(i) => tk.delete(i),
            Ask::Flatten => {
                tk.flatten();
            }
            Ask::Rename(i, name) => tk.rename(i, &name),
        }
        // Pieces made again: the old ones can't stay selected.
        let ids: std::collections::HashSet<u64> =
            self.tracks[ti].clips.iter().map(|c| c.id).collect();
        let others: std::collections::HashSet<u64> = (self.tracks.iter().enumerate())
            .filter(|(i, _)| *i != ti)
            .flat_map(|(_, t)| t.clips.iter().map(|c| c.id))
            .collect();
        self.timeline
            .selected
            .retain(|id| ids.contains(id) || others.contains(id));
        self.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::tests::clip;

    fn part(take: u32, start: i64, end: i64) -> CompPart {
        CompPart { take, start, end }
    }

    #[test]
    fn swiping_puts_a_take_over_what_was_there() {
        let mut parts = vec![part(0, 0, 1000)];
        swipe(&mut parts, 1, 200, 400);
        assert_eq!(
            parts,
            [part(0, 0, 200), part(1, 200, 400), part(0, 400, 1000)]
        );
        // The same take again, touching: one part.
        swipe(&mut parts, 1, 400, 600);
        assert_eq!(
            parts,
            [part(0, 0, 200), part(1, 200, 600), part(0, 600, 1000)]
        );
        // Over everything.
        swipe(&mut parts, 2, 0, 2000);
        assert_eq!(parts, [part(2, 0, 2000)]);
        clear(&mut parts, 500, 700);
        assert_eq!(parts, [part(2, 0, 500), part(2, 700, 2000)]);
        assert_eq!(part_at(&parts, 600), None);
        assert_eq!(part_at(&parts, 700), Some(part(2, 700, 2000)));
    }

    /// Three takes recorded over the same 10 000 frames, as loop
    /// recording leaves them (each its own file).
    fn three_takes() -> Vec<Clip> {
        let mut clips = Vec::new();
        for _ in 0..3 {
            timeline::add_take(&mut clips, clip(48_000, 10_000));
        }
        clips
    }

    fn comps() -> (Vec<Comp>, Option<String>) {
        (Vec::new(), None)
    }

    fn playing(clips: &[Clip]) -> Vec<(Option<usize>, i64, i64)> {
        let mut v: Vec<_> = (clips.iter())
            .filter(|c| !c.muted)
            .map(|c| (c.take, c.start, c.end()))
            .collect();
        v.sort_by_key(|p| p.1);
        v
    }

    #[test]
    fn a_comp_plays_pieces_of_each_take_crossfaded_where_they_meet() {
        let mut clips = three_takes();
        let (mut list, mut comp) = comps();
        let rate = 48_000;
        let mut tk = Takes {
            clips: &mut clips,
            comps: &mut list,
            comp: &mut comp,
            rate,
        };
        // Starts from what plays: the newest take.
        tk.swipe(0, 52_000, 55_000);
        assert_eq!(tk.comps.len(), 1);
        assert_eq!(tk.comps[0].name, "Comp 1");
        assert_eq!(
            tk.comps[0].parts,
            [
                part(2, 48_000, 52_000),
                part(0, 52_000, 55_000),
                part(2, 55_000, 58_000)
            ]
        );
        // Every take is muted; three pieces play, reaching 10 ms into
        // each other at the joins.
        let half = 480;
        assert_eq!(
            playing(tk.clips),
            [
                (Some(2), 48_000, 52_000 + half),
                (Some(0), 52_000 - half, 55_000 + half),
                (Some(2), 55_000 - half, 58_000),
            ]
        );
        let pieces: Vec<&Clip> = tk.clips.iter().filter(|c| c.comp).collect();
        assert!(tk.clips.iter().filter(|c| !c.comp).all(|c| c.muted));
        let middle = pieces.iter().find(|c| c.take == Some(0)).unwrap();
        assert_eq!(middle.fade_in.len, 2 * half as usize);
        assert_eq!(middle.fade_out.len, 2 * half as usize);
        assert_eq!(middle.fade_in.curve, Curve::EqualPower);
        // The audio stays where it was recorded.
        assert_eq!(middle.offset as i64, middle.start - 48_000);
        // Read back from the pieces, the joins are where they were put.
        assert_eq!(parts_of(tk.clips), tk.comps[0].parts);
    }

    #[test]
    fn an_end_with_nothing_beside_it_fades_briefly() {
        let mut clips = three_takes();
        let mut parts = Vec::new();
        swipe(&mut parts, 1, 50_000, 51_000);
        apply(&mut clips, &parts, 48_000);
        let p = clips.iter().find(|c| c.comp).unwrap();
        assert_eq!((p.start, p.end()), (50_000, 51_000));
        assert_eq!(p.fade_in.len, 240);
        assert_eq!(p.fade_out.len, 240);
        // Where a piece ends with its take, the take's own end stays.
        let mut parts = Vec::new();
        swipe(&mut parts, 1, 40_000, 51_000);
        apply(&mut clips, &parts, 48_000);
        let p = clips.iter().find(|c| c.comp).unwrap();
        assert_eq!((p.start, p.end()), (48_000, 51_000));
        assert_eq!(p.fade_in.len, 0);
        assert_eq!(clips.iter().filter(|c| c.comp).count(), 1, "old pieces go");
    }

    #[test]
    fn several_comps_of_the_same_takes_can_be_switched() {
        let mut clips = three_takes();
        let (mut list, mut comp) = comps();
        let mut tk = Takes {
            clips: &mut clips,
            comps: &mut list,
            comp: &mut comp,
            rate: 48_000,
        };
        tk.swipe(0, 48_000, 53_000);
        let first = tk.comps[0].parts.clone();
        tk.duplicate();
        assert_eq!(tk.active(), Some(1));
        assert_eq!(tk.comps[1].name, "Comp 2");
        tk.swipe(1, 48_000, 53_000);
        assert_eq!(tk.comps[0].parts, first, "the first comp is kept");
        tk.choose(Some(0));
        assert_eq!(parts_of(tk.clips), first);
        // As recorded: no pieces, the newest take plays.
        tk.choose(None);
        assert_eq!(tk.active(), None);
        assert_eq!(playing(tk.clips), [(Some(2), 48_000, 58_000)]);
        assert_eq!(tk.comps.len(), 2, "the comps are kept");
        // A new comp then starts from what plays.
        tk.new_comp();
        assert_eq!(tk.comps[2].parts, [part(2, 48_000, 58_000)]);
        tk.rename(2, "  Best  ");
        assert_eq!(tk.comps[2].name, "Best");
        tk.delete(2);
        assert_eq!(tk.active(), None);
        assert_eq!(tk.comps.len(), 2);
    }

    #[test]
    fn edits_to_the_pieces_on_the_timeline_change_the_comp() {
        let mut clips = three_takes();
        let (mut list, mut comp) = comps();
        let mut tk = Takes {
            clips: &mut clips,
            comps: &mut list,
            comp: &mut comp,
            rate: 48_000,
        };
        tk.swipe(1, 50_000, 52_000);
        // Delete the take-1 piece, as on the timeline.
        tk.clips.retain(|c| !(c.comp && c.take == Some(1)));
        tk.swipe(0, 56_000, 57_000);
        assert_eq!(
            tk.comps[0].parts,
            [
                part(2, 48_000, 50_000 + 480),
                part(2, 52_000 - 480, 56_000),
                part(0, 56_000, 57_000),
                part(2, 57_000, 58_000),
            ],
            "the deleted piece's time stays empty, but for the crossfades \
             that reached into it"
        );
    }

    #[test]
    fn a_new_take_goes_into_the_comp_and_undoing_it_brings_back_the_one_before() {
        let mut clips = three_takes();
        let (mut list, mut comp) = comps();
        let mut tk = Takes {
            clips: &mut clips,
            comps: &mut list,
            comp: &mut comp,
            rate: 48_000,
        };
        tk.swipe(0, 48_000, 50_000);
        tk.add_take(clip(54_000, 2000));
        assert_eq!(
            tk.comps[0].parts,
            [
                part(0, 48_000, 50_000),
                part(2, 50_000, 54_000),
                part(3, 54_000, 56_000),
                part(2, 56_000, 58_000)
            ]
        );
        assert!(tk.clips.iter().filter(|c| !c.comp).all(|c| c.muted));
        // Undo the last take: take 2, the newest left, plays there.
        tk.clips.retain(|c| c.take != Some(3));
        tk.takes_gone();
        assert_eq!(
            tk.comps[0].parts,
            [part(0, 48_000, 50_000), part(2, 50_000, 58_000)]
        );
        // Without a comp, a take arrives as before.
        tk.choose(None);
        tk.add_take(clip(54_000, 2000));
        assert_eq!(tk.active(), None);
        assert!(!tk.clips.iter().any(|c| c.comp));
    }

    #[test]
    fn flattening_keeps_the_comp_as_plain_clips() {
        let mut clips = three_takes();
        let (mut list, mut comp) = comps();
        let mut tk = Takes {
            clips: &mut clips,
            comps: &mut list,
            comp: &mut comp,
            rate: 48_000,
        };
        assert!(!tk.flatten(), "nothing to flatten without a comp");
        tk.swipe(0, 48_000, 50_000);
        let before = playing(tk.clips);
        assert!(tk.flatten());
        assert!(tk.comps.is_empty() && tk.comp.is_none());
        assert_eq!(tk.clips.len(), 2);
        assert!(
            tk.clips
                .iter()
                .all(|c| c.take.is_none() && !c.comp && !c.muted)
        );
        assert_eq!(tk.clips[0].name(), "Comp 1, take 1");
        let after: Vec<_> = (tk.clips.iter()).map(|c| (c.start, c.end())).collect();
        assert_eq!(after, before.iter().map(|p| (p.1, p.2)).collect::<Vec<_>>());
    }

    #[test]
    fn choosing_a_take_from_the_list_leaves_the_comp() {
        let mut clips = three_takes();
        let (mut list, mut comp) = comps();
        let mut tk = Takes {
            clips: &mut clips,
            comps: &mut list,
            comp: &mut comp,
            rate: 48_000,
        };
        tk.swipe(0, 48_000, 50_000);
        tk.choose_take(Some(1));
        assert_eq!(tk.active(), None);
        assert_eq!(playing(tk.clips), [(Some(1), 48_000, 58_000)]);
        // Clicking a take uses it for the part there.
        assert!(tk.use_at(0, 49_000));
        assert_eq!(tk.comps.len(), 2);
        assert_eq!(tk.comps[1].parts, [part(0, 48_000, 58_000)]);
    }

    mod in_the_app {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;

        use super::*;
        use crate::commands::Cmd;
        use crate::{App, Track};

        /// A track with three takes of the same two bars, the Take lanes
        /// pane open.
        fn harness() -> Harness<'static, App> {
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let app = h.state_mut();
            app.tracks.push(Track::new("Guitar".into()));
            for _ in 0..3 {
                timeline::add_take(&mut app.tracks[0].clips, clip(0, 96_000));
            }
            app.rebuild();
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, Cmd::TakeLanes);
            h.run();
            // Room to see every lane.
            h.get_by_label("Close mixer").click();
            h.run();
            h
        }

        #[test]
        fn a_take_used_from_its_lane_makes_a_comp_that_is_saved_and_undone() {
            let mut h = harness();
            assert!(h.query_by_label("Take lanes tab").is_some());
            for n in 1..=3 {
                assert!(
                    h.query_by_label(&format!("Use take {n} at the playhead"))
                        .is_some()
                );
            }
            let ctx = h.ctx.clone();
            h.state_mut().sync_doc(&ctx, true);
            h.get_by_label("Use take 1 at the playhead").click();
            h.run();
            let t = &h.state().tracks[0];
            assert_eq!(t.comp_name(), Some("Comp 1"));
            assert_eq!(parts_of(&t.clips), [part(0, 0, 96_000)]);
            // In the project, with its parts.
            let p = h.state().project();
            assert_eq!(p.tracks[0].comps.len(), 1);
            assert_eq!(p.tracks[0].comps[0].parts, [part(0, 0, 96_000)]);
            assert_eq!(p.tracks[0].comp, Some(p.tracks[0].comps[0].id.clone()));
            assert_eq!(p.tracks[0].clips.iter().filter(|c| c.comp).count(), 1);
            // Read back in, it plays the same.
            h.state_mut().apply(p.clone());
            h.run();
            assert_eq!(h.state().project(), p);
            // Undo goes back to the takes as recorded.
            h.state_mut().sync_doc(&ctx, true);
            h.state_mut().undo(&ctx);
            h.run();
            let t = &h.state().tracks[0];
            assert_eq!(t.comp_name(), None);
            assert!(t.comps.is_empty());
            assert_eq!(timeline::active_take(&t.clips), Some(2));
        }
    }
}
