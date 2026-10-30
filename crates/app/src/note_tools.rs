// SPDX-License-Identifier: GPL-3.0-or-later
//! Note editing depth (roadmap 3.1): the tools that work on the note
//! editor's notes (humanise, legato, fit to scale, choosing notes by
//! what they are, groove templates), MIDI learn, and turning controller
//! lanes and per-note expression into what the engine plays.
//!
//! Every tool is a command (see `commands.rs`), so the menus, keys and
//! anything driving the app from outside reach the same thing. The
//! tools act on the clip in the note editor: on its selected notes, or
//! on all of them with none selected.

use std::collections::HashSet;

use daw_engine::engine::Command;
use daw_engine::graph::{Control, ControlSpec, ExprSpec, NoteSpec};
use daw_engine::midi::{Captured, Expression, Kind};
use daw_engine::time::{Frames, TempoMap, Ticks};
use daw_project::noteops::{self, Filter, Humanise, Random};
use daw_project::{
    Controller, Groove, Lane, MidiMap, Note, NoteClip, PPQ, ROOTS, Scale, ScaleKind,
};
use eframe::egui::{self, RichText};

use crate::App;
use crate::automation;

/// How finely lines are turned into steps for the engine, in ticks.
const LINE_STEP: i64 = PPQ / 48;

/// The values along `lane` (points at ticks from `base`) over
/// `[from, to)` in those ticks, as steps placed at `place + tick`, each
/// rounded by `round`, with repeats left out: the value in force at
/// `from` first, then a step wherever it changes.
fn steps<T: PartialEq + Copy>(
    lane: &Lane,
    from: i64,
    to: i64,
    place: i64,
    round: impl Fn(f64) -> T,
) -> Vec<(i64, T)> {
    let mut out: Vec<(i64, T)> = Vec::new();
    let push = |at: i64, v: T, out: &mut Vec<(i64, T)>| {
        if out.last().is_none_or(|l| l.1 != v) {
            out.push((place + at, v));
        }
    };
    let Some(first) = lane.value_at(from) else {
        return out;
    };
    push(from, round(first), &mut out);
    // Through each stretch between points inside the span.
    let mut at = from;
    let corners: Vec<i64> = (lane.points.iter())
        .map(|p| p.at)
        .filter(|&t| t > from && t < to)
        .chain([to])
        .collect();
    for corner in corners {
        // Points sharing a tick jump straight to the later one.
        let flat = lane.value_at(at) == lane.value_at(corner - 1);
        if !flat {
            let mut t = at + LINE_STEP;
            while t < corner {
                if let Some(v) = lane.value_at(t) {
                    push(t, round(v), &mut out);
                }
                t += LINE_STEP;
            }
        }
        if corner < to
            && let Some(v) = lane.value_at(corner)
        {
            push(corner, round(v), &mut out);
        }
        at = corner;
    }
    out
}

/// A note's expression kind as the engine names it.
fn expression_kind(param: &str) -> Option<Expression> {
    match param {
        Note::PITCH => Some(Expression::Tuning),
        Note::PRESSURE => Some(Expression::Pressure),
        Note::TIMBRE => Some(Expression::Timbre),
        _ => None,
    }
}

fn control_of(c: Controller) -> Control {
    match c {
        Controller::Cc(n) => Control::Cc(n),
        Controller::PitchBend => Control::PitchBend,
        Controller::Pressure => Control::Pressure,
    }
}

/// What the engine plays of a track's note clips: the notes that play,
/// with their expression, and the controller lanes, placed on the
/// timeline.
pub(crate) fn specs(clips: &[NoteClip]) -> (Vec<NoteSpec>, Vec<ExprSpec>, Vec<ControlSpec>) {
    let placed = daw_project::Track {
        note_clips: clips.to_vec(),
        ..daw_project::Track::new(String::new(), String::new())
    }
    .placed_notes();
    let mut exprs = Vec::new();
    let notes = (placed.iter())
        .map(|n| {
            let from = exprs.len() as u32;
            for l in n.expression.iter().filter(|l| !l.points.is_empty()) {
                let Some(kind) = expression_kind(&l.param) else {
                    continue;
                };
                // Steps of a hundredth of a semitone, or of the range.
                let k = if kind == Expression::Tuning {
                    100.0
                } else {
                    1000.0
                };
                for (at, v) in steps(l, 0, n.len, n.start, |v| (v * k).round() as i32) {
                    exprs.push(ExprSpec {
                        at: Ticks(at),
                        kind,
                        value: v as f32 / k as f32,
                    });
                }
            }
            // Kinds after one another; each note's run in time order.
            exprs[from as usize..].sort_by_key(|e: &ExprSpec| e.at);
            NoteSpec {
                start: Ticks(n.start),
                len: Ticks(n.len),
                pitch: n.pitch,
                velocity: n.velocity,
                channel: n.channel,
                expr: (from, exprs.len() as u32),
            }
        })
        .collect();
    // Controller lanes: each clip's part of each, joined per controller.
    let mut controls: Vec<(Controller, Vec<(Ticks, i32)>)> = Vec::new();
    for c in clips.iter().filter(|c| !c.muted) {
        for l in &c.controllers {
            let Some(ctl) = Controller::from_param(&l.param) else {
                continue;
            };
            let (lo, hi, _) = ctl.range();
            let origin = c.start - c.offset;
            let pts = steps(l, c.offset, c.offset + c.len, origin, |v| {
                v.clamp(lo, hi).round() as i32
            });
            let i = match controls.iter().position(|(k, _)| *k == ctl) {
                Some(i) => i,
                None => {
                    controls.push((ctl, Vec::new()));
                    controls.len() - 1
                }
            };
            controls[i]
                .1
                .extend(pts.into_iter().map(|(t, v)| (Ticks(t), v)));
        }
    }
    let controls = controls
        .into_iter()
        .map(|(ctl, mut points)| {
            points.sort_by_key(|p| p.0);
            ControlSpec {
                channel: 0,
                control: control_of(ctl),
                points: points.into(),
            }
        })
        .collect();
    (notes, exprs, controls)
}

