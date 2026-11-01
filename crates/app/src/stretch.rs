// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio that follows tempo (roadmap 6.1): clips set to musical time
//! follow the tempo map, stretched live and offline; stretch markers
//! found from attacks or placed by hand; audio quantise with a strength
//! and a window; and a tempo map made from a free performance.
//!
//! The clip model is in [`crate::timeline::Clip`]: a clip that follows
//! tempo keeps the beat it starts on and its own tempo, and markers tie
//! source frames to clip time. Here are the commands over the selection
//! and their windows.

use eframe::egui::{self, RichText};

use daw_engine::time::{Frames, PPQ, TempoMap, Ticks};
use daw_project::TempoChange;

use crate::commands::Cmd;
use crate::timeline::{self, Clip, Grid};
use crate::{App, timing};

/// How the beats of a performance are counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    /// Every attack is a beat.
    #[default]
    Beat,
    /// Every attack starts a bar.
    Bar,
}

impl Unit {
    pub const ALL: [Unit; 2] = [Unit::Beat, Unit::Bar];

    pub fn label(self) -> &'static str {
        match self {
            Unit::Beat => "A beat",
            Unit::Bar => "A bar",
        }
    }
}

/// Audio quantise settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantise {
    pub grid: Grid,
    /// How far toward the grid, 0 to 1.
    pub strength: f64,
    /// Attacks further from the grid than this stay put.
    pub window_ms: f64,
}

impl Default for Quantise {
    fn default() -> Quantise {
        Quantise {
            grid: Grid::Part(2),
            strength: 1.0,
            window_ms: 60.0,
        }
    }
}

/// Settings for a tempo map from a performance.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoFrom {
    pub unit: Unit,
    /// Attack detection sensitivity, 0 to 1.
    pub sensitivity: f32,
    /// Take the clip's stretch markers as the beats when it has any.
    pub use_markers: bool,
}

impl Default for TempoFrom {
    fn default() -> TempoFrom {
        TempoFrom {
            unit: Unit::Beat,
            sensitivity: 0.5,
            use_markers: true,
        }
    }
}

/// Which window is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    ClipTempo,
    Quantise,
    TempoFrom,
    BeatGrid,
}

#[derive(Default)]
pub(crate) struct Stretch {
    pub open: Option<Window>,
    /// The tempo being typed for the selected clips.
    pub tempo_text: String,
    pub quantise: Quantise,
    pub tempo_from: TempoFrom,
    /// Attack detection sensitivity for markers from attacks.
    pub sensitivity: f32,
    pub message: Option<String>,
    /// The beat grid being found, or found, for the beat grid window.
    pub beat_grid: Option<crate::beat_grid::Finding>,
}

/// Timeline frames attacks are at and where they go.
type Moves = Vec<(i64, i64)>;

/// The slowest and fastest tempo a performance can set.
const TEMPO_RANGE: (f64, f64) = (20.0, 400.0);

/// Attacks closer than this are one hit.
const MIN_GAP_SECONDS: f64 = 0.08;

/// Where the attacks are in a clip's audio, in source frames.
fn attacks_in(c: &Clip, sensitivity: f32) -> Result<Vec<i64>, String> {
    let samples = c.source.read_mono(c.source.frames)?;
    let gap = (c.source.rate as f64 * MIN_GAP_SECONDS) as usize;
    let mut out: Vec<i64> = Vec::new();
    for f in timing::attacks(&samples, c.source.rate, sensitivity) {
        if f <= c.offset || f >= c.offset + c.len {
            continue;
        }
        if out.last().is_some_and(|&l| f < l as usize + gap) {
            continue;
        }
        out.push(f as i64);
    }
    Ok(out)
}

/// A tempo map from when the beats of a performance fell: `hits` are
/// timeline frames, one per `unit`, counted under `map`'s time
/// signatures. Gives the starting tempo, the tempo changes, and how
/// far the performance moves later so its first hit lands on a beat
/// (0 unless it starts within half a unit of the song's start).
pub(crate) fn tempo_map_from(
    map: &TempoMap,
    rate: u32,
    hits: &[i64],
    unit: Unit,
) -> Result<(f64, Vec<TempoChange>, i64), String> {
    if hits.len() < 2 {
        return Err("Found fewer than two beats".into());
    }
    let sig = map.signature_at(Ticks(0));
    let ticks_per_hit = match unit {
        Unit::Beat => sig.beat_ticks(),
        Unit::Bar => sig.bar_ticks(),
    };
    let bpm_of = |frames: i64| {
        (ticks_per_hit as f64 / PPQ as f64) * 60.0 * rate as f64 / frames.max(1) as f64
    };
    // Whole units before the first hit, at the first interval's tempo.
    let first = hits[1] - hits[0];
    let mut k = (hits[0] as f64 / first as f64).round() as i64;
    let mut shift = 0;
    if k == 0 {
        // A bar of lead-in at the first tempo.
        k = sig.bar_ticks() / ticks_per_hit;
        shift = k * first - hits[0];
    }
    let start_frames = hits[0] + shift;
    let start_bpm = (bpm_of(start_frames) * k as f64).clamp(TEMPO_RANGE.0, TEMPO_RANGE.1);
    let mut changes = Vec::new();
    for (i, w) in hits.windows(2).enumerate() {
        let bpm = bpm_of(w[1] - w[0]);
        if !(TEMPO_RANGE.0..=TEMPO_RANGE.1).contains(&bpm) {
            return Err(format!(
                "Beats {} and {} are {:.0} ms apart, outside the tempo range",
                i + 1,
                i + 2,
                (w[1] - w[0]) as f64 * 1000.0 / rate as f64
            ));
        }
        let at = Ticks((k + i as i64) * ticks_per_hit);
        let bb = map.bar_beat(at);
        changes.push(TempoChange {
            id: daw_project::new_id(),
            bar: bb.bar,
            beat: bb.beat as u32,
            bpm,
            glide: false,
        });
    }
    Ok((start_bpm, changes, shift))
}

