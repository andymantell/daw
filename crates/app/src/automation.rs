// SPDX-License-Identifier: GPL-3.0-or-later
//! Automation lanes (roadmap 2.6): a line for any setting of a track
//! (volume, pan, a send, a built-in effect's setting or a plugin's
//! parameter) that the setting follows as the song plays.
//!
//! Each track has a mode. **Off** leaves every control to the user and
//! ignores the lanes. **Read** plays the lanes. **Touch** writes a lane
//! while a control is held (and from then on, with the lane made if it
//! had none), and goes back to the line when it is let go. **Latch**
//! writes from the first touch until play stops. **Write** writes every
//! lane on the track from the moment play starts, with whatever the
//! controls are set to, until play stops.
//!
//! The engine plays every lane itself (see `daw_engine::automation`):
//! volume, pan, sends, built-in effect settings and plugin parameters
//! (handed to the plugin with each block, in this process or in a helper
//! process), so export follows them all. A plugin's own window is told
//! the value from here each frame while playing, as it has to be told on
//! this thread.
//!
//! Writing works by watching the controls: a control that moves while
//! the song plays in a writing mode is being touched. While a lane is
//! being written the engine plays the control, not the line; when the
//! pass ends the values seen go into the lane over the stretch passed.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use daw_engine::automation::{Curve, LaneSpec, Target};
use daw_engine::engine::Command;
use daw_project::{AutoPoint, Lane, LaneTarget};
use eframe::egui::{self, Align2, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::Track;
use crate::fx::FxSlot;
use crate::timeline::{self, HEADER};

/// A track's automation mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    Off,
    #[default]
    Read,
    Touch,
    Latch,
    Write,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Off, Mode::Read, Mode::Touch, Mode::Latch, Mode::Write];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "Off",
            Mode::Read => "Read",
            Mode::Touch => "Touch",
            Mode::Latch => "Latch",
            Mode::Write => "Write",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Mode::Off => "The lanes are ignored; every control is yours",
            Mode::Read => "The controls follow their lanes",
            Mode::Touch => {
                "A control you hold while playing is written; let go and it goes back to its line"
            }
            Mode::Latch => "A control you touch while playing is written until you stop",
            Mode::Write => {
                "Every lane on the track is written from play to stop, with the controls as they are"
            }
        }
    }

    /// Whether moving a control while playing writes its lane.
    fn writes(self) -> bool {
        matches!(self, Mode::Touch | Mode::Latch | Mode::Write)
    }
}

/// A pass of writing one lane: the values seen since a control was
/// touched, to go into the lane when it ends.
struct Pass {
    track: String,
    param: String,
    from: i64,
    points: Vec<AutoPoint>,
    /// The playhead when the last value was taken.
    last_at: i64,
    /// When the control last moved.
    moved: Instant,
    /// The value last taken.
    value: f64,
}

/// What the automation pane shows, and the passes being written.
#[derive(Default)]
pub struct Automation {
    /// The track whose lanes the pane shows, by key.
    pub track: Option<String>,
    /// The track of the timeline's selection last seen.
    followed: Option<String>,
    /// Each track's mode, by key; a track not here is in Read.
    modes: HashMap<String, Mode>,
    passes: Vec<Pass>,
    /// Lanes playing a value into a control this frame: the control's
    /// own value, to put back once it has been drawn.
    shown: Vec<(String, String, f64)>,
    /// Each control's value last seen, to notice one moving.
    seen: HashMap<(String, String), f64>,
    /// Plugin parameters driven: the value last set, by track and lane.
    driven: HashMap<(String, String), f64>,
    /// Whether the song was playing last frame.
    was_playing: bool,
    /// A point being dragged: the lane and the point.
    drag: Option<(usize, usize)>,
    /// Right-clicked: the lane, the point if one, and the frame.
    menu: Option<(usize, Option<usize>, i64)>,
}

/// A lane row's height in the pane.
const ROW: f32 = 64.0;
/// How near a point the pointer must be to take it, in points.
const GRAB: f32 = 7.0;
/// A plugin parameter that stops moving for this long is let go (its
/// window gives no other sign).
const PLUGIN_RELEASE: Duration = Duration::from_millis(400);
/// Seconds between points while a control is held still.
const HOLD_EVERY: f64 = 0.25;
/// Most plugin parameters offered per plugin.
const MOST_PLUGIN_PARAMS: usize = 64;
/// How far (0 to 1) a plugin parameter may be from its lane before it
/// counts as moved by hand.
const PLUGIN_SLACK: f64 = 1e-3;

/// One setting a lane can be on, as the pane lists it.
struct Choice {
    param: String,
    name: String,
}

/// A setting's range for the pane: lowest, highest, and whether it moves
/// in ratio (frequencies, times).
#[derive(Clone, Copy)]
struct Range {
    min: f64,
    max: f64,
    log: bool,
}

impl Range {
    /// Where `v` sits in the range, 0 to 1.
    fn fraction(self, v: f64) -> f64 {
        let v = v.clamp(self.min, self.max);
        if self.log && self.min > 0.0 {
            (v.ln() - self.min.ln()) / (self.max.ln() - self.min.ln())
        } else {
            (v - self.min) / (self.max - self.min)
        }
    }

    fn value(self, k: f64) -> f64 {
        let k = k.clamp(0.0, 1.0);
        if self.log && self.min > 0.0 {
            (self.min.ln() + (self.max.ln() - self.min.ln()) * k).exp()
        } else {
            self.min + (self.max - self.min) * k
        }
    }
}

const DB: Range = Range {
    min: -60.0,
    max: 12.0,
    log: false,
};
const PAN: Range = Range {
    min: -1.0,
    max: 1.0,
    log: false,
};
const UNIT: Range = Range {
    min: 0.0,
    max: 1.0,
    log: false,
};

impl Automation {
    pub fn mode(&self, track: &str) -> Mode {
        self.modes.get(track).copied().unwrap_or_default()
    }

    pub fn set_mode(&mut self, track: &str, mode: Mode) {
        self.modes.insert(track.to_string(), mode);
    }

