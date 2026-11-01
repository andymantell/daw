// SPDX-License-Identifier: GPL-3.0-or-later
//! Values said and heard (Phase 4 D3).
//!
//! A slider tells a screen reader only its number, so a pan of 0.3 or a
//! volume of -6 is read without saying which way or in what. Each pane
//! says its values in words here instead: "30% left", "-6.0 dB", "12 ms".
//!
//! A value can be heard as well as read. Hear the value (Ctrl+Shift+H)
//! plays a short sound for the control with the keyboard, or the one
//! last moved: a tone panned where a pan control points, a tone as loud
//! as a fader is set, or a rising and falling tone that follows an
//! automation lane from its first point to its last. Its words show for
//! a few seconds at the foot of the window, where a screen reader reads
//! them out as they change.

use std::time::{Duration, Instant};

use eframe::egui::{self, Id};

use daw_engine::engine::Command;
use daw_engine::preview::PreviewSound;
use daw_project::{Lane, LaneTarget};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::automation;

/// How long a value's words stay at the foot of the window.
const SHOWN_FOR: Duration = Duration::from_secs(4);

/// A pan of -1 (left) to 1 (right) in words.
pub(crate) fn pan_text(p: f64) -> String {
    let pct = (p.abs() * 100.0).round();
    if pct < 1.0 {
        "centre".into()
    } else if p < 0.0 {
        format!("{pct:.0}% left")
    } else {
        format!("{pct:.0}% right")
    }
}

/// A level in decibels in words; the bottom of a fader is off.
pub(crate) fn db_text(db: f64) -> String {
    if db <= -60.0 {
        "off".into()
    } else if db.abs() < 0.05 {
        "0 dB".into()
    } else {
        format!("{db:+.1} dB")
    }
}

/// What a value sounds like.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Hear {
    /// A pan, -1 (left) to 1 (right).
    Pan(f64),
    /// A level in dB.
    Level(f64),
    /// A line through time, each value 0 (lowest) to 1 (highest).
    Line(Vec<f64>),
}

/// A value to hear, with its words.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Heard {
    pub(crate) text: String,
    pub(crate) hear: Hear,
}

fn key(id: Id) -> Id {
    id.with("spoken value")
}

fn last_moved() -> Id {
    Id::unique("spoken value last moved")
}

/// Where a control starts among the widgets drawn on `ui`'s layer: give
/// it to [`value`], [`pan`] or [`level`] once the control is drawn.
pub(crate) fn start(ui: &egui::Ui) -> usize {
    let layer = ui.layer_id();
    ui.ctx()
        .viewport(|v| v.this_pass.widgets.get_layer(layer).count())
}

/// The controls drawn on `ui`'s layer since `first`: a slider and the
/// number box beside it, say. (Read from the widgets, not the screen
/// reader's tree, which is only made while a screen reader is on.)
fn inputs_since(ui: &egui::Ui, first: usize) -> Vec<Id> {
    let layer = ui.layer_id();
    ui.ctx().viewport(|v| {
        (v.this_pass.widgets.get_layer(layer).skip(first))
            .filter(|r| r.sense.is_focusable())
            .map(|r| r.id)
            .collect()
    })
}

/// Has a screen reader say `text` for the value of the controls drawn
/// since `first` (see [`start`]), in place of the bare number a slider
/// gives.
pub(crate) fn value(ui: &egui::Ui, first: usize, text: impl Into<String>) {
    let text = text.into();
    for id in inputs_since(ui, first) {
        ui.ctx()
            .accesskit_node_builder(id, |node| node.set_value(text.clone()));
    }
}

/// Says the value of the controls drawn since `first` (as [`value`]) and
/// keeps it to be heard. One whose words change was moved.
pub(crate) fn hearable(ui: &egui::Ui, first: usize, text: impl Into<String>, hear: Hear) {
    let heard = Heard {
        text: text.into(),
        hear,
    };
    for id in inputs_since(ui, first) {
        ui.ctx()
            .accesskit_node_builder(id, |node| node.set_value(heard.text.clone()));
        keep(ui.ctx(), id, heard.clone());
    }
}

/// Keeps `heard` as the value of the widget `id`, to be heard; one whose
/// words change while it is dragged was moved.
pub(crate) fn keep(ctx: &egui::Context, id: Id, heard: Heard) {
    let dragged = ctx.is_being_dragged(id);
    ctx.data_mut(|d| {
        let before = d.get_temp::<Heard>(key(id));
        // Moved by the person, not by a lane playing or an assistant.
        if before.is_some_and(|b| b != heard) && dragged {
            d.insert_temp(last_moved(), id);
        }
        d.insert_temp(key(id), heard);
    });
}

/// A pan control: says "30% left" and is heard panned there.
pub(crate) fn pan(ui: &egui::Ui, first: usize, p: f64) {
    hearable(ui, first, pan_text(p), Hear::Pan(p));
}

/// A level control: says "-6.0 dB" and is heard that loud.
pub(crate) fn level(ui: &egui::Ui, first: usize, db: f64) {
    hearable(ui, first, db_text(db), Hear::Level(db));
}