/// The beat line (or bar line, if `bar`) nearest timeline frame `at`.
pub(crate) fn nearest_line(map: &TempoMap, at: i64, bar: bool) -> i64 {
    let t = map.ticks_at(Frames(at));
    let bb = map.bar_beat(t);
    let sig = map.signature_at(t);
    let (before, after) = if bar {
        (map.bar_start(bb.bar), map.bar_start(bb.bar + 1))
    } else {
        let b = Ticks(t.0 - bb.tick);
        (b, Ticks(b.0 + sig.beat_ticks()))
    };
    let (b, a) = (map.frames_at(before).0, map.frames_at(after).0);
    if at - b <= a - at { b } else { a }
}

/// Where a clip starting at frame `start` (tick `beat`, when it
/// `follows` tempo) starts once moved so the point now at timeline frame
/// `anchor` is at `to`: in frames, and in ticks. A clip that follows
/// tempo moves in ticks, so the point lands exactly whatever the tempo
/// does in between.
pub(crate) fn moved_by_anchor(
    map: &TempoMap,
    start: i64,
    beat: i64,
    follows: bool,
    anchor: i64,
    to: i64,
) -> Result<(i64, i64), String> {
    let (start, beat) = if follows {
        let into = map.ticks_at(Frames(anchor)).0 - beat;
        let beat = map.ticks_at(Frames(to)).0 - into;
        (map.frames_at(Ticks(beat)).0, beat)
    } else {
        let start = start + (to - anchor);
        (start, map.ticks_at(Frames(start.max(0))).0)
    };
    if start < 0 {
        return Err(format!(
            "That would start the clip {:.3} seconds before the song does.",
            -start as f64 / map.sample_rate().max(1) as f64
        ));
    }
    Ok((start, beat))
}

impl App {
    /// Moves the chosen clips so the point of them under the playhead
    /// lands on the nearest beat line (or bar line, if `bar`); clips
    /// grouped with them move by as much. Nothing moves unless the
    /// playhead is inside one of them.
    pub(crate) fn anchor_to_grid(&mut self, bar: bool) {
        let at = self.ctl.status().position().0;
        self.line_up(at, bar);
    }

    /// Moves the chosen clips so their point at timeline frame `at`
    /// lands on the nearest beat line (or bar line, if `bar`); clips
    /// grouped with them move by as much. Nothing moves unless `at` is
    /// inside one of them.
    pub(crate) fn line_up(&mut self, at: i64, bar: bool) {
        if !self.may_edit_song() {
            return;
        }
        let map = self.tempo_map();
        let chosen = self.stretch_clips();
        let inside = chosen.iter().any(|&(l, id)| {
            (self.tracks[l].clips.iter()).any(|c| c.id == id && at >= c.start && at <= c.end())
        });
        if !inside {
            self.said
                .say("Put the playhead on the point of the clip to line up".into());
            return;
        }
        let to = nearest_line(&map, at, bar);
        if to == at {
            return;
        }
        let mut moves = Vec::new();
        for &(l, id) in &chosen {
            let Some(c) = self.tracks[l].clips.iter().find(|c| c.id == id) else {
                continue;
            };
            match moved_by_anchor(&map, c.start, c.beat, c.tempo.is_some(), at, to) {
                Ok(m) => moves.push((l, id, m)),
                Err(e) => {
                    self.said.say(e);
                    return;
                }
            }
        }
        for (l, id, (start, beat)) in moves {
            if let Some(c) = self.tracks[l].clips.iter_mut().find(|c| c.id == id) {
                c.start = start;
                if c.tempo.is_some() {
                    c.beat = beat;
                }
                c.retime(&map);
            }
        }
        let by = (to - at) as f64 / self.rate.max(1) as f64;
        self.said.say(format!(
            "Moved {:.3} seconds {}",
            by.abs(),
            if by > 0.0 { "later" } else { "earlier" }
        ));
        self.rebuild();
    }

    /// The selected audio clips and those grouped with them, by lane
    /// and id: what the stretch commands act on.
    pub(crate) fn stretch_clips(&mut self) -> Vec<(usize, u64)> {
        self.timing_clips()
    }

    /// Whether a stretch command has something to act on.
    pub(crate) fn has_stretch_clips(&self) -> bool {
        (self.tracks.iter())
            .flat_map(|t| &t.clips)
            .any(|c| self.timeline.selected.contains(&c.id))
    }

    /// Does `f` to each chosen clip, then rebuilds if any changed.
    fn each_stretch_clip(&mut self, mut f: impl FnMut(&mut Clip, &TempoMap) -> bool) {
        if !self.may_edit_song() {
            return;
        }
        let map = self.tempo_map();
        let chosen = self.stretch_clips();
        let mut changed = false;
        for (l, id) in chosen {
            if let Some(c) = self.tracks[l].clips.iter_mut().find(|c| c.id == id) {
                changed |= f(c, &map);
            }
        }
        if changed {
            self.rebuild();
        }
    }