    fn writing(&self, track: &str, param: &str) -> bool {
        self.passes
            .iter()
            .any(|p| p.track == track && p.param == param)
    }
}

/// The slot with this key.
fn slot<'a>(t: &'a Track, key: &str) -> Option<&'a FxSlot> {
    t.fx.iter().find(|s| s.key == key)
}

/// A built-in setting's index in its effect, by key.
fn fx_index(s: &FxSlot, key: &str) -> Option<usize> {
    s.builtin()?
        .kind()
        .params()
        .iter()
        .position(|p| p.key == key)
}

/// The value a track's control holds now, if the setting is there.
fn control(t: &Track, param: &str) -> Option<f64> {
    match Lane::new(param).target() {
        LaneTarget::Gain => Some(t.mix.gain_db as f64),
        LaneTarget::Pan => Some(t.mix.pan as f64),
        LaneTarget::Send(bus) => t.sends.iter().find(|s| s.to == bus).map(|s| s.gain_db),
        LaneTarget::Fx { effect, key } => {
            let s = slot(t, effect)?;
            Some(s.builtin()?.get(fx_index(s, key)?))
        }
        LaneTarget::Plugin { effect, id } => slot(t, effect)?.plugin()?.param(id),
        LaneTarget::Unknown => None,
    }
}

/// Sets a track's control.
fn set_control(t: &mut Track, param: &str, v: f64) {
    match Lane::new(param).target() {
        LaneTarget::Gain => t.mix.gain_db = v as f32,
        LaneTarget::Pan => t.mix.pan = v as f32,
        LaneTarget::Send(bus) => {
            if let Some(s) = t.sends.iter_mut().find(|s| s.to == bus) {
                s.gain_db = v;
            }
        }
        LaneTarget::Fx { effect, key } => {
            if let Some(s) = slot(t, effect)
                && let Some(i) = fx_index(s, key)
                && let Some(b) = s.builtin()
            {
                b.set(i, v);
            }
        }
        LaneTarget::Plugin { effect, id } => {
            if let Some(p) = slot(t, effect).and_then(FxSlot::plugin) {
                p.set_param(id, v);
            }
        }
        LaneTarget::Unknown => {}
    }
}

/// The setting's range, if it is there.
fn range(t: &Track, param: &str) -> Option<Range> {
    match Lane::new(param).target() {
        LaneTarget::Gain | LaneTarget::Send(_) => Some(DB),
        LaneTarget::Pan => Some(PAN),
        LaneTarget::Fx { effect, key } => {
            let s = slot(t, effect)?;
            let p = s.builtin()?.kind().params()[fx_index(s, key)?];
            Some(Range {
                min: p.min,
                max: p.max,
                log: p.log && p.min > 0.0,
            })
        }
        LaneTarget::Plugin { .. } => Some(UNIT),
        LaneTarget::Unknown => None,
    }
}

/// Whether the engine plays this lane (else this module does, or no one).
fn engine_target(t: &Track, sends: &[(&daw_project::Send, usize)], param: &str) -> Option<Target> {
    match Lane::new(param).target() {
        LaneTarget::Gain => Some(Target::Gain),
        LaneTarget::Pan => Some(Target::Pan),
        LaneTarget::Send(bus) => sends
            .iter()
            .position(|(s, _)| s.to == bus)
            .map(Target::Send),
        LaneTarget::Fx { effect, key } => {
            let s = slot(t, effect)?;
            Some(Target::Insert {
                id: s.id,
                param: fx_index(s, key)? as u32,
            })
        }
        LaneTarget::Plugin { effect, id } => {
            let s = slot(t, effect)?;
            s.plugin()?;
            Some(Target::Insert {
                id: s.id,
                param: id,
            })
        }
        LaneTarget::Unknown => None,
    }
}

/// The setting's name, with its effect's.
fn name_of(t: &Track, buses: &[(String, String)], param: &str) -> String {
    match Lane::new(param).target() {
        LaneTarget::Gain => "Volume".into(),
        LaneTarget::Pan => "Pan".into(),
        LaneTarget::Send(bus) => {
            let to = buses
                .iter()
                .find(|(k, _)| k == bus)
                .map_or("a missing bus", |(_, n)| n.as_str());
            format!("Send to {to}")
        }
        LaneTarget::Fx { effect, key } => match slot(t, effect) {
            Some(s) => {
                let p = fx_index(s, key).map(|i| s.builtin().map(|b| b.kind().params()[i].name));
                format!("{}: {}", s.name, p.flatten().unwrap_or(key))
            }
            None => format!("Missing effect: {key}"),
        },
        LaneTarget::Plugin { effect, id } => match slot(t, effect) {
            Some(s) => {
                let title = s
                    .plugin()
                    .and_then(|p| p.params().into_iter().find(|p| p.id == id))
                    .map(|p| p.title);
                format!("{}: {}", s.name, title.unwrap_or_else(|| format!("#{id}")))
            }
            None => format!("Missing plugin: #{id}"),
        },
        LaneTarget::Unknown => param.to_string(),
    }
}

/// The setting's value for showing.
fn text_of(t: &Track, param: &str, v: f64) -> String {
    match Lane::new(param).target() {
        LaneTarget::Gain | LaneTarget::Send(_) => format!("{v:+.1} dB"),
        LaneTarget::Pan if v.abs() < 0.005 => "Centre".into(),
        LaneTarget::Pan if v < 0.0 => format!("L {:.0}%", -v * 100.0),
        LaneTarget::Pan => format!("R {:.0}%", v * 100.0),
        LaneTarget::Fx { effect, key } => slot(t, effect)
            .and_then(|s| Some(s.builtin()?.kind().params()[fx_index(s, key)?].text(v)))
            .unwrap_or_else(|| format!("{v:.2}")),
        LaneTarget::Plugin { effect, id } => slot(t, effect)
            .and_then(|s| s.plugin()?.param_text(id, v))
            .unwrap_or_else(|| format!("{:.0}%", v * 100.0)),
        LaneTarget::Unknown => format!("{v:.2}"),
    }
}