/// Which notes "Select notes" picks, and what it does with them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectMode {
    /// The matching notes become the selection.
    #[default]
    Replace,
    /// They are added to it.
    Add,
    /// Only the selected notes that match stay selected.
    Narrow,
}

/// The tools' settings, and their windows.
pub struct NoteTools {
    pub humanise: Humanise,
    /// Legato: ticks left between notes (negative overlaps them).
    pub legato_gap: i64,
    /// Drawn and moved notes stay on the clip's scale.
    pub scale_mode: bool,
    /// "Select notes": the window, its ranges as typed, and the mode.
    pub select_open: bool,
    pub ranges: Ranges,
    pub select_mode: SelectMode,
    /// The groove applied, by id, and how far, in percent.
    pub groove: Option<String>,
    pub groove_timing: f64,
    pub groove_accent: f64,
    /// MIDI learn: the window, the track and setting picked, and whether
    /// the next control moved is tied to it.
    pub learn_open: bool,
    pub learn_track: Option<String>,
    pub learn_param: String,
    pub learning: bool,
    /// Seeds humanising, one after another.
    seed: u64,
}

impl Default for NoteTools {
    fn default() -> NoteTools {
        NoteTools {
            humanise: Humanise::default(),
            legato_gap: 0,
            scale_mode: false,
            select_open: false,
            ranges: Ranges::default(),
            select_mode: SelectMode::Replace,
            groove: None,
            groove_timing: 100.0,
            groove_accent: 50.0,
            learn_open: false,
            learn_track: None,
            learn_param: Lane::GAIN.into(),
            learning: false,
            seed: 0x5eed,
        }
    }
}

/// The "Select notes" ranges, each on or off.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ranges {
    /// Bars, from 1, inclusive.
    pub bars: Option<(i64, i64)>,
    pub pitch: Option<(u8, u8)>,
    pub velocity: Option<(u8, u8)>,
    /// Percent.
    pub chance: Option<(f64, f64)>,
    /// In sixteenths.
    pub length: Option<(f64, f64)>,
    /// Only notes outside the clip's scale.
    pub off_scale: bool,
}

impl Ranges {
    fn filter(&self, map: &TempoMap, scale: Option<Scale>) -> Filter {
        Filter {
            time: self
                .bars
                .map(|(a, b)| (map.bar_start(a.max(1)).0, map.bar_start(b.max(a) + 1).0 - 1)),
            pitch: self.pitch,
            velocity: self.velocity,
            chance: self.chance.map(|(a, b)| (a / 100.0, b / 100.0)),
            length: self.length.map(|(a, b)| {
                (
                    (a * PPQ as f64 / 4.0).floor() as i64,
                    (b * PPQ as f64 / 4.0).ceil() as i64,
                )
            }),
            off_scale: scale.filter(|_| self.off_scale),
        }
    }
}

impl App {
    /// The clip in the note editor, if there is one.
    pub(crate) fn editing_clip(&self) -> Option<&NoteClip> {
        let key = self.notes_editor.clip.as_deref()?;
        (self.tracks.iter())
            .flat_map(|t| t.note_clips.iter())
            .find(|c| c.id == key)
    }

    fn editing_clip_mut(&mut self) -> Option<&mut NoteClip> {
        let key = self.notes_editor.clip.clone()?;
        (self.tracks.iter_mut())
            .flat_map(|t| t.note_clips.iter_mut())
            .find(|c| c.id == key)
    }

    /// The scale the clip in the note editor is in: its own, else the
    /// song's.
    pub(crate) fn editing_scale(&self) -> Option<Scale> {
        self.editing_clip().and_then(|c| c.scale).or(self.scale)
    }

    /// Runs `f` on the note editor's clip with a test of which notes it
    /// acts on (the selection, or all), and rebuilds if it changed
    /// anything. Says so when there is no clip.
    fn with_notes(
        &mut self,
        f: impl FnOnce(&mut NoteClip, &dyn Fn(&Note) -> bool, &TempoMap) -> bool,
    ) -> bool {
        let map = self.tempo_map();
        let picked = self.notes_editor.selected.clone();
        let Some(clip) = self.editing_clip_mut() else {
            self.message = Some("Open a note clip in the note editor first.".into());
            return false;
        };
        let pick = |n: &Note| picked.is_empty() || picked.contains(&n.id);
        let changed = f(clip, &pick, &map);
        if changed {
            clip.sort_notes();
            self.rebuild();
        }
        changed
    }

    pub(crate) fn humanise_notes(&mut self) -> bool {
        let h = self.note_tools.humanise;
        self.note_tools.seed = self.note_tools.seed.wrapping_add(1);
        let mut rng = Random::new(self.note_tools.seed ^ daw_project::new_peer());
        self.with_notes(|c, pick, _| noteops::humanise(&mut c.notes, pick, h, &mut rng))
    }