    /// Makes the chosen clips follow tempo, or stop, by the selection:
    /// on if any selected clip doesn't follow yet.
    pub(crate) fn toggle_follow_tempo(&mut self) {
        let selected = &self.timeline.selected;
        let on = (self.tracks.iter())
            .flat_map(|t| &t.clips)
            .any(|c| selected.contains(&c.id) && c.tempo.is_none());
        self.each_stretch_clip(|c, map| {
            let was = c.tempo.is_some();
            c.follow_tempo(map, on);
            was != c.tempo.is_some()
        });
    }

    /// Sets the chosen clips' own tempo, making them follow tempo if
    /// they don't yet.
    pub(crate) fn set_clip_tempo(&mut self, bpm: f64) {
        if !(TEMPO_RANGE.0..=TEMPO_RANGE.1).contains(&bpm) {
            self.stretch.message = Some(format!(
                "A tempo from {} to {} BPM",
                TEMPO_RANGE.0, TEMPO_RANGE.1
            ));
            return;
        }
        self.stretch.message = None;
        self.each_stretch_clip(|c, map| {
            c.follow_tempo(map, true);
            c.set_tempo(map, bpm);
            true
        });
    }

    /// Puts a stretch marker on every attack in the chosen clips.
    pub(crate) fn markers_from_attacks(&mut self) {
        let sensitivity = self.stretch.sensitivity();
        let mut problem = None;
        self.each_stretch_clip(|c, map| match attacks_in(c, sensitivity) {
            Ok(found) => {
                let n = c.markers.len();
                for f in found {
                    c.add_marker(f);
                }
                c.retime(map);
                c.markers.len() != n
            }
            Err(e) => {
                problem = Some(e);
                false
            }
        });
        self.stretch.message = problem;
    }

    /// Adds a stretch marker where the playhead is in each chosen clip.
    pub(crate) fn add_stretch_marker(&mut self) {
        let at = self.ctl.status().position().0;
        self.each_stretch_clip(|c, map| {
            if at <= c.start || at >= c.end() {
                return false;
            }
            let frame = c.frame_of_out(map, at).round() as i64;
            let n = c.markers.len();
            c.add_marker(frame);
            c.retime(map);
            c.markers.len() != n
        });
    }

    /// Takes every stretch marker off the chosen clips; the audio
    /// between them plays evenly again.
    pub(crate) fn clear_stretch_markers(&mut self) {
        self.each_stretch_clip(|c, map| {
            if c.markers.is_empty() {
                return false;
            }
            c.markers.clear();
            c.retime(map);
            true
        });
    }

    /// Moves the chosen clips' attacks toward the grid by the quantise
    /// settings, putting markers on them first where there are none.
    /// Clips grouped with a selected one get the same moves, by where
    /// they fall on the timeline, so a kit's mics stay together.
    pub(crate) fn quantise_audio(&mut self) {
        if !self.may_edit_song() {
            return;
        }
        let q = self.stretch.quantise.clone();
        let sensitivity = self.stretch.sensitivity();
        let map = self.tempo_map();
        let window = (q.window_ms / 1000.0 * self.rate as f64).round() as i64;
        let chosen = self.stretch_clips();
        let selected = self.timeline.selected.clone();
        // Where the selected clips' attacks are and where they go.
        let mut moves: Vec<(usize, u64, Moves)> = Vec::new();
        let mut problem = None;
        for &(l, id) in chosen.iter().filter(|(_, id)| selected.contains(id)) {
            let Some(c) = self.tracks[l].clips.iter_mut().find(|c| c.id == id) else {
                continue;
            };
            if c.markers.is_empty() {
                match attacks_in(c, sensitivity) {
                    Ok(found) => {
                        for f in found {
                            c.add_marker(f);
                        }
                    }
                    Err(e) => {
                        problem = Some(e);
                        continue;
                    }
                }
            }
            let mut mv = Vec::new();
            for m in &c.markers {
                let out = c.out_of_frame(&map, m.frame as f64);
                let target = timeline::snap(&map, out, q.grid);
                let d = target - out;
                if d != 0 && d.abs() <= window {
                    mv.push((out, out + (d as f64 * q.strength).round() as i64));
                }
            }
            moves.push((l, id, mv));
        }
        let mut changed = false;
        for (lead_lane, lead, mv) in &moves {
            // The selected clip itself, then its partners on other lanes
            // where they overlap it.
            let range = {
                let c = self.tracks[*lead_lane].clips.iter().find(|c| c.id == *lead);
                c.map(|c| (c.start, c.end()))
            };
            let Some((lo, hi)) = range else {
                continue;
            };
            for &(l, id) in &chosen {
                let same = id == *lead;
                if !same && (l == *lead_lane || selected.contains(&id)) {
                    continue;
                }
                let Some(c) = self.tracks[l].clips.iter_mut().find(|c| c.id == id) else {
                    continue;
                };
                if !same && (c.end() <= lo || c.start >= hi) {
                    continue;
                }
                for &(from, to) in mv {
                    if from <= c.start || from >= c.end() {
                        continue;
                    }
                    let frame = c.frame_of_out(&map, from).round() as i64;
                    // An attack that already has a marker moves it: the
                    // way back from the timeline can land a frame or two
                    // off, and a second marker beside it would pin the
                    // audio where it was.
                    let near = (c.source.rate as i64 / 500).max(1);
                    let found = (c.markers.iter()).position(|m| (m.frame - frame).abs() <= near);
                    let Some(i) = found.or_else(|| c.add_marker(frame)) else {
                        continue;
                    };
                    let at = c.at_of_out(&map, to).round() as i64;
                    changed |= c.move_marker(i, at);
                }
                c.retime(&map);
            }
        }
        self.stretch.message = problem;
        if changed {
            self.rebuild();
        }
    }