/// Every setting of the track a lane can be on, the track's own first.
fn choices(t: &Track, buses: &[(String, String)]) -> Vec<Choice> {
    let mut out = vec![
        Choice {
            param: Lane::GAIN.into(),
            name: "Volume".into(),
        },
        Choice {
            param: Lane::PAN.into(),
            name: "Pan".into(),
        },
    ];
    for s in &t.sends {
        let param = format!("{}{}", Lane::SEND, s.to);
        out.push(Choice {
            name: name_of(t, buses, &param),
            param,
        });
    }
    for s in &t.fx {
        if let Some(b) = s.builtin() {
            for p in b.kind().params() {
                out.push(Choice {
                    param: format!("{}{}:{}", Lane::FX, s.key, p.key),
                    name: format!("{}: {}", s.name, p.name),
                });
            }
        } else if let Some(p) = s.plugin() {
            for p in (p.params().into_iter())
                .filter(|p| p.automatable && !p.bypass && !p.program_change)
                .take(MOST_PLUGIN_PARAMS)
            {
                out.push(Choice {
                    param: format!("{}{}:{}", Lane::PLUGIN, s.key, p.id),
                    name: format!("{}: {}", s.name, p.title),
                });
            }
        }
    }
    out
}

/// Drops points on a straight line between their neighbours, within a
/// small part of the range, so a pass leaves a line, not a thicket.
fn thin(points: &[AutoPoint], range: Range) -> Vec<AutoPoint> {
    let mut out: Vec<AutoPoint> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        if i == 0 || i + 1 == points.len() {
            out.push(*p);
            continue;
        }
        let a = out[out.len() - 1];
        let b = points[i + 1];
        let k = if b.at == a.at {
            0.0
        } else {
            (p.at - a.at) as f64 / (b.at - a.at) as f64
        };
        let on_line =
            range.fraction(a.value) + (range.fraction(b.value) - range.fraction(a.value)) * k;
        if (range.fraction(p.value) - on_line).abs() > 0.004 {
            out.push(*p);
        }
    }
    out
}

impl crate::App {
    /// The engine's lanes for `t`: those the engine plays, in a mode
    /// that plays them, with the ones being written marked live.
    pub(crate) fn lane_specs(&self, t: &Track) -> Vec<LaneSpec> {
        if self.automation.mode(&t.key) == Mode::Off {
            return Vec::new();
        }
        let sends = self.sends_of(t);
        (t.automation.iter())
            .filter(|l| !l.points.is_empty())
            .filter_map(|l| {
                let target = engine_target(t, &sends, &l.param)?;
                let curve = Curve::new(l.points.iter().map(|p| (p.at, p.value as f32)).collect());
                Some(LaneSpec {
                    target,
                    curve: std::sync::Arc::new(curve),
                    live: self.automation.writing(&t.key, &l.param),
                })
            })
            .collect()
    }

    /// Before the controls are drawn: each lane that plays puts its
    /// value at the playhead into its control, so the control shows it.
    /// The controls' own values are kept to put back afterwards.
    pub(crate) fn show_automation(&mut self) {
        let at = self.ctl.status().position().0;
        let mut shown = std::mem::take(&mut self.automation.shown);
        shown.clear();
        for t in &mut self.tracks {
            if self.automation.mode(&t.key) == Mode::Off {
                continue;
            }
            let lanes: Vec<(String, f64)> = (t.automation.iter())
                .filter(|l| !self.automation.writing(&t.key, &l.param))
                .filter_map(|l| Some((l.param.clone(), l.value_at(at)?)))
                .collect();
            for (param, v) in lanes {
                // Plugins' windows are told in `automation_tick`, and
                // show it themselves.
                if param.starts_with(Lane::PLUGIN) {
                    continue;
                }
                let Some(own) = control(t, &param) else {
                    continue;
                };
                shown.push((t.key.clone(), param.clone(), own));
                set_control(t, &param, v);
            }
        }
        self.automation.shown = shown;
    }