    pub(crate) fn legato_notes(&mut self) -> bool {
        let gap = self.note_tools.legato_gap;
        self.with_notes(|c, pick, _| noteops::legato(&mut c.notes, pick, gap))
    }

    pub(crate) fn quantise_notes(&mut self) -> bool {
        let map = self.tempo_map();
        let Some(key) = self.notes_editor.clip.clone() else {
            self.message = Some("Open a note clip in the note editor first.".into());
            return false;
        };
        let editor = &self.notes_editor;
        let changed = (self.tracks.iter_mut())
            .flat_map(|t| t.note_clips.iter_mut())
            .find(|c| c.id == key)
            .is_some_and(|c| editor.quantise(c, &map));
        if changed {
            self.rebuild();
        }
        changed
    }

    pub(crate) fn fit_notes_to_scale(&mut self) -> bool {
        let Some(scale) = self.editing_scale() else {
            self.message = Some("Set a key and scale for the song or the clip first.".into());
            return false;
        };
        self.with_notes(|c, pick, _| noteops::fit_to_scale(&mut c.notes, pick, scale))
    }

    /// Selects the note editor's notes that match the "Select notes"
    /// ranges, as its mode says. Returns how many are selected.
    pub(crate) fn select_notes(&mut self) -> usize {
        let map = self.tempo_map();
        let scale = self.editing_scale();
        let filter = self.note_tools.ranges.filter(&map, scale);
        let mode = self.note_tools.select_mode;
        let Some(clip) = self.editing_clip() else {
            return 0;
        };
        let origin = clip.start - clip.offset;
        let matching: HashSet<String> = (clip.notes.iter())
            .filter(|n| filter.matches(n, origin))
            .map(|n| n.id.clone())
            .collect();
        let sel = &mut self.notes_editor.selected;
        match mode {
            SelectMode::Replace => *sel = matching,
            SelectMode::Add => sel.extend(matching),
            SelectMode::Narrow => sel.retain(|id| matching.contains(id)),
        }
        sel.len()
    }