/// The value of the control with the keyboard, or of the one last moved.
pub(crate) fn current(ctx: &egui::Context) -> Option<Heard> {
    let focused = ctx.memory(|m| m.focused());
    ctx.data(|d| {
        focused
            .and_then(|id| d.get_temp::<Heard>(key(id)))
            .or_else(|| {
                let id = d.get_temp::<Id>(last_moved())?;
                d.get_temp::<Heard>(key(id))
            })
    })
}

/// A short sound for `hear`, at `rate`, to play through the preview
/// voice.
pub(crate) fn sound(hear: &Hear, rate: u32) -> PreviewSound {
    let rate = rate.max(1) as f64;
    // A gentle start and end, so no click.
    let edge = |i: usize, n: usize| {
        let ramp = (0.01 * rate) as usize;
        let k = i.min(n - 1 - i.min(n - 1));
        (k as f64 / ramp.max(1) as f64).min(1.0)
    };
    let tone = |secs: f64, freq: &dyn Fn(f64) -> f64, gain: (f64, f64)| {
        let n = ((secs * rate) as usize).max(2);
        let mut phase = 0.0f64;
        let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..n {
            phase += freq(i as f64 / n as f64) / rate;
            let x = (phase * std::f64::consts::TAU).sin() * edge(i, n);
            l.push((x * gain.0) as f32);
            r.push((x * gain.1) as f32);
        }
        (l, r)
    };
    let (left, right) = match hear {
        Hear::Pan(p) => {
            // Equal power, as the mixer pans.
            let a = (p.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
            tone(0.4, &|_| 660.0, (0.3 * a.cos(), 0.3 * a.sin()))
        }
        Hear::Level(db) => {
            // The top of a fader (+12 dB) is a quarter of full scale.
            let g = 0.25 * 10f64.powf((db.clamp(-60.0, 12.0) - 12.0) / 20.0);
            let g = if *db <= -60.0 { 0.0 } else { g };
            tone(0.4, &|_| 440.0, (g, g))
        }
        Hear::Line(points) => {
            let at = |k: f64| {
                if points.is_empty() {
                    return 0.5;
                }
                let x = k * (points.len() - 1) as f64;
                let i = (x.floor() as usize).min(points.len() - 1);
                let j = (i + 1).min(points.len() - 1);
                let f = x - i as f64;
                points[i] * (1.0 - f) + points[j] * f
            };
            // Two octaves, from 220 Hz at the bottom to 880 at the top.
            tone(
                1.5,
                &|k| 220.0 * 4f64.powf(at(k).clamp(0.0, 1.0)),
                (0.2, 0.2),
            )
        }
    };
    PreviewSound {
        left,
        right,
        gain: 1.0,
        looped: false,
    }
}

/// The words of the value last heard, at the foot of the window.
#[derive(Default)]
pub(crate) struct Said {
    shown: Option<(String, Instant)>,
}

impl Said {
    pub(crate) fn say(&mut self, text: String) {
        self.shown = Some((text, Instant::now()));
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> Option<&str> {
        self.shown.as_ref().map(|(t, _)| t.as_str())
    }

    /// Draws the words while they last, as a polite live region so a
    /// screen reader reads them out when they change.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.shown else {
            return;
        };
        let left = SHOWN_FOR.saturating_sub(at.elapsed());
        if left.is_zero() {
            self.shown = None;
            return;
        }
        egui::Area::new(Id::unique("spoken value shown"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let r = ui.label(text);
                    ctx.accesskit_node_builder(r.id, |node| {
                        node.set_live(egui::accesskit::Live::Polite);
                    });
                });
            });
        ctx.request_repaint_after(left);
    }
}