    /// After the controls are drawn: controls the user didn't move go
    /// back to their own values; one that moved was touched. Then the
    /// passes being written take their values, plugins follow their
    /// lanes, and passes that are over go into their lanes.
    pub(crate) fn automation_tick(&mut self, ctx: &egui::Context) {
        let playing = self.ctl.status().playing();
        let at = self.ctl.status().position().0;
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        let now = Instant::now();
        let mut touched: Vec<(String, String, f64)> = Vec::new();

        // Shown values back, unless moved.
        let shown = std::mem::take(&mut self.automation.shown);
        for (key, param, own) in shown {
            let Some(t) = self.tracks.iter_mut().find(|t| t.key == key) else {
                continue;
            };
            let Some(lane) = t.automation.iter().find(|l| l.param == param) else {
                continue;
            };
            let displayed = lane.value_at(at).unwrap_or(own);
            let got = control(t, &param).unwrap_or(displayed);
            if (got - displayed).abs() < 1e-9 || !pointer_down {
                set_control(t, &param, own);
            } else {
                touched.push((key, param, got));
            }
        }

        // Controls without a lane that moved under the pointer.
        let mut seen = std::mem::take(&mut self.automation.seen);
        let mut live: HashSet<(String, String)> = HashSet::new();
        for t in &self.tracks {
            let mode = self.automation.mode(&t.key);
            for param in self.watched(t) {
                let plugin = param.starts_with(Lane::PLUGIN);
                let Some(v) = control(t, &param) else {
                    continue;
                };
                let k = (t.key.clone(), param.clone());
                let was = seen.insert(k.clone(), v);
                let has_lane = t.automation.iter().any(|l| l.param == param);
                if plugin && has_lane && mode != Mode::Off {
                    // Shown by this module and set by the engine block by
                    // block: moved if neither where it was shown nor
                    // anywhere the lane has been since (a plugin in a
                    // helper process reports the engine's value).
                    let driven = self.automation.driven.get(&k).copied();
                    let line = (t.automation.iter())
                        .find(|l| l.param == param)
                        .and_then(|l| l.value_at(at));
                    let off_line = match (driven, line) {
                        (Some(d), Some(l)) => {
                            v < d.min(l) - PLUGIN_SLACK || v > d.max(l) + PLUGIN_SLACK
                        }
                        (Some(d), None) => (d - v).abs() > PLUGIN_SLACK,
                        (None, _) => false,
                    };
                    if off_line && !touched.iter().any(|(a, b, _)| (a, b) == (&k.0, &k.1)) {
                        touched.push((k.0.clone(), k.1.clone(), v));
                    }
                } else if !has_lane
                    && pointer_down
                    && was.is_some_and(|w| (w - v).abs() > 1e-9)
                    && !touched.iter().any(|(a, b, _)| (a, b) == (&k.0, &k.1))
                {
                    touched.push((k.0.clone(), k.1.clone(), v));
                }
                live.insert(k);
            }
        }
        seen.retain(|k, _| live.contains(k));
        self.automation.seen = seen;

        let mut changed = false;
        // Write mode: every lane writes from the moment play starts.
        if playing && !self.automation.was_playing {
            let starts: Vec<(String, String, f64)> = (self.tracks.iter())
                .filter(|t| self.automation.mode(&t.key) == Mode::Write)
                .flat_map(|t| {
                    t.automation
                        .iter()
                        .filter_map(|l| {
                            Some((t.key.clone(), l.param.clone(), control(t, &l.param)?))
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            for (key, param, v) in starts {
                self.start_pass(&key, &param, at, v, now);
            }
        }
        // Touches start passes while playing in a writing mode.
        for (key, param, v) in touched {
            let mode = self.automation.mode(&key);
            if !playing || !mode.writes() {
                continue;
            }
            if self.automation.writing(&key, &param) {
                if let Some(p) =
                    (self.automation.passes.iter_mut()).find(|p| p.track == key && p.param == param)
                {
                    p.moved = now;
                }
            } else {
                self.start_pass(&key, &param, at, v, now);
            }
        }

        // Each pass takes the control's value now.
        let mut ended = Vec::new();
        for (i, p) in self.automation.passes.iter_mut().enumerate() {
            let Some(t) = self.tracks.iter().find(|t| t.key == p.track) else {
                ended.push(i);
                continue;
            };
            let mode = self
                .automation
                .modes
                .get(&p.track)
                .copied()
                .unwrap_or_default();
            let jumped = at < p.last_at || at > p.last_at + 2 * self.rate as i64;
            let Some(v) = control(t, &p.param) else {
                ended.push(i);
                continue;
            };
            if v != p.value {
                p.moved = now;
            }
            let held_for = (at - p.last_at) as f64 / self.rate as f64;
            if playing && !jumped && (v != p.value || held_for >= HOLD_EVERY) {
                p.points.push(AutoPoint { at, value: v });
                p.value = v;
                p.last_at = at;
            }
            let plugin = p.param.starts_with(Lane::PLUGIN);
            let let_go = match mode {
                Mode::Touch if plugin => now.duration_since(p.moved) > PLUGIN_RELEASE,
                Mode::Touch => !pointer_down,
                _ => false,
            };
            if !playing || jumped || let_go || !mode.writes() {
                ended.push(i);
            }
        }
        for i in ended.into_iter().rev() {
            let p = self.automation.passes.remove(i);
            changed |= self.end_pass(p, at);
        }

        // Plugins' own windows follow their lanes (the engine sets the
        // plugins themselves, block by block).
        let mut sets: Vec<(usize, String, u32, f64, String)> = Vec::new();
        for (ti, t) in self.tracks.iter().enumerate() {
            if self.automation.mode(&t.key) == Mode::Off {
                continue;
            }
            for l in &t.automation {
                let LaneTarget::Plugin { effect, id } = l.target() else {
                    continue;
                };
                if self.automation.writing(&t.key, &l.param) {
                    continue;
                }
                let Some(v) = l.value_at(at) else { continue };
                let k = (t.key.clone(), l.param.clone());
                if self
                    .automation
                    .driven
                    .get(&k)
                    .is_none_or(|d| (d - v).abs() > 1e-6)
                {
                    sets.push((ti, effect.to_string(), id, v, l.param.clone()));
                }
            }
        }
        for (ti, effect, id, v, param) in sets {
            let t = &self.tracks[ti];
            if let Some(p) = slot(t, &effect).and_then(FxSlot::plugin) {
                p.show_param(id, v);
                self.automation.driven.insert((t.key.clone(), param), v);
            }
        }
        let keep: HashSet<(String, String)> = (self.tracks.iter())
            .flat_map(|t| {
                t.automation
                    .iter()
                    .map(|l| (t.key.clone(), l.param.clone()))
            })
            .collect();
        self.automation.driven.retain(|k, _| keep.contains(k));

        self.automation.was_playing = playing;
        if changed {
            self.rebuild();
        }
        if playing && !self.automation.passes.is_empty() {
            ctx.request_repaint();
        }
    }

    /// The controls watched for a touch on `t`: its own, its sends and
    /// its built-in effects' settings, and the plugin parameters that
    /// have a lane (every parameter of every plugin would be too many
    /// to ask after each frame).
    fn watched(&self, t: &Track) -> Vec<String> {
        if !self.automation.mode(&t.key).writes() {
            return Vec::new();
        }
        let mut out = vec![Lane::GAIN.to_string(), Lane::PAN.to_string()];
        out.extend(t.sends.iter().map(|s| format!("{}{}", Lane::SEND, s.to)));
        for s in &t.fx {
            if let Some(b) = s.builtin() {
                out.extend(
                    b.kind()
                        .params()
                        .iter()
                        .map(|p| format!("{}{}:{}", Lane::FX, s.key, p.key)),
                );
            }
        }
        out.extend(
            t.automation
                .iter()
                .filter(|l| l.param.starts_with(Lane::PLUGIN))
                .map(|l| l.param.clone()),
        );
        out
    }

    fn start_pass(&mut self, key: &str, param: &str, at: i64, v: f64, now: Instant) {
        self.automation.passes.push(Pass {
            track: key.into(),
            param: param.into(),
            from: at,
            points: vec![AutoPoint { at, value: v }],
            last_at: at,
            moved: now,
            value: v,
        });
        self.set_live(key, param, true);
    }

    /// Tells the engine a lane is being written, or played again.
    fn set_live(&mut self, key: &str, param: &str, live: bool) {
        let Some(ti) = self.tracks.iter().position(|t| t.key == key) else {
            return;
        };
        let t = &self.tracks[ti];
        let Some(target) = engine_target(t, &self.sends_of(t), param) else {
            return;
        };
        self.send(Command::SetAutoLive {
            track: ti,
            target,
            live,
        });
    }

    /// Puts a finished pass into its lane. True if a lane changed.
    fn end_pass(&mut self, p: Pass, at: i64) -> bool {
        let key = p.track.clone();
        let param = p.param.clone();
        let Some(t) = self.tracks.iter_mut().find(|t| t.key == key) else {
            return false;
        };
        let Some(range) = range(t, &param) else {
            return false;
        };
        let to = at.max(p.last_at);
        let mut points = p.points;
        // The value held to the end of the pass.
        if points.last().is_some_and(|l| l.at < to) {
            points.push(AutoPoint {
                at: to,
                value: p.value,
            });
        }
        let points = thin(&points, range);
        let lane = match t.automation.iter().position(|l| l.param == param) {
            Some(i) => &mut t.automation[i],
            None => {
                t.automation.push(Lane::new(param.clone()));
                t.automation.last_mut().unwrap()
            }
        };
        lane.write(p.from, to, &points);
        self.automation.driven.remove(&(key.clone(), param.clone()));
        self.set_live(&key, &param, false);
        true
    }

    /// The automation pane: one track's lanes, in line with the
    /// timeline, with its mode and a menu to add lanes.
    pub(crate) fn automation_pane(&mut self, ui: &mut egui::Ui) {
        let rate = self.rate;
        let map = self.tempo_map();
        let playhead = self.ctl.status().position().0;
        let selected = (self.tracks.iter())
            .find(|t| {
                t.clips
                    .iter()
                    .any(|c| self.timeline.selected.contains(&c.id))
            })
            .map(|t| t.key.clone());
        let pane = &mut self.automation;
        if selected.is_some() && selected != pane.followed {
            pane.track = selected.clone();
        }
        pane.followed = selected;
        if !(pane.track.as_ref()).is_some_and(|k| self.tracks.iter().any(|t| &t.key == k)) {
            pane.track = (self.tracks.iter())
                .find(|t| !t.automation.is_empty())
                .or(self.tracks.first())
                .map(|t| t.key.clone());
        }
        let Some(ti) =
            (pane.track.as_ref()).and_then(|k| self.tracks.iter().position(|t| &t.key == k))
        else {
            ui.label("Add a track to automate its settings here.");
            return;
        };
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();

        let mut changed = false;
        let mut add: Option<String> = None;
        let mut remove: Option<usize> = None;
        ui.horizontal(|ui| {
            let mut pick = ti;
            egui::ComboBox::from_id_salt("automation-track")
                .selected_text(&self.tracks[ti].name)
                .show_ui(ui, |ui| {
                    for (i, t) in self.tracks.iter().enumerate() {
                        ui.selectable_value(&mut pick, i, &t.name);
                    }
                })
                .response
                .accessible_name("Track whose automation is shown");
            if pick != ti {
                self.automation.track = Some(self.tracks[pick].key.clone());
            }
            let key = self.tracks[ti].key.clone();
            let mut mode = self.automation.mode(&key);
            let before = mode;
            egui::ComboBox::from_id_salt("automation-mode")
                .selected_text(mode.name())
                .show_ui(ui, |ui| {
                    for m in Mode::ALL {
                        ui.selectable_value(&mut mode, m, m.name())
                            .on_hover_text(m.hint());
                    }
                })
                .response
                .on_hover_text(mode.hint())
                .accessible_name("Automation mode");
            if mode != before {
                self.automation.set_mode(&key, mode);
                changed = true;
            }
            let t = &self.tracks[ti];
            let have: HashSet<&str> = t.automation.iter().map(|l| l.param.as_str()).collect();
            ui.menu_button("+ Lane", |ui| {
                ui.set_min_width(240.0);
                for c in choices(t, &buses) {
                    if ui
                        .add_enabled(!have.contains(c.param.as_str()), egui::Button::new(&c.name))
                        .clicked()
                    {
                        add = Some(c.param);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Add a lane for one of this track's settings")
            .accessible_name("Add automation lane");
            if mode.writes() {
                ui.label(RichText::new("Move a control while playing to write it").weak());
            }
        });
        if let Some(param) = add {
            let t = &mut self.tracks[ti];
            if !t.automation.iter().any(|l| l.param == param) {
                let mut lane = Lane::new(param.clone());
                // Starts level at the control's own value, so adding a
                // lane changes nothing until it is drawn on.
                if let Some(v) = control(t, &param) {
                    lane.put(0, v);
                }
                t.automation.push(lane);
                changed = true;
            }
        }

        let look = crate::theme::look(ui.ctx());
        let tc = look.c().clone();
        let colour = tc.track(ti);
        let (scroll, pps) = self.timeline.x_view();
        let (grid, snapping) = (self.timeline.grid, self.timeline.snap);
        let n = self.tracks[ti].automation.len();
        if n == 0 {
            ui.label(RichText::new("No lanes yet. Add one with + Lane, or move a control while playing in Touch, Latch or Write.").weak());
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let size = vec2(ui.available_width(), n as f32 * ROW);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let left = rect.left() + HEADER;
                let resp = ui.interact(
                    Rect::from_min_max(pos2(left, rect.top()), rect.max),
                    ui.scope_id().with("automation-lanes"),
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
                let body = |i: usize| {
                    Rect::from_min_max(
                        pos2(left, rect.top() + i as f32 * ROW),
                        pos2(rect.right(), rect.top() + (i + 1) as f32 * ROW),
                    )
                };
                let inner = |i: usize| body(i).shrink2(vec2(0.0, 8.0));
                let row_at = |y: f32| {
                    let r = ((y - rect.top()) / ROW).floor();
                    (r >= 0.0 && (r as usize) < n).then_some(r as usize)
                };
                let t = &self.tracks[ti];
                let ranges: Vec<Option<Range>> =
                    t.automation.iter().map(|l| range(t, &l.param)).collect();
                let y_of = |i: usize, v: f64| {
                    let r = inner(i);
                    let k = ranges[i].map_or(0.5, |g| g.fraction(v)) as f32;
                    r.bottom() - k * r.height()
                };
                let value_at_y = |i: usize, y: f32| {
                    let r = inner(i);
                    let k = ((r.bottom() - y) / r.height()).clamp(0.0, 1.0) as f64;
                    ranges[i].map_or(0.0, |g| g.value(k))
                };
                // The point under `p`, if one is near.
                let point_at = |p: egui::Pos2| -> Option<(usize, usize)> {
                    let i = row_at(p.y)?;
                    let l = &t.automation[i];
                    (l.points.iter().enumerate())
                        .map(|(j, q)| (j, (pos2(x_of(q.at), y_of(i, q.value)) - p).length()))
                        .filter(|(_, d)| *d <= GRAB)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(j, _)| (i, j))
                };

                let pos = resp.interact_pointer_pos();
                let mut edit: Option<(usize, Option<usize>, i64, f64)> = None;
                if resp.drag_started()
                    && let Some(p) = pos
                {
                    self.automation.drag = point_at(p);
                }
                if resp.dragged()
                    && let Some((i, j)) = self.automation.drag
                    && let Some(p) = pos
                {
                    let f = if ui.input(|i| i.modifiers.alt) {
                        frame_of(p.x).max(0)
                    } else {
                        snapped(frame_of(p.x))
                    };
                    edit = Some((i, Some(j), f, value_at_y(i, p.y)));
                }
                if resp.drag_stopped() {
                    self.automation.drag = None;
                }
                if resp.clicked()
                    && let Some(p) = pos
                    && let Some(i) = row_at(p.y)
                    && point_at(p).is_none()
                {
                    edit = Some((i, None, snapped(frame_of(p.x)), value_at_y(i, p.y)));
                }
                if resp.secondary_clicked()
                    && let Some(p) = pos
                    && let Some(i) = row_at(p.y)
                {
                    self.automation.menu = Some((i, point_at(p).map(|(_, j)| j), frame_of(p.x)));
                }
                if let Some(h) = resp.hover_pos()
                    && point_at(h).is_some()
                {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }

                let menu = self.automation.menu;
                let mut clear: Option<usize> = None;
                let mut delete: Option<(usize, usize)> = None;
                resp.context_menu(|ui| {
                    let Some((i, point, _)) = menu else {
                        return;
                    };
                    if let Some(j) = point
                        && ui.button("Delete point").clicked()
                    {
                        delete = Some((i, j));
                        ui.close();
                    }
                    if ui.button("Clear the lane").clicked() {
                        clear = Some(i);
                        ui.close();
                    }
                    if ui.button("Remove the lane").clicked() {
                        remove = Some(i);
                        ui.close();
                    }
                });

                // Edits, with the track borrowed again for writing.
                let t = &mut self.tracks[ti];
                if let Some((i, j, f, v)) = edit
                    && let Some(l) = t.automation.get_mut(i)
                {
                    match j {
                        Some(j) if j < l.points.len() => {
                            l.points.remove(j);
                            let k = l.put(f, v);
                            self.automation.drag = Some((i, k));
                        }
                        _ => {
                            l.put(f, v);
                        }
                    }
                    changed = true;
                }
                if let Some((i, j)) = delete
                    && let Some(l) = t.automation.get_mut(i)
                    && j < l.points.len()
                {
                    l.points.remove(j);
                    changed = true;
                }
                if let Some(i) = clear
                    && let Some(l) = t.automation.get_mut(i)
                {
                    l.points.clear();
                    changed = true;
                }

                let t = &self.tracks[ti];
                let painter = ui.painter_at(rect);
                let writing: Vec<bool> = (t.automation.iter())
                    .map(|l| self.automation.writing(&t.key, &l.param))
                    .collect();
                for (i, l) in t.automation.iter().enumerate() {
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
                    // Bar lines.
                    let first = map
                        .bar_beat(map.ticks_at(daw_engine::time::Frames(frame_of(left).max(0))))
                        .bar;
                    for bar in first.max(1).. {
                        let x = x_of(map.frames_at(map.bar_start(bar)).0);
                        if x > rect.right() || bar > first + 400 {
                            break;
                        }
                        if x >= left {
                            painter.line_segment(
                                [pos2(x, b.top()), pos2(x, b.bottom())],
                                Stroke::new(look.line, tc.grid),
                            );
                        }
                    }
                    let name = name_of(t, &buses, &l.param);
                    painter.with_clip_rect(head).text(
                        head.left_top() + vec2(8.0, 8.0),
                        Align2::LEFT_TOP,
                        &name,
                        look.font(look.small),
                        tc.text,
                    );
                    let now = l.value_at(playhead).or_else(|| control(t, &l.param));
                    let state = if writing[i] {
                        "writing".to_string()
                    } else {
                        now.map(|v| text_of(t, &l.param, v)).unwrap_or_default()
                    };
                    painter.with_clip_rect(head).text(
                        head.left_bottom() + vec2(8.0, -8.0),
                        Align2::LEFT_BOTTOM,
                        state,
                        look.font(look.small),
                        if writing[i] { tc.record } else { tc.text_dim },
                    );
                    let x_btn =
                        Rect::from_min_size(head.right_top() + vec2(-26.0, 4.0), vec2(22.0, 20.0));
                    if ui
                        .put(x_btn, egui::Button::new("✕").small())
                        .on_hover_text("Remove this lane")
                        .accessible_name(format!("Remove the {name} lane"))
                        .clicked()
                    {
                        remove = Some(i);
                    }

                    // The line, held level beyond its ends.
                    let line_colour = if writing[i] { tc.record } else { colour };
                    let stroke = Stroke::new(1.5, line_colour);
                    if let (Some(first), Some(last)) = (l.points.first(), l.points.last()) {
                        let mut pts: Vec<egui::Pos2> = Vec::with_capacity(l.points.len() + 2);
                        pts.push(pos2(left, y_of(i, first.value)));
                        pts.extend(l.points.iter().map(|p| pos2(x_of(p.at), y_of(i, p.value))));
                        pts.push(pos2(rect.right(), y_of(i, last.value)));
                        let visible: Vec<egui::Pos2> = pts
                            .iter()
                            .map(|p| pos2(p.x.clamp(left, rect.right()), p.y))
                            .collect();
                        painter.add(egui::Shape::line(visible, stroke));
                        for p in &l.points {
                            let x = x_of(p.at);
                            if x < left - GRAB || x > rect.right() + GRAB {
                                continue;
                            }
                            let c = pos2(x, y_of(i, p.value));
                            painter.circle_filled(c, 4.0, tc.lane);
                            painter.circle_stroke(c, 4.0, stroke);
                        }
                    } else if let Some(v) = control(t, &l.param) {
                        // No points: the control's own value, dotted.
                        let y = y_of(i, v);
                        let mut x = left;
                        while x < rect.right() {
                            painter.line_segment(
                                [pos2(x, y), pos2((x + 4.0).min(rect.right()), y)],
                                Stroke::new(1.0, tc.text_dim),
                            );
                            x += 8.0;
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
                if self.automation.drag.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            });

        if let Some(i) = remove {
            let t = &mut self.tracks[ti];
            if i < t.automation.len() {
                let param = t.automation.remove(i).param;
                self.automation
                    .passes
                    .retain(|p| !(p.track == t.key && p.param == param));
                changed = true;
            }
        }
        if changed {
            self.rebuild();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use daw_audio_io::SilentInput;
    use daw_engine::engine::Command;
    use daw_engine::time::Frames;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::{App, Audio};

    /// The app on the silent stand-in with one track, so the playhead
    /// moves.
    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        let app = h.state_mut();
        let eng = app.replace_engine();
        app.audio = Audio::Silent(
            daw_audio_io::start_silent_with(app.rate, 256, eng, SilentInput::Tones),
            String::new(),
        );
        app.tracks.push(Track::new("Guitar".into()));
        app.rebuild();
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

    fn gain_lane(app: &App) -> Option<&Lane> {
        app.tracks[0]
            .automation
            .iter()
            .find(|l| l.param == Lane::GAIN)
    }

    #[test]
    fn a_lane_reaches_the_engine_and_shows_on_the_fader_without_moving_it() {
        let mut h = harness();
        let app = h.state_mut();
        let mut lane = Lane::new(Lane::GAIN);
        lane.put(0, -20.0);
        lane.put(48_000, -10.0);
        app.tracks[0].automation.push(lane);
        app.tracks[0].mix.gain_db = -3.0;
        app.rebuild();
        let s = app.session();
        assert_eq!(s.tracks[0].automation.len(), 1);
        assert_eq!(s.tracks[0].automation[0].target, Target::Gain);
        assert_eq!(s.tracks[0].automation[0].curve.at(24_000), Some(-15.0));
        h.run();
        // Between frames the fader holds its own value, so the project
        // (and undo) see no change from the lane playing.
        assert_eq!(h.state().tracks[0].mix.gain_db, -3.0);
        assert!(!h.state().proj.dirty || h.state().proj.committed.tracks[0].gain_db == -3.0);

        // Off: the engine gets no lanes.
        let key = h.state().tracks[0].key.clone();
        h.state_mut().automation.set_mode(&key, Mode::Off);
        assert!(h.state().session().tracks[0].automation.is_empty());
    }

    #[test]
    fn a_plugin_lane_goes_to_the_engine_and_shows_on_the_plugin() {
        let plugin = crate::freeze::test_plugin::bundle();
        let class = daw_plugins::scan::scan_here(&plugin)
            .unwrap()
            .into_iter()
            .find(|c| c.is_audio_plugin() && !c.is_instrument())
            .unwrap();
        let mut h = harness();
        let app = h.state_mut();
        let rate = app.rate;
        let effect = daw_project::Effect {
            id: daw_project::new_id(),
            name: class.name.clone(),
            class: class.cid.iter().map(|&b| b as u8).collect(),
            bundle: plugin.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let slot = app.effects.restore(&effect, rate).unwrap();
        let slot_id = slot.id;
        let gain = slot.plugin().unwrap().params()[0].id;
        app.tracks[0].fx.push(slot);
        let mut lane = Lane::new(format!("{}{}:{gain}", Lane::PLUGIN, effect.id));
        lane.put(0, 0.5);
        lane.put(48_000, 0.25);
        app.tracks[0].automation.push(lane);
        app.rebuild();

        // The engine gets the lane as the plugin's insert, so playback
        // and export set the plugin block by block.
        let s = app.session();
        assert_eq!(s.tracks[0].automation.len(), 1);
        assert_eq!(
            s.tracks[0].automation[0].target,
            Target::Insert {
                id: slot_id,
                param: gain
            }
        );
        // Playing, the plugin's own controls are shown the lane's value.
        app.send(Command::Seek(Frames(0)));
        app.send(Command::Play);
        assert!(until(&mut h, 10, |a| a.ctl.status().position().0 > 1000));
        let app = h.state();
        let shown = app.tracks[0].fx[0].plugin().unwrap().param(gain).unwrap();
        let at = app.ctl.status().position().0;
        let line = app.tracks[0].automation[0].value_at(at).unwrap();
        assert!((shown - line).abs() < 0.05, "shown {shown}, lane {line}");
        // In Touch mode, that is not a touch: no pass starts.
        let key = app.tracks[0].key.clone();
        h.state_mut().automation.set_mode(&key, Mode::Touch);
        for _ in 0..10 {
            h.step();
        }
        assert!(h.state().automation.passes.is_empty());
        h.state_mut().send(Command::Stop);
    }

    #[test]
    fn holding_the_fader_while_playing_in_touch_writes_a_lane_and_lets_go() {
        let mut h = harness();
        let key = h.state().tracks[0].key.clone();
        h.state_mut().automation.set_mode(&key, Mode::Touch);
        h.state_mut().toggle_play();
        assert!(until(&mut h, 3, |a| a.ctl.status().position().0 > 4800));

        // Hold the fader and move it along.
        let r = h.get_by_label("Guitar volume").rect();
        let from = r.center();
        h.drag_at(from);
        h.step();
        for i in 1..=8 {
            h.hover_at(from + vec2(i as f32 * 6.0, 0.0));
            h.step();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(
            h.state().automation.writing(&key, Lane::GAIN),
            "a pass is under way while held"
        );
        // While held, the engine plays the fader, not the line.
        let s = h.state().session();
        assert!(s.tracks[0].automation.is_empty() || s.tracks[0].automation[0].live);
        h.drop_at(from + vec2(48.0, 0.0));
        assert!(until(&mut h, 2, |a| !a
            .automation
            .writing(&key, Lane::GAIN)));
        let app = h.state();
        let lane = gain_lane(app).expect("the lane was made");
        assert!(lane.points.len() >= 2, "points: {:?}", lane.points);
        let (first, last) = (lane.points[0], lane.points[lane.points.len() - 1]);
        assert!(first.at > 0 && last.at > first.at);
        assert!(last.value > first.value, "the fader was pushed up");
        // The engine plays it now.
        let s = app.session();
        assert_eq!(s.tracks[0].automation.len(), 1);
        assert!(!s.tracks[0].automation[0].live);
        h.state_mut().toggle_play();
        assert!(until(&mut h, 2, |a| !a.ctl.status().playing()));
    }

    #[test]
    fn latch_keeps_writing_until_play_stops_and_write_mode_writes_every_lane() {
        let mut h = harness();
        let key = h.state().tracks[0].key.clone();
        h.state_mut().automation.set_mode(&key, Mode::Latch);
        h.state_mut().toggle_play();
        assert!(until(&mut h, 3, |a| a.ctl.status().position().0 > 4800));
        let r = h.get_by_label("Guitar volume").rect();
        let from = r.center();
        h.drag_at(from);
        h.step();
        h.hover_at(from + vec2(12.0, 0.0));
        h.step();
        h.drop_at(from + vec2(12.0, 0.0));
        for _ in 0..5 {
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            h.state().automation.writing(&key, Lane::GAIN),
            "latched: still writing after letting go"
        );
        h.state_mut().toggle_play();
        assert!(until(&mut h, 2, |a| !a.ctl.status().playing()
            && !a.automation.writing(&key, Lane::GAIN)));
        let lane = gain_lane(h.state()).expect("the lane was made");
        let stop = h.state().ctl.status().position().0;
        let last = lane.points[lane.points.len() - 1];
        assert!(
            (last.at - stop).abs() <= 4800,
            "held to the stop ({} vs {stop})",
            last.at
        );

        // Write: every lane writes from play, untouched.
        let app = h.state_mut();
        app.automation.set_mode(&key, Mode::Write);
        app.tracks[0].automation.push(Lane::new(Lane::PAN));
        app.tracks[0].mix.pan = 0.5;
        app.send(Command::Seek(Frames(0)));
        app.rebuild();
        h.step();
        h.state_mut().toggle_play();
        assert!(until(&mut h, 3, |a| a.ctl.status().position().0 > 24_000));
        assert!(h.state().automation.writing(&key, Lane::PAN));
        h.state_mut().toggle_play();
        assert!(until(&mut h, 2, |a| !a.ctl.status().playing()
            && !a.automation.writing(&key, Lane::PAN)));
        let pan = (h.state().tracks[0].automation.iter())
            .find(|l| l.param == Lane::PAN)
            .unwrap();
        assert!(!pan.points.is_empty());
        assert!(pan.points.iter().all(|p| (p.value - 0.5).abs() < 1e-6));
    }

    #[test]
    fn lanes_survive_the_project_and_undo() {
        let mut h = harness();
        let app = h.state_mut();
        let mut lane = Lane::new("fx:missing:gain");
        lane.put(100, 1.0);
        app.tracks[0].automation.push(lane);
        let ctx = h.ctx.clone();
        h.state_mut().sync_doc(&ctx, true);
        let p = h.state().proj.doc.get();
        assert_eq!(p.tracks[0].automation.len(), 1);
        assert_eq!(p.tracks[0].automation[0].points[0].at, 100);

        // A point moved is one undo step.
        h.state_mut().tracks[0].automation[0].put(100, 2.0);
        h.state_mut().sync_doc(&ctx, true);
        h.state_mut().undo(&ctx);
        assert_eq!(h.state().tracks[0].automation[0].points[0].value, 1.0);
        h.state_mut().redo(&ctx);
        assert_eq!(h.state().tracks[0].automation[0].points[0].value, 2.0);
    }

    #[test]
    fn thinning_keeps_the_corners_and_drops_the_straight_runs() {
        let pts: Vec<AutoPoint> = (0..=10)
            .map(|i| AutoPoint {
                at: i * 100,
                value: if i <= 5 { i as f64 } else { 5.0 },
            })
            .collect();
        let thin = thin(&pts, UNIT.clone_with(0.0, 10.0));
        let ats: Vec<i64> = thin.iter().map(|p| p.at).collect();
        assert_eq!(ats, vec![0, 500, 1000]);
    }

    #[test]
    fn ranges_map_both_ways_in_steps_and_in_ratio() {
        let hz = Range {
            min: 20.0,
            max: 20000.0,
            log: true,
        };
        let k = hz.fraction(632.4555);
        assert!((k - 0.5).abs() < 1e-3, "{k}");
        assert!((hz.value(0.5) - 632.4555).abs() < 0.01);
        assert_eq!(DB.fraction(-60.0), 0.0);
        assert_eq!(DB.value(1.0), 12.0);
    }

    impl Range {
        fn clone_with(self, min: f64, max: f64) -> Range {
            Range { min, max, ..self }
        }
    }
}