    /// Makes the song's tempo map from the first selected clip's beats:
    /// its markers if it has any (and the setting says so), else its
    /// attacks. The clip stops following tempo, since it sets it, and
    /// the tempo changes there were are replaced.
    pub(crate) fn tempo_from_clip(&mut self) {
        if !self.may_edit_song() {
            return;
        }
        let settings = self.stretch.tempo_from.clone();
        let map = self.tempo_map();
        let selected = self.timeline.selected.clone();
        let Some((l, id)) =
            (self.stretch_clips().into_iter()).find(|(_, id)| selected.contains(id))
        else {
            return;
        };
        let Some(c) = self.tracks[l].clips.iter_mut().find(|c| c.id == id) else {
            return;
        };
        let frames: Vec<i64> = if settings.use_markers && !c.markers.is_empty() {
            std::iter::once(c.offset as i64)
                .chain(c.markers.iter().map(|m| m.frame))
                .collect()
        } else {
            match attacks_in(c, settings.sensitivity) {
                Ok(f) => f,
                Err(e) => {
                    self.stretch.message = Some(e);
                    return;
                }
            }
        };
        let hits: Vec<i64> = frames
            .iter()
            .map(|&f| c.out_of_frame(&map, f as f64))
            .collect();
        let (bpm, changes, shift) = match tempo_map_from(&map, self.rate, &hits, settings.unit) {
            Ok(x) => x,
            Err(e) => {
                self.stretch.message = Some(e);
                return;
            }
        };
        self.stretch.message = None;
        c.follow_tempo(&map, false);
        c.markers.clear();
        c.start += shift;
        c.retime(&map);
        self.tempo_bpm = bpm;
        self.time.tempos = changes;
        self.time_changed();
        self.stretch.open = None;
    }

    /// The tempo to offer in the clip tempo window: what the selected
    /// clips have, if they agree.
    fn selected_clip_tempo(&self) -> Option<f64> {
        let selected = &self.timeline.selected;
        let mut tempos = (self.tracks.iter())
            .flat_map(|t| &t.clips)
            .filter(|c| selected.contains(&c.id))
            .map(|c| c.tempo);
        let first = tempos.next()??;
        tempos.all(|t| t == Some(first)).then_some(first)
    }

    /// The longest selected clip's length in seconds, for guessing a
    /// loop's tempo.
    fn selected_clip_seconds(&self) -> Option<f64> {
        let selected = &self.timeline.selected;
        (self.tracks.iter())
            .flat_map(|t| &t.clips)
            .filter(|c| selected.contains(&c.id))
            .map(|c| c.len as f64 / c.source.rate.max(1) as f64)
            .max_by(f64::total_cmp)
    }

    pub(crate) fn stretch_windows(&mut self, ctx: &egui::Context) {
        match self.stretch.open {
            Some(Window::ClipTempo) => self.clip_tempo_window(ctx),
            Some(Window::Quantise) => self.quantise_window(ctx),
            Some(Window::TempoFrom) => self.tempo_from_window(ctx),
            Some(Window::BeatGrid) => self.beat_grid_window(ctx),
            None => {}
        }
    }