/// Where `v` sits between `min` and `max`, 0 to 1, heard as a steady
/// tone that high.
pub(crate) fn steady(v: f64, min: f64, max: f64) -> Hear {
    let k = if max > min {
        ((v - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.5
    };
    Hear::Line(vec![k, k])
}

pub(crate) const TOOLS: &[Tool] = &[Tool {
    name: "hear_value",
    description: "Plays a value to the person at the computer as a short sound, and says it \
                  in words, as Hear the value (Ctrl+Shift+H) does for the control with the \
                  keyboard: a pan is heard from its side, a level as loud as it is set, any \
                  other setting as a tone as high as it sits in its range, and an automation \
                  lane (lane: true) as a tone that rises and falls with it from its first \
                  point to its last. The words show at the foot of the window, where a \
                  screen reader reads them. param is as in edit_automation (gain, pan, \
                  send:<bus>, an effect's setting); the default is gain.",
    access: Access::Read,
    schema: || {
        json!({"type": "object", "properties": {
            "track": {"type": "string", "description": "A track's id, or master."},
            "param": {"type": "string", "description": "The setting; gain if not given."},
            "lane": {"type": "boolean", "description": "Hear the setting's automation lane."}
        }, "required": ["track"]})
    },
}];

impl App {
    /// Hear the value: the control with the keyboard, or the one last
    /// moved.
    pub(crate) fn hear_value(&mut self, ctx: &egui::Context) {
        match current(ctx) {
            Some(h) => self.hear(h),
            None => self.said.say(
                "Nothing to hear yet: put the keyboard on a pan, a level, a setting or an \
                 automation lane, then Hear the value."
                    .into(),
            ),
        }
    }

    fn hear(&mut self, h: Heard) {
        let sound = sound(&h.hear, self.rate);
        self.send(Command::Preview(Some(Box::new(sound))));
        self.said.say(h.text);
    }

    pub(crate) fn spoken_read(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        (name == "hear_value").then(|| self.hear_value_tool(args))
    }

    fn hear_value_tool(&mut self, args: &Value) -> Result<Value, String> {
        let key = args["track"]
            .as_str()
            .ok_or("Give the track's id, or master.")?;
        let param = args["param"].as_str().unwrap_or(Lane::GAIN);
        let lane = args["lane"].as_bool().unwrap_or(false);
        let heard = if key == daw_project::Master::KEY {
            if param != Lane::GAIN || lane {
                return Err("The master is heard by its volume: param gain, no lane.".into());
            }
            let db = self.master.mix.gain_db as f64;
            Heard {
                text: format!("Master volume, {}", db_text(db)),
                hear: Hear::Level(db),
            }
        } else {
            let t = (self.tracks.iter())
                .find(|t| t.key == key)
                .ok_or_else(|| format!("There is no track {key}."))?;
            let buses: Vec<(String, String)> = (self.tracks.iter())
                .filter(|t| t.kind.is_bus())
                .map(|t| (t.key.clone(), t.name.clone()))
                .collect();
            if lane {
                let l = (t.automation.iter())
                    .find(|l| l.param == param)
                    .ok_or_else(|| {
                        format!(
                            "{} has no {} lane; get_automation lists them.",
                            t.name,
                            automation::name_of(t, &buses, param)
                        )
                    })?;
                if l.points.is_empty() {
                    return Err("That lane has no points to hear.".into());
                }
                let map = self.tempo_map();
                Heard {
                    text: format!("{}, {}", t.name, automation::lane_words(t, &buses, l, &map)),
                    hear: Hear::Line(automation::lane_line(t, l)),
                }
            } else {
                let v = automation::control(t, param).ok_or_else(|| {
                    format!(
                        "{} has no setting {param}; list_parameters gives them.",
                        t.name
                    )
                })?;
                let name = automation::name_of(t, &buses, param);
                let (text, hear) = match Lane::new(param).target() {
                    LaneTarget::Pan => (pan_text(v), Hear::Pan(v)),
                    LaneTarget::Gain | LaneTarget::Send(_) => (db_text(v), Hear::Level(v)),
                    _ => {
                        let g = automation::range(t, param);
                        let h = g.map_or(Hear::Line(vec![0.5, 0.5]), |g| {
                            let k = g.fraction(v);
                            Hear::Line(vec![k, k])
                        });
                        (automation::text_of(t, param, v), h)
                    }
                };
                Heard {
                    text: format!("{} {}, {text}", t.name, name.to_lowercase()),
                    hear,
                }
            }
        };
        self.hear(heard.clone());
        Ok(json!({"text": heard.text, "playing": true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_said_in_words() {
        assert_eq!(pan_text(0.0), "centre");
        assert_eq!(pan_text(-0.3), "30% left");
        assert_eq!(pan_text(1.0), "100% right");
        assert_eq!(db_text(-6.0), "-6.0 dB");
        assert_eq!(db_text(0.0), "0 dB");
        assert_eq!(db_text(3.5), "+3.5 dB");
        assert_eq!(db_text(-60.0), "off");
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn a_pan_is_heard_on_its_side_and_a_level_as_loud() {
        let s = sound(&Hear::Pan(-1.0), 48_000);
        assert!(peak(&s.left) > 0.25 && peak(&s.right) < 1e-6, "hard left");
        let s = sound(&Hear::Pan(0.0), 48_000);
        assert!((peak(&s.left) - peak(&s.right)).abs() < 1e-3, "centre");

        let loud = peak(&sound(&Hear::Level(0.0), 48_000).left);
        let quiet = peak(&sound(&Hear::Level(-20.0), 48_000).left);
        assert!((loud / quiet - 10.0).abs() < 0.1, "20 dB is ten times");
        assert_eq!(peak(&sound(&Hear::Level(-60.0), 48_000).left), 0.0);
    }

    /// Zero crossings in each half: more where the line is higher.
    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn a_line_is_heard_rising_as_it_rises() {
        let s = sound(&Hear::Line(vec![0.0, 1.0]), 48_000);
        let half = s.left.len() / 2;
        let (a, b) = (crossings(&s.left[..half]), crossings(&s.left[half..]));
        assert!(b * 2 > a * 3, "rises: {a} then {b}");
        let s = sound(&Hear::Line(vec![1.0, 0.0]), 48_000);
        let (a, b) = (crossings(&s.left[..half]), crossings(&s.left[half..]));
        assert!(a * 2 > b * 3, "falls: {a} then {b}");
    }
}