    /// The start of the bar a timeline tick is in.
    fn bar_starts(map: &TempoMap) -> impl Fn(i64) -> i64 + '_ {
        move |t| map.bar_start(map.bar_beat(Ticks(t)).bar).0
    }

    /// The grid step and how many steps a groove taken here has: the
    /// note editor's grid over a bar.
    fn groove_grid(&self, map: &TempoMap, at: i64) -> (i64, usize) {
        let step = self.notes_editor.grid.ticks(map, at).max(1);
        let bar = map.signature_at(Ticks(at)).bar_ticks();
        (step, (bar / step).max(1) as usize)
    }

    fn add_groove(&mut self, name: String, step: i64, steps: Vec<daw_project::GrooveStep>) {
        let g = Groove {
            id: daw_project::new_id(),
            name,
            step,
            steps,
        };
        self.message = Some(format!("Made the groove \"{}\".", g.name));
        self.note_tools.groove = Some(g.id.clone());
        self.grooves.push(g);
    }

    /// A groove from the note editor's notes (the selection, or all).
    pub(crate) fn groove_from_notes(&mut self) -> bool {
        let map = self.tempo_map();
        let Some(clip) = self.editing_clip() else {
            self.message = Some("Open a note clip in the note editor first.".into());
            return false;
        };
        let origin = clip.start - clip.offset;
        let sel = &self.notes_editor.selected;
        let hits: Vec<(i64, f64)> = (clip.notes.iter())
            .filter(|n| sel.is_empty() || sel.contains(&n.id))
            .map(|n| (origin + n.start, n.velocity as f64))
            .collect();
        if hits.is_empty() {
            self.message = Some("There are no notes to take a groove from.".into());
            return false;
        }
        let name = format!("Groove from {}", clip.name);
        let (step, n) = self.groove_grid(&map, clip.start);
        let steps = noteops::groove_steps(&hits, &Self::bar_starts(&map), step, n);
        self.add_groove(name, step, steps);
        true
    }

    /// A groove from the selected audio clips: where their notes start,
    /// and how loud each starts.
    pub(crate) fn groove_from_audio(&mut self) -> bool {
        let map = self.tempo_map();
        let rate = self.rate;
        let mut hits = Vec::new();
        let mut first: Option<(i64, String)> = None;
        for t in &self.tracks {
            for c in t
                .clips
                .iter()
                .filter(|c| self.timeline.selected.contains(&c.id))
            {
                let samples = match c.source.read_mono(c.source.frames) {
                    Ok(s) => s,
                    Err(e) => {
                        self.message = Some(format!("Couldn't read {}: {e}", c.name()));
                        return false;
                    }
                };
                let src_rate = c.source.rate;
                let window = (src_rate as usize / 50).max(1);
                for at in crate::timing::attacks(&samples, src_rate, 0.5) {
                    if at < c.offset || at >= c.offset + c.len {
                        continue;
                    }
                    let level = samples[at..(at + window).min(samples.len())]
                        .iter()
                        .fold(0f32, |m, v| m.max(v.abs()));
                    let frame = c.start
                        + ((at - c.offset) as f64 * rate as f64 / src_rate.max(1) as f64) as i64;
                    hits.push((map.ticks_at(Frames(frame)).0, level as f64));
                }
                if first.as_ref().is_none_or(|(s, _)| c.start < *s) {
                    first = Some((c.start, c.name()));
                }
            }
        }
        let Some((start, name)) = first else {
            self.message = Some("Select an audio clip on the timeline first.".into());
            return false;
        };
        if hits.is_empty() {
            self.message = Some(format!("No notes were found in {name}."));
            return false;
        }
        let (step, n) = self.groove_grid(&map, map.ticks_at(Frames(start)).0);
        let steps = noteops::groove_steps(&hits, &Self::bar_starts(&map), step, n);
        self.add_groove(format!("Groove from {name}"), step, steps);
        true
    }

    /// Moves the note editor's notes towards the groove chosen.
    pub(crate) fn apply_groove(&mut self) -> bool {
        let Some(g) = (self.note_tools.groove.as_ref())
            .and_then(|id| self.grooves.iter().find(|g| &g.id == id))
            .cloned()
        else {
            self.message = Some("Make or choose a groove first.".into());
            return false;
        };
        let (timing, accent) = (
            self.note_tools.groove_timing / 100.0,
            self.note_tools.groove_accent / 100.0,
        );
        self.with_notes(|c, pick, map| {
            let origin = c.start - c.offset;
            noteops::apply_groove(
                &mut c.notes,
                pick,
                &g,
                origin,
                &Self::bar_starts(map),
                timing,
                accent,
            )
        })
    }

    /// Controls moved on MIDI devices: tied to a setting while learning,
    /// and setting the settings tied to them. True if a tie was made.
    pub(crate) fn midi_learn(&mut self, captured: &[Captured]) -> bool {
        let mut changed = false;
        for c in captured {
            let Kind::Control { number, value } = c.event.kind else {
                continue;
            };
            let channel = c.event.channel;
            if self.note_tools.learning
                && let Some(track) = self.note_tools.learn_track.clone()
            {
                self.note_tools.learning = false;
                let param = self.note_tools.learn_param.clone();
                // One control, one setting: a control tied before is
                // tied afresh.
                self.midi_maps.retain(|m| {
                    (m.channel, m.cc) != (channel, number)
                        && (&m.track, &m.param) != (&track, &param)
                });
                self.midi_maps.push(MidiMap {
                    id: daw_project::new_id(),
                    channel,
                    cc: number,
                    track,
                    param,
                });
                self.message = Some(format!(
                    "Tied controller {number} (channel {}).",
                    channel + 1
                ));
                changed = true;
                continue;
            }
            let maps: Vec<MidiMap> = (self.midi_maps.iter())
                .filter(|m| (m.channel, m.cc) == (channel, number))
                .cloned()
                .collect();
            for m in maps {
                self.set_from_control(&m, value as f64 / 127.0);
            }
        }
        changed
    }

    /// Sets a tied setting to `k` (0 to 1) of its range, and tells the
    /// engine.
    fn set_from_control(&mut self, m: &MidiMap, k: f64) {
        let Some(i) = self.tracks.iter().position(|t| t.key == m.track) else {
            return;
        };
        let Some(range) = automation::range(&self.tracks[i], &m.param) else {
            return;
        };
        automation::set_control(&mut self.tracks[i], &m.param, range.value(k));
        let mix = self.tracks[i].mix;
        self.send(Command::SetMix { track: i, mix });
        let sends: Vec<(f32, bool)> = (self.sends_of(&self.tracks[i]).iter())
            .map(|(s, _)| (s.gain_db as f32, s.mute))
            .collect();
        for (send, (gain_db, mute)) in sends.into_iter().enumerate() {
            self.send(Command::SetSend {
                track: i,
                send,
                gain_db,
                mute,
            });
        }
    }

    /// The "Select notes" window.
    pub(crate) fn select_notes_window(&mut self, ctx: &egui::Context) {
        if !self.note_tools.select_open {
            return;
        }
        let mut open = true;
        let mut run = false;
        let has_scale = self.editing_scale().is_some();
        egui::Window::new("Select notes")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Choose the note editor's notes by what they are. Each range ticked must hold.",
                );
                let r = &mut self.note_tools.ranges;
                egui::Grid::new("select-notes")
                    .num_columns(4)
                    .show(ui, |ui| {
                        fn row<T: egui::emath::Numeric>(
                            ui: &mut egui::Ui,
                            name: &str,
                            v: &mut Option<(T, T)>,
                            default: (T, T),
                            range: std::ops::RangeInclusive<T>,
                            speed: f64,
                        ) {
                            let mut on = v.is_some();
                            ui.checkbox(&mut on, name);
                            if on && v.is_none() {
                                *v = Some(default);
                            }
                            if !on {
                                *v = None;
                            }
                            let (mut a, mut b) = v.unwrap_or(default);
                            ui.add_enabled(
                                on,
                                egui::DragValue::new(&mut a)
                                    .range(range.clone())
                                    .speed(speed),
                            )
                            .accessible_name(format!("{name} from"));
                            ui.label("to");
                            ui.add_enabled(
                                on,
                                egui::DragValue::new(&mut b).range(range).speed(speed),
                            )
                            .accessible_name(format!("{name} to"));
                            if on {
                                *v = Some((a, b));
                            }
                            ui.end_row();
                        }
                        row(ui, "Bars", &mut r.bars, (1, 4), 1..=9999, 0.1);
                        row(ui, "Pitch", &mut r.pitch, (48, 72), 0..=127, 0.2);
                        row(ui, "Velocity", &mut r.velocity, (1, 64), 1..=127, 0.5);
                        row(
                            ui,
                            "Chance (%)",
                            &mut r.chance,
                            (0.0, 99.0),
                            0.0..=100.0,
                            0.5,
                        );
                        row(
                            ui,
                            "Length (sixteenths)",
                            &mut r.length,
                            (0.0, 1.0),
                            0.0..=256.0,
                            0.05,
                        );
                    });
                if let Some((a, b)) = r.pitch {
                    ui.label(
                        RichText::new(format!(
                            "Pitch {} to {}",
                            Note::pitch_name(a),
                            Note::pitch_name(b)
                        ))
                        .weak(),
                    );
                }
                ui.add_enabled(
                    has_scale,
                    egui::Checkbox::new(&mut r.off_scale, "Only notes off the scale"),
                )
                .on_disabled_hover_text("Set a key and scale for the song or the clip first");
                ui.horizontal(|ui| {
                    let m = &mut self.note_tools.select_mode;
                    ui.radio_value(m, SelectMode::Replace, "New selection");
                    ui.radio_value(m, SelectMode::Add, "Add to it");
                    ui.radio_value(m, SelectMode::Narrow, "Keep only those in it");
                });
                if ui.button("Select").clicked() {
                    run = true;
                }
            });
        if run {
            let n = self.select_notes();
            self.message = Some(format!("{n} notes selected."));
        }
        self.note_tools.select_open = open;
    }

    /// The MIDI learn window: the ties made, and making one.
    pub(crate) fn midi_learn_window(&mut self, ctx: &egui::Context) {
        if !self.note_tools.learn_open {
            self.note_tools.learning = false;
            return;
        }
        let mut open = true;
        let mut remove = None;
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        egui::Window::new("MIDI learn")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Tie a knob, fader or pedal on a MIDI device to a setting: pick the \
                     setting, press Learn, then move the control.",
                );
                if self.midi_maps.is_empty() {
                    ui.label(RichText::new("Nothing is tied yet.").weak());
                }
                egui::Grid::new("midi-maps").num_columns(3).show(ui, |ui| {
                    for (i, m) in self.midi_maps.iter().enumerate() {
                        ui.label(format!("Controller {} (channel {})", m.cc, m.channel + 1));
                        let what = match self.tracks.iter().find(|t| t.key == m.track) {
                            Some(t) => {
                                format!("{}: {}", t.name, automation::name_of(t, &buses, &m.param))
                            }
                            None => "a track that is gone".into(),
                        };
                        ui.label(what);
                        if ui
                            .button("Remove")
                            .accessible_name(format!("Remove the tie for controller {}", m.cc))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
                ui.separator();
                let t = &mut self.note_tools;
                if t.learn_track
                    .as_ref()
                    .is_none_or(|k| !self.tracks.iter().any(|x| &x.key == k))
                {
                    t.learn_track = self.tracks.first().map(|x| x.key.clone());
                }
                let Some(ti) = (t.learn_track.as_ref())
                    .and_then(|k| self.tracks.iter().position(|x| &x.key == k))
                else {
                    ui.label("Add a track first.");
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label("Track");
                    egui::ComboBox::from_id_salt("learn-track")
                        .selected_text(self.tracks[ti].name.as_str())
                        .show_ui(ui, |ui| {
                            for x in &self.tracks {
                                ui.selectable_value(
                                    &mut t.learn_track,
                                    Some(x.key.clone()),
                                    x.name.as_str(),
                                );
                            }
                        })
                        .response
                        .accessible_name("Track to tie");
                    let choices = automation::choices(&self.tracks[ti], &buses);
                    if !choices.iter().any(|c| c.param == t.learn_param) {
                        t.learn_param = Lane::GAIN.into();
                    }
                    let current = (choices.iter())
                        .find(|c| c.param == t.learn_param)
                        .map_or("Volume".to_string(), |c| c.name.clone());
                    egui::ComboBox::from_id_salt("learn-param")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for c in &choices {
                                ui.selectable_value(
                                    &mut t.learn_param,
                                    c.param.clone(),
                                    c.name.as_str(),
                                );
                            }
                        })
                        .response
                        .accessible_name("Setting to tie");
                });
                if t.learning {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Move a control on your MIDI device…");
                        if ui.button("Cancel").clicked() {
                            t.learning = false;
                        }
                    });
                } else if ui.button("Learn").clicked() {
                    t.learning = true;
                }
            });
        if let Some(i) = remove {
            self.midi_maps.remove(i);
        }
        self.note_tools.learn_open = open;
    }

    /// The note editor's second row: the key, scale mode and the tools.
    pub(crate) fn note_tools_bar(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let mut run = None;
        ui.horizontal_wrapped(|ui| {
            // The clip's key, or the song's.
            ui.label("Key");
            let own = self.editing_clip().and_then(|c| c.scale);
            let song = self.scale;
            let shown = own.or(song);
            let mut pick: Option<Option<Option<Scale>>> = None;
            let text = match (own, song) {
                (Some(s), _) => format!("{} (this clip)", s.label()),
                (None, Some(s)) => format!("{} (the song's)", s.label()),
                (None, None) => "None".into(),
            };
            egui::ComboBox::from_id_salt("notes-key")
                .selected_text(text)
                .width(170.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(shown.is_none(), "None").clicked() {
                        pick = Some(None);
                    }
                    if own.is_some() && song.is_some() && ui.button("Use the song's key").clicked()
                    {
                        pick = Some(Some(None));
                    }
                    for root in 0..12u8 {
                        ui.menu_button(ROOTS[root as usize], |ui| {
                            for kind in ScaleKind::ALL {
                                let s = Scale::new(root, kind);
                                if ui.selectable_label(shown == Some(s), s.label()).clicked() {
                                    pick = Some(Some(Some(s)));
                                }
                            }
                        });
                    }
                })
                .response
                .accessible_name("Key and scale");
            if let Some(p) = pick {
                match p {
                    // None: no key anywhere.
                    None => {
                        self.scale = None;
                        if let Some(c) = self.editing_clip_mut() {
                            c.scale = None;
                        }
                    }
                    // The song's key again.
                    Some(None) => {
                        if let Some(c) = self.editing_clip_mut() {
                            c.scale = None;
                        }
                    }
                    // A key: the clip's, if it has its own, else the
                    // song's.
                    Some(Some(s)) => match self.editing_clip_mut() {
                        Some(c) if c.scale.is_some() => c.scale = Some(s),
                        _ => self.scale = Some(s),
                    },
                }
                changed = true;
            }
            if let Some(c) = self.editing_clip_mut()
                && song.is_some()
                && own.is_none()
                && let Some(s) = shown
                && ui
                    .small_button("Own key")
                    .on_hover_text("Give this clip a key of its own, over the song's")
                    .clicked()
            {
                c.scale = Some(s);
                changed = true;
            }
            ui.toggle_value(&mut self.note_tools.scale_mode, "Scale mode")
                .on_hover_text("Keep drawn and moved notes on the key's scale");
            if ui
                .add_enabled(shown.is_some(), egui::Button::new("Fit to scale"))
                .on_hover_text("Move notes off the scale to the nearest note on it")
                .clicked()
            {
                run = Some(crate::commands::Cmd::FitNotesToScale);
            }
            ui.separator();
            let h = &mut self.note_tools.humanise;
            if ui
                .button("Humanise")
                .on_hover_text("Move notes a little, at random, in time, velocity and length")
                .clicked()
            {
                run = Some(crate::commands::Cmd::HumaniseNotes);
            }
            ui.add(
                egui::DragValue::new(&mut h.timing)
                    .range(0..=PPQ / 4)
                    .suffix(" ticks"),
            )
            .accessible_name("Humanise timing")
            .on_hover_text("At most this far early or late (960 ticks a beat)");
            ui.add(
                egui::DragValue::new(&mut h.velocity)
                    .range(0..=64)
                    .prefix("vel ±"),
            )
            .accessible_name("Humanise velocity");
            ui.add(
                egui::DragValue::new(&mut h.length_pct)
                    .range(0.0..=50.0)
                    .prefix("len ±")
                    .suffix("%"),
            )
            .accessible_name("Humanise length");
            ui.separator();
            if ui
                .button("Legato")
                .on_hover_text("Make each note last until the next one starts")
                .clicked()
            {
                run = Some(crate::commands::Cmd::LegatoNotes);
            }
            ui.add(
                egui::DragValue::new(&mut self.note_tools.legato_gap)
                    .range(-PPQ..=PPQ)
                    .prefix("gap ")
                    .suffix(" ticks"),
            )
            .accessible_name("Legato gap")
            .on_hover_text("Room left before the next note; below 0 overlaps them");
            ui.separator();
            if ui
                .button("Select…")
                .on_hover_text("Choose notes by time, pitch, velocity, chance or length")
                .clicked()
            {
                run = Some(crate::commands::Cmd::SelectNotes);
            }
            ui.separator();
            ui.label("Groove");
            let name = (self.note_tools.groove.as_ref())
                .and_then(|id| self.grooves.iter().find(|g| &g.id == id))
                .map_or("none".to_string(), |g| g.name.clone());
            let t = &mut self.note_tools;
            egui::ComboBox::from_id_salt("notes-groove")
                .selected_text(name)
                .width(150.0)
                .show_ui(ui, |ui| {
                    for g in &self.grooves {
                        ui.selectable_value(&mut t.groove, Some(g.id.clone()), g.name.as_str());
                    }
                    ui.separator();
                    if ui.button("From these notes").clicked() {
                        run = Some(crate::commands::Cmd::GrooveFromNotes);
                    }
                    if ui.button("From the selected audio clip").clicked() {
                        run = Some(crate::commands::Cmd::GrooveFromAudio);
                    }
                })
                .response
                .accessible_name("Groove");
            ui.add(
                egui::DragValue::new(&mut t.groove_timing)
                    .range(0.0..=100.0)
                    .prefix("timing ")
                    .suffix("%"),
            )
            .accessible_name("Groove timing");
            ui.add(
                egui::DragValue::new(&mut t.groove_accent)
                    .range(0.0..=100.0)
                    .prefix("accent ")
                    .suffix("%"),
            )
            .accessible_name("Groove accent");
            if ui
                .add_enabled(t.groove.is_some(), egui::Button::new("Apply groove"))
                .clicked()
            {
                run = Some(crate::commands::Cmd::ApplyGroove);
            }
        });
        if changed {
            self.rebuild();
        }
        if let Some(cmd) = run {
            changed |= self.run_note_tool(cmd);
        }
        changed
    }

    /// Runs one of the note tools' commands. True if notes changed.
    pub(crate) fn run_note_tool(&mut self, cmd: crate::commands::Cmd) -> bool {
        use crate::commands::Cmd;
        match cmd {
            Cmd::HumaniseNotes => self.humanise_notes(),
            Cmd::LegatoNotes => self.legato_notes(),
            Cmd::QuantiseNotes => self.quantise_notes(),
            Cmd::FitNotesToScale => self.fit_notes_to_scale(),
            Cmd::ScaleMode => {
                self.note_tools.scale_mode = !self.note_tools.scale_mode;
                false
            }
            Cmd::SelectNotes => {
                self.note_tools.select_open = true;
                false
            }
            Cmd::GrooveFromNotes => self.groove_from_notes(),
            Cmd::GrooveFromAudio => self.groove_from_audio(),
            Cmd::ApplyGroove => self.apply_groove(),
            Cmd::MidiLearn => {
                self.note_tools.learn_open = true;
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_become_steps_where_the_value_changes() {
        let mut l = Lane::new("cc:1");
        l.put(0, 0.0);
        l.put(PPQ, 127.0);
        l.put(PPQ * 2, 127.0);
        let s = steps(&l, 0, PPQ * 3, 1000, |v| v.round() as i32);
        assert_eq!(s.first(), Some(&(1000, 0)));
        assert_eq!(s.last(), Some(&(1000 + PPQ, 127)));
        assert!(s.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
        // From half way: the value there first.
        let s = steps(&l, PPQ / 2, PPQ * 3, 0, |v| v.round() as i32);
        assert_eq!(s[0], (PPQ / 2, 64));
        // A jump: two points on neighbouring ticks.
        let mut j = Lane::new("cc:64");
        j.put(0, 0.0);
        j.put(PPQ - 1, 0.0);
        j.put(PPQ, 127.0);
        let s = steps(&j, 0, PPQ * 2, 0, |v| v.round() as i32);
        assert_eq!(s, vec![(0, 0), (PPQ, 127)]);
    }

    #[test]
    fn clips_give_the_engine_their_lanes_and_expression() {
        let mut c = NoteClip::new("c".into(), "C".into(), PPQ * 4, PPQ * 4);
        c.offset = PPQ;
        c.controller_mut(Controller::Cc(64)).put(0, 127.0);
        c.controller_mut(Controller::Cc(64)).put(PPQ * 3, 127.0);
        c.controller_mut(Controller::Cc(64)).put(PPQ * 3 + 1, 0.0);
        let mut n = Note::new(PPQ * 2, PPQ, 60, 100);
        n.expression_mut(Note::PITCH).put(0, 0.0);
        n.expression_mut(Note::PITCH).put(PPQ, 2.0);
        c.notes.push(n);
        let (notes, exprs, controls) = specs(&[c]);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].start, Ticks(PPQ * 5));
        let (a, b) = notes[0].expr;
        let run = &exprs[a as usize..b as usize];
        assert_eq!(run[0].at, Ticks(PPQ * 5));
        assert!(run.iter().all(|e| e.kind == Expression::Tuning));
        assert!(run.last().unwrap().value > 1.9);
        assert_eq!(controls.len(), 1);
        // The clip's content starts a beat before the clip: the pedal
        // goes down where the clip starts, and up two beats later.
        assert_eq!(
            &controls[0].points[..],
            &[(Ticks(PPQ * 4), 127), (Ticks(PPQ * 6 + 1), 0)]
        );
    }
}

#[cfg(test)]
mod app_tests {
    use std::time::{Duration, Instant};

    use daw_audio_io::SilentInput;
    use daw_engine::midi::Event;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::commands::Cmd;
    use crate::workspace::Pane;
    use crate::{App, Audio};

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1600.0, 900.0])
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

    /// An instrument track with a two-bar clip of eighth notes, open in
    /// the note editor.
    fn with_clip(h: &mut Harness<'_, App>) {
        let app = h.state_mut();
        let mut t = crate::Track::instrument("Keys".into());
        let mut c = NoteClip::new("clip1".into(), "Riff".into(), 0, PPQ * 8);
        for i in 0..16 {
            c.notes.push(Note::new(
                i * PPQ / 2,
                PPQ / 4,
                [60, 61, 64, 66][i as usize % 4],
                90,
            ));
        }
        t.note_clips.push(c);
        if let Some(i) = &t.instrument {
            app.pending_instruments.push((i.id, i.make(app.rate)));
        }
        app.tracks.push(t);
        app.notes_editor.edit("clip1");
        app.workspace.open(Pane::NotesEditor);
        app.rebuild();
        h.run();
    }

    fn notes(h: &Harness<'_, App>) -> Vec<Note> {
        h.state().tracks[0].note_clips[0].notes.clone()
    }

    #[test]
    fn the_note_tools_work_as_commands_on_the_selection_or_all() {
        let mut h = harness();
        with_clip(&mut h);
        let ctx = h.ctx.clone();
        // Legato: each lasts to the next.
        h.state_mut().run(&ctx, Cmd::LegatoNotes);
        assert!(notes(&h)[..15].iter().all(|n| n.len == PPQ / 2));
        // Fit to scale needs a key; then the sharps go.
        h.state_mut().run(&ctx, Cmd::FitNotesToScale);
        assert!(h.state().message.as_deref().unwrap_or("").contains("key"));
        h.state_mut().scale = Some(Scale::new(0, ScaleKind::Major));
        h.state_mut().run(&ctx, Cmd::FitNotesToScale);
        assert!(notes(&h).iter().all(|n| matches!(n.pitch, 60 | 64 | 65)));
        // Select notes: only E, then humanise moves only them.
        {
            let app = h.state_mut();
            app.note_tools.ranges.pitch = Some((64, 64));
            assert_eq!(app.select_notes(), 4);
            app.note_tools.humanise = Humanise {
                timing: 20,
                velocity: 10,
                length_pct: 0.0,
            };
        }
        let before = notes(&h);
        h.state_mut().run(&ctx, Cmd::HumaniseNotes);
        let after = notes(&h);
        for n in &after {
            let b = before.iter().find(|b| b.id == n.id).unwrap();
            if n.pitch != 64 {
                assert_eq!((n.start, n.velocity), (b.start, b.velocity));
            } else {
                assert!((n.start - b.start).abs() <= 20);
            }
        }
        assert!(after.iter().zip(&before).any(|(a, b)| a != b));
        // Quantise (Q) puts them back on the grid.
        h.state_mut().notes_editor.selected.clear();
        h.state_mut().run(&ctx, Cmd::QuantiseNotes);
        assert!(notes(&h).iter().all(|n| n.start % (PPQ / 4) == 0));
        h.run();
    }

    #[test]
    fn a_groove_from_notes_moves_another_clip_and_survives_the_project() {
        let mut h = harness();
        with_clip(&mut h);
        let ctx = h.ctx.clone();
        // Swing the clip by hand: every second eighth late.
        {
            let app = h.state_mut();
            app.notes_editor.grid = crate::notes_editor::Grid::Eighth;
            let c = &mut app.tracks[0].note_clips[0];
            for (i, n) in c.notes.iter_mut().enumerate() {
                if i % 2 == 1 {
                    n.start += 100;
                }
            }
        }
        h.state_mut().run(&ctx, Cmd::GrooveFromNotes);
        assert_eq!(h.state().grooves.len(), 1);
        let g = h.state().grooves[0].clone();
        assert_eq!((g.step, g.steps.len()), (PPQ / 2, 8));
        assert_eq!(g.steps[1].offset, 100);
        // A straight clip takes it.
        {
            let app = h.state_mut();
            let mut c = NoteClip::new("clip2".into(), "Straight".into(), PPQ * 8, PPQ * 4);
            c.notes = (0..8)
                .map(|i| Note::new(i * PPQ / 2, PPQ / 4, 60, 90))
                .collect();
            app.tracks[0].note_clips.push(c);
            app.notes_editor.edit("clip2");
        }
        h.state_mut().run(&ctx, Cmd::ApplyGroove);
        let c2 = &h.state().tracks[0].note_clips[1];
        assert_eq!(c2.notes[1].start, PPQ / 2 + 100);
        // Kept in the project.
        let p = h.state().project();
        assert_eq!(p.grooves, h.state().grooves);
        h.run();
    }

    #[test]
    fn midi_learn_ties_a_control_and_moving_it_sets_the_volume() {
        let mut h = harness();
        with_clip(&mut h);
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::MidiLearn);
        h.run();
        h.get_by_label("Learn").click();
        // A spinner shows while learning, so step rather than settle.
        for _ in 0..3 {
            h.step();
        }
        assert!(h.state().note_tools.learning);
        let cc = |value: u8| Captured {
            port: 0,
            event: Event {
                frame: 0,
                channel: 3,
                kind: Kind::Control { number: 21, value },
            },
            at: 0,
            playing: false,
            recording: false,
        };
        let app = h.state_mut();
        assert!(app.midi_learn(&[cc(0)]));
        assert_eq!(app.midi_maps.len(), 1);
        assert_eq!((app.midi_maps[0].channel, app.midi_maps[0].cc), (3, 21));
        assert_eq!(app.midi_maps[0].param, Lane::GAIN);
        // Full up is the top of the fader; the bottom, its bottom.
        app.midi_learn(&[cc(127)]);
        assert!((app.tracks[0].mix.gain_db - 12.0).abs() < 1e-3);
        app.midi_learn(&[cc(0)]);
        assert!((app.tracks[0].mix.gain_db + 60.0).abs() < 1e-3);
        // Another channel's controller 21 is someone else's.
        app.midi_learn(&[Captured {
            event: Event {
                channel: 4,
                ..cc(127).event
            },
            ..cc(127)
        }]);
        assert!((app.tracks[0].mix.gain_db + 60.0).abs() < 1e-3);
        assert_eq!(app.project().midi_maps, app.midi_maps);
    }

    #[test]
    fn a_controller_lane_drawn_in_the_editor_reaches_the_engine() {
        let mut h = harness();
        with_clip(&mut h);
        h.state_mut().notes_editor.lane =
            crate::notes_editor::LaneView::Controller(Controller::Cc(1));
        // The editor full size, so the lane has room.
        h.state_mut().workspace.full = Some(Pane::NotesEditor);
        h.run();
        // Click in the lane below the grid: a point.
        let grid = h.get_by_label("Note editor grid").rect();
        let at = egui::pos2(grid.left() + 44.0 + 100.0, grid.bottom() - 20.0);
        h.input_mut().events.push(egui::Event::PointerMoved(at));
        h.step();
        for pressed in [true, false] {
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
        h.run();
        let c = &h.state().tracks[0].note_clips[0];
        assert_eq!(c.controllers.len(), 1, "a lane was made");
        assert_eq!(c.controllers[0].param, "cc:1");
        assert_eq!(c.controllers[0].points.len(), 1);
        let v = c.controllers[0].points[0].value;
        assert!(v > 5.0 && v < 60.0, "low in the lane: {v}");
        // The engine is given it.
        let (_, _, controls) = h.state().tracks[0].note_specs();
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].control, Control::Cc(1));
        // Notes in scale mode stay on the scale when drawn.
        let end = Instant::now() + Duration::from_millis(10);
        while Instant::now() < end {
            h.step();
        }
    }
}