    fn clip_tempo_window(&mut self, ctx: &egui::Context) {
        let may_edit = self.may_edit_song() && self.has_stretch_clips();
        let song_bpm = self.tempo_bpm;
        let guess =
            (self.selected_clip_seconds()).and_then(|s| crate::browser::guess_bpm(s, song_bpm));
        if self.stretch.tempo_text.is_empty() {
            let bpm = self.selected_clip_tempo().unwrap_or(song_bpm);
            self.stretch.tempo_text = format!("{bpm:.2}");
        }
        let mut open = true;
        let mut set = None;
        let s = &mut self.stretch;
        egui::Window::new("Clip tempo")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "The tempo the selected clips were played at. They follow the song's \
                     tempo from there: faster where it is faster, slower where slower.",
                );
                if !may_edit {
                    ui.label(RichText::new("Select an audio clip first.").weak());
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label("Tempo");
                    ui.add(egui::TextEdit::singleline(&mut s.tempo_text).desired_width(70.0))
                        .labelled_by(ui.label("BPM").id);
                });
                ui.horizontal(|ui| {
                    if ui.button("Use the song's tempo").clicked() {
                        s.tempo_text = format!("{song_bpm:.2}");
                    }
                    if let Some(g) = guess
                        && ui
                            .button(format!("Guess from length: {g:.1}"))
                            .on_hover_text("Taking the clip as a loop of whole bars")
                            .clicked()
                    {
                        s.tempo_text = format!("{g:.2}");
                    }
                });
                if let Some(m) = &s.message {
                    ui.colored_label(ui.visuals().warn_fg_color, m);
                }
                if ui.button("Set tempo").clicked() {
                    set = Some(s.tempo_text.trim().parse::<f64>());
                }
            });
        match set {
            Some(Ok(bpm)) => self.set_clip_tempo(bpm),
            Some(Err(_)) => self.stretch.message = Some("A number of beats per minute".into()),
            None => {}
        }
        if !open {
            self.stretch.open = None;
            self.stretch.tempo_text.clear();
        }
    }

    fn quantise_window(&mut self, ctx: &egui::Context) {
        let may_edit = self.may_edit_song() && self.has_stretch_clips();
        let mut open = true;
        let mut apply = false;
        let s = &mut self.stretch;
        egui::Window::new("Quantise audio")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Moves each attack in the selected clips toward the grid, stretching the \
                     audio between them so nothing is cut. Grouped tracks move together.",
                );
                if !may_edit {
                    ui.label(RichText::new("Select an audio clip first.").weak());
                    return;
                }
                let q = &mut s.quantise;
                egui::Grid::new("quantise-audio").num_columns(2).show(ui, |ui| {
                    ui.label("Move attacks to");
                    egui::ComboBox::from_id_salt("quantise-audio-grid")
                        .selected_text(q.grid.label())
                        .show_ui(ui, |ui| {
                            for g in Grid::ALL.into_iter().filter(|g| *g != Grid::Auto) {
                                ui.selectable_value(&mut q.grid, g, g.label());
                            }
                        })
                        .response
                        .accessible_name("Grid to move attacks to");
                    ui.end_row();
                    ui.label("Strength");
                    ui.add(
                        egui::Slider::new(&mut q.strength, 0.0..=1.0)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    )
                    .on_hover_text("How far toward the grid: 100% puts attacks right on it")
                    .accessible_name("Quantise strength");
                    ui.end_row();
                    ui.label("Window");
                    ui.add(egui::Slider::new(&mut q.window_ms, 5.0..=250.0).suffix(" ms"))
                        .on_hover_text("Attacks further from the grid than this stay put")
                        .accessible_name("Quantise window");
                    ui.end_row();
                    ui.label("Sensitivity");
                    ui.add(
                        egui::Slider::new(&mut s.sensitivity, 0.0..=1.0)
                            .step_by(0.05)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    )
                    .on_hover_text("Finding attacks where the clip has no markers: higher finds softer ones")
                    .accessible_name("Attack sensitivity");
                    ui.end_row();
                });
                if let Some(m) = &s.message {
                    ui.colored_label(ui.visuals().warn_fg_color, m);
                }
                apply = ui.button("Quantise").clicked();
            });
        if apply {
            self.quantise_audio();
        }
        if !open {
            self.stretch.open = None;
        }
    }

    fn tempo_from_window(&mut self, ctx: &egui::Context) {
        let may_edit = self.may_edit_song() && self.has_stretch_clips();
        let has_markers = {
            let selected = &self.timeline.selected;
            (self.tracks.iter())
                .flat_map(|t| &t.clips)
                .any(|c| selected.contains(&c.id) && !c.markers.is_empty())
        };
        let mut open = true;
        let mut apply = false;
        let s = &mut self.stretch;
        egui::Window::new("Tempo from performance")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Makes the song's tempo follow the selected clip, as it was played: a \
                     tempo change on every beat, replacing the tempo changes there are. \
                     Other clips that follow tempo move with it.",
                );
                if !may_edit {
                    ui.label(RichText::new("Select an audio clip first.").weak());
                    return;
                }
                let t = &mut s.tempo_from;
                egui::Grid::new("tempo-from-clip")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Each attack is");
                        egui::ComboBox::from_id_salt("tempo-from-unit")
                            .selected_text(t.unit.label())
                            .show_ui(ui, |ui| {
                                for u in Unit::ALL {
                                    ui.selectable_value(&mut t.unit, u, u.label());
                                }
                            })
                            .response
                            .accessible_name("What each attack counts as");
                        ui.end_row();
                        if has_markers {
                            ui.label("Beats from");
                            ui.checkbox(&mut t.use_markers, "The clip's stretch markers")
                                .on_hover_text("Off: the attacks found in the audio");
                            ui.end_row();
                        }
                        ui.label("Sensitivity");
                        ui.add(
                            egui::Slider::new(&mut t.sensitivity, 0.0..=1.0)
                                .step_by(0.05)
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                        )
                        .on_hover_text("Higher finds softer attacks too")
                        .accessible_name("Tempo attack sensitivity");
                        ui.end_row();
                    });
                if let Some(m) = &s.message {
                    ui.colored_label(ui.visuals().warn_fg_color, m);
                }
                apply = ui.button("Set the song's tempo").clicked();
            });
        if apply {
            self.tempo_from_clip();
        }
        if !open {
            self.stretch.open = None;
        }
    }

    /// Runs a stretch command; false if `cmd` isn't one.
    pub(crate) fn run_stretch(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::FollowTempo => self.toggle_follow_tempo(),
            Cmd::ClipTempo => {
                self.stretch.tempo_text.clear();
                self.stretch.message = None;
                self.stretch.open = Some(Window::ClipTempo);
            }
            Cmd::StretchMarkersFromAttacks => self.markers_from_attacks(),
            Cmd::AddStretchMarker => self.add_stretch_marker(),
            Cmd::ClearStretchMarkers => self.clear_stretch_markers(),
            Cmd::QuantiseAudio => {
                self.stretch.message = None;
                self.stretch.open = Some(Window::Quantise);
            }
            Cmd::QuantiseAudioApply => self.quantise_audio(),
            Cmd::TempoFromClip => {
                self.stretch.message = None;
                self.stretch.open = Some(Window::TempoFrom);
            }
            Cmd::TempoFromClipApply => self.tempo_from_clip(),
            Cmd::BeatGrid => self.find_beat_grid(),
            Cmd::AnchorToBeat => self.anchor_to_grid(false),
            Cmd::AnchorToBar => self.anchor_to_grid(true),
            _ => return false,
        }
        true
    }
}

impl Stretch {
    /// The attack sensitivity in use: the default until it is set.
    fn sensitivity(&self) -> f32 {
        if self.sensitivity > 0.0 {
            self.sensitivity
        } else {
            0.5
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempo;

    const RATE: u32 = 48_000;

    fn map() -> TempoMap {
        tempo::map(RATE, 120.0, 4, &tempo::Changes::default())
    }

    #[test]
    fn a_steady_performance_gives_one_tempo_and_a_lead_in() {
        // Beats every 0.4 s (150 BPM), the first at 2.0 s: five beats of
        // lead-in, so the song starts at 150 BPM too.
        let hits: Vec<i64> = (0..8)
            .map(|i| (RATE as f64 * (2.0 + 0.4 * i as f64)) as i64)
            .collect();
        let (bpm, changes, shift) = tempo_map_from(&map(), RATE, &hits, Unit::Beat).unwrap();
        assert!((bpm - 150.0).abs() < 0.01, "{bpm}");
        assert_eq!(shift, 0);
        assert_eq!(changes.len(), 7);
        assert_eq!((changes[0].bar, changes[0].beat), (2, 2));
        assert!(changes.iter().all(|c| (c.bpm - 150.0).abs() < 0.01));
    }

    #[test]
    fn a_performance_at_the_very_start_is_moved_a_bar_later() {
        let hits: Vec<i64> = (0..4).map(|i| 1_000 + i * RATE as i64 / 2).collect();
        let (bpm, changes, shift) = tempo_map_from(&map(), RATE, &hits, Unit::Beat).unwrap();
        // Four beats of 0.5 s at 120 BPM, less the 1,000 frames it starts in.
        assert_eq!(shift, 4 * RATE as i64 / 2 - 1_000);
        assert!((bpm - 120.0).abs() < 0.01, "{bpm}");
        assert_eq!((changes[0].bar, changes[0].beat), (2, 1));
    }

    #[test]
    fn a_speeding_up_performance_gets_a_change_per_beat_and_bars_count_too() {
        let mut t = 2.0;
        let mut hits = Vec::new();
        for i in 0..6 {
            hits.push((RATE as f64 * t) as i64);
            t += 0.5 - 0.02 * i as f64;
        }
        let (_, changes, _) = tempo_map_from(&map(), RATE, &hits, Unit::Beat).unwrap();
        assert!(
            changes.windows(2).all(|w| w[1].bpm > w[0].bpm),
            "{changes:?}"
        );
        // Bars of 2 s from 4 s in: two bars of lead-in at 120 BPM.
        let bars: Vec<i64> = (0..3).map(|i| (4 + 2 * i) * RATE as i64).collect();
        let (bpm, bars, _) = tempo_map_from(&map(), RATE, &bars, Unit::Bar).unwrap();
        assert!((bpm - 120.0).abs() < 0.01, "{bpm}");
        assert_eq!((bars[0].bar, bars[0].beat), (3, 1));
        assert!(tempo_map_from(&map(), RATE, &hits[..1], Unit::Beat).is_err());
        assert!(
            tempo_map_from(&map(), RATE, &[0, 10], Unit::Beat).is_err(),
            "too fast"
        );
    }

    mod in_the_app {
        use serde_json::json;

        use crate::Track;
        use crate::agent::driver::Driver;
        use crate::timing::tests::take;
        use daw_engine::time::PPQ;

        /// A track holding `c`, selected, in a running app.
        fn with_clip(d: &mut Driver, c: crate::timeline::Clip) -> String {
            let app = d.app();
            app.tracks.push(Track::new("Guitar".into()));
            app.tracks[0].clips.push(c);
            app.rebuild();
            let key = d.project()["tracks"][0]["clips"][0]["id"]
                .as_str()
                .unwrap()
                .to_string();
            d.call("select_clips", json!({"clips": [key.clone()]}))
                .unwrap();
            key
        }

        /// Where the first clip starts now, in seconds.
        fn start_of(d: &mut Driver) -> f64 {
            let c = d.project()["tracks"][0]["clips"][0].clone();
            c["start"].as_f64().unwrap() / d.app().rate as f64
        }

        #[test]
        fn a_point_of_a_clip_is_put_on_a_bar_a_beat_or_a_time() {
            let mut d = Driver::new();
            let key = with_clip(&mut d, take(96_000, &[14_400, 48_000]));
            let edit = |d: &mut Driver, args: serde_json::Value| {
                let mut a = args;
                a["clip"] = json!(key.clone());
                d.call("edit_clip", a)
            };
            edit(&mut d, json!({"seconds": 1.0})).unwrap();
            // The point 0.3 s into it onto bar 3: 4 s in, at 120 bpm.
            let v = edit(&mut d, json!({"anchor_seconds": 1.3, "bar": 3})).unwrap();
            assert!((start_of(&mut d) - 3.7).abs() < 1e-4, "{v}");
            assert!(
                (v["moved_seconds"].as_f64().unwrap() - 2.7).abs() < 1e-4,
                "{v}"
            );
            assert!(
                (v["anchor_now_seconds"].as_f64().unwrap() - 4.0).abs() < 1e-4,
                "{v}"
            );
            // 0.2 s into it to the nearest beat, then 0.6 s in to the
            // nearest bar line, then to a time.
            edit(&mut d, json!({"anchor_seconds": 3.9, "snap": "beat"})).unwrap();
            assert!((start_of(&mut d) - 3.8).abs() < 1e-4);
            edit(&mut d, json!({"anchor_seconds": 4.4, "snap": "bar"})).unwrap();
            assert!((start_of(&mut d) - 3.4).abs() < 1e-4);
            edit(&mut d, json!({"anchor_seconds": 3.5, "seconds": 10.0})).unwrap();
            assert!((start_of(&mut d) - 9.9).abs() < 1e-4);
            // One undo step each.
            d.command("undo");
            assert!((start_of(&mut d) - 3.4).abs() < 1e-4);

            let err = |d: &mut Driver, args| edit(d, args).unwrap_err();
            assert!(
                err(&mut d, json!({"anchor_seconds": 1.0, "bar": 2})).contains("isn't in the clip")
            );
            assert!(err(&mut d, json!({"snap": "bar"})).contains("anchor_seconds"));
            assert!(err(&mut d, json!({"anchor_seconds": 4.0})).contains("Say where"));
            assert!(err(&mut d, json!({"anchor_seconds": 4.0, "snap": "half"})).contains("snap"));
            assert!(
                err(
                    &mut d,
                    json!({"anchor_seconds": 4.0, "snap": "bar", "bar": 2})
                )
                .contains("not both")
            );
            assert!(
                err(&mut d, json!({"anchor_seconds": 4.4, "seconds": 0.5}))
                    .contains("before the song")
            );
            assert!(
                (start_of(&mut d) - 3.4).abs() < 1e-4,
                "a refused move changes nothing"
            );
        }

        #[test]
        fn a_clip_that_follows_tempo_is_put_on_a_bar_by_a_point_in_beats() {
            let mut d = Driver::new();
            let key = with_clip(&mut d, take(96_000, &[14_400, 48_000]));
            d.command("follow-tempo");
            d.call("edit_clip", json!({"clip": key, "bar": 2})).unwrap();
            // Half a beat in (0.25 s at 120) onto bar 4 beat 2.
            d.call(
                "edit_clip",
                json!({"clip": key, "anchor_seconds": 2.25, "bar": 4.25}),
            )
            .unwrap();
            let c = d.project()["tracks"][0]["clips"][0].clone();
            assert_eq!(c["beat"], json!(13 * PPQ - PPQ / 2), "{c}");
            // The song slows: the point stays on bar 4 beat 2.
            d.call("set_tempo", json!({"bpm": 60})).unwrap();
            let start = start_of(&mut d);
            assert!((start - (13.0 - 0.5)).abs() < 1e-3, "{start}");
        }

        #[test]
        fn the_point_at_the_playhead_is_lined_up_from_the_timeline() {
            let mut d = Driver::new();
            let key = with_clip(&mut d, take(96_000, &[14_400, 48_000]));
            d.call("edit_clip", json!({"clip": key, "seconds": 1.1}))
                .unwrap();
            // The playhead outside the clip: nothing moves.
            d.call("seek", json!({"seconds": 0.5})).unwrap();
            d.command("anchor-to-beat");
            assert!((start_of(&mut d) - 1.1).abs() < 1e-4);
            // 0.3 s in, at 1.4 s: the nearest beat is 1.5 s, the nearest
            // bar 2 s.
            d.call("seek", json!({"seconds": 1.4})).unwrap();
            d.command("anchor-to-beat");
            assert!((start_of(&mut d) - 1.2).abs() < 1e-4);
            d.call("seek", json!({"seconds": 1.6})).unwrap();
            d.command("anchor-to-bar");
            assert!((start_of(&mut d) - 1.6).abs() < 1e-4);
            d.command("undo");
            assert!((start_of(&mut d) - 1.2).abs() < 1e-4);
        }

        #[test]
        fn a_clip_follows_tempo_and_its_attacks_are_quantised_through_the_commands() {
            let mut d = Driver::new();
            // Eighths at 120 BPM are 12,000 frames apart: played up to 40 ms
            // early or late.
            with_clip(&mut d, take(60_000, &[13_900, 22_500, 36_700, 46_100]));
            let clip = |d: &mut Driver| d.project()["tracks"][0]["clips"][0].clone();
            assert_eq!(clip(&mut d)["tempo"], json!(null));
            d.command("follow-tempo");
            assert_eq!(clip(&mut d)["tempo"], json!(120.0));
            assert_eq!(clip(&mut d)["beat"], json!(0));
            d.command("stretch-markers-from-attacks");
            let c = clip(&mut d);
            assert_eq!(c["markers"].as_array().unwrap().len(), 4, "{c}");
            // Half-beat grid, all the way, within 60 ms: every attack lands.
            d.app().stretch.quantise = super::Quantise {
                grid: crate::timeline::Grid::Part(2),
                strength: 1.0,
                window_ms: 60.0,
            };
            d.command("quantise-audio-apply");
            let c = clip(&mut d);
            let ats: Vec<i64> = (c["markers"].as_array().unwrap().iter())
                .map(|m| m["at"].as_i64().unwrap())
                .collect();
            assert_eq!(ats, vec![480, 960, 1440, 1920], "{c}");
            assert_eq!(c["len"], json!(60_000), "the audio is all there");
            // Half way, only within 30 ms: the two attacks 40 ms out stay.
            d.command("clear-stretch-markers");
            assert!(clip(&mut d)["markers"].as_array().unwrap().is_empty());
            d.app().stretch.quantise = super::Quantise {
                grid: crate::timeline::Grid::Part(2),
                strength: 0.5,
                window_ms: 35.0,
            };
            d.command("quantise-audio-apply");
            let c = clip(&mut d);
            let ats: Vec<i64> = (c["markers"].as_array().unwrap().iter())
                .map(|m| m["at"].as_i64().unwrap())
                .collect();
            assert_eq!(ats.len(), 4);
            // 22,500 frames is 900 ticks: half way to 960 is 930. 36,700 is
            // 1,468: half way to 1,440 is 1,454. The rest are left.
            assert!(
                (ats[1] - 930).abs() <= 4 && (ats[2] - 1454).abs() <= 4,
                "{ats:?}"
            );
            assert!(
                (ats[0] - 556).abs() <= 4 && (ats[3] - 1844).abs() <= 4,
                "{ats:?}"
            );
            // One step of undo takes the quantise back.
            d.command("undo");
            assert!(clip(&mut d)["markers"].as_array().unwrap().is_empty());
            d.command("follow-tempo");
            assert_eq!(clip(&mut d)["tempo"], json!(null));
        }

        #[test]
        fn the_songs_tempo_follows_a_free_performance() {
            let mut d = Driver::new();
            // Beats at 20,000, 39,000, 58,000 and 76,000 frames: 151.6 BPM
            // slipping to 160, after one beat of lead-in (144 BPM).
            with_clip(&mut d, take(96_000, &[20_000, 39_000, 58_000, 76_000]));
            d.command("tempo-from-clip-apply");
            let p = d.project();
            let bpm = p["tempo_bpm"].as_f64().unwrap();
            assert!((bpm - 144.0).abs() < 1.0, "{bpm}");
            let changes = p["tempo_changes"].as_array().unwrap();
            assert_eq!(changes.len(), 3, "{changes:?}");
            assert_eq!(
                (changes[0]["bar"].clone(), changes[0]["beat"].clone()),
                (json!(1), json!(2))
            );
            assert!((changes[2]["bpm"].as_f64().unwrap() - 160.0).abs() < 1.0);
            assert_eq!(
                p["tracks"][0]["clips"][0]["start"],
                json!(0),
                "the clip stays"
            );
            // The beats now fall on the grid.
            let map = d.app().tempo_map();
            for (i, f) in [20_000, 39_000, 58_000, 76_000].iter().enumerate() {
                let beat = map
                    .frames_at(daw_engine::time::Ticks(PPQ * (i as i64 + 1)))
                    .0;
                assert!((beat - f).abs() <= 60, "beat {i} at {beat}, played at {f}");
            }
        }

        #[test]
        fn a_following_clip_moves_with_a_tempo_change_and_keeps_playing_offline() {
            let mut d = Driver::new();
            with_clip(&mut d, take(48_000, &[1_000, 25_000]));
            d.app().timeline.selected.clear();
            let app = d.app();
            let mut late = take(48_000, &[1_000]);
            late.start = 96_000;
            app.tracks[0].clips.push(late);
            app.rebuild();
            let key = d.project()["tracks"][0]["clips"][1]["id"]
                .as_str()
                .unwrap()
                .to_string();
            d.call("select_clips", json!({"clips": [key]})).unwrap();
            d.command("follow-tempo");
            assert_eq!(d.project()["tracks"][0]["clips"][1]["beat"], json!(PPQ * 4));
            // The song slows to 60 BPM: bar 2 is twice as far in.
            let app = d.app();
            app.tempo_bpm = 60.0;
            app.time_changed();
            let c = d.project()["tracks"][0]["clips"][1].clone();
            assert_eq!(c["start"], json!(192_000), "{c}");
            assert_eq!(c["beat"], json!(PPQ * 4));
            assert_eq!(c["len"], json!(48_000));
            let app = d.app();
            assert_eq!(app.tracks[0].clips[1].span, 96_000, "twice as long");
            // Export renders through the same warp.
            let s = app.session();
            let mix = daw_engine::engine::render_offline(
                s.compile(256).unwrap(),
                daw_engine::time::Frames(150_000),
                50_000,
                256,
            );
            let loud = |from: usize, to: usize| {
                mix[0][from - 150_000..to - 150_000]
                    .iter()
                    .any(|x| x.abs() > 0.05)
            };
            assert!(loud(193_000, 200_000), "the attack at the clip's start");
            assert!(!loud(150_000, 190_000), "nothing before it");
        }
    }
}
