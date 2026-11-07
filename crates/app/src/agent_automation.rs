// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for automation (roadmap 2.11, 2.6): the lanes a
//! track's settings follow as the song plays, their points, and each
//! track's automation mode, as the automation pane has them.
//!
//! A lane is named by the setting it is on, as `list_parameters` names
//! it (`gain`, `pan`, `send:<bus>`, `fx:<effect>:<key>`,
//! `plugin:<effect>:<id>`, `instrument.<key>`, `layer:<layer>:gain`,
//! `layer:<layer>:pan`, `layer:<layer>:instrument.<key>`). Its points
//! are joined by straight lines and held level before the first and
//! after the last; there are no curves.
//! A point is found by its place in its lane, from 0 in time order, as
//! `get_automation` lists it.

use daw_engine::time::{Frames, TempoMap, Ticks};
use daw_project::{AutoPoint, Lane};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::automation::Mode;

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

/// Where a point goes, in a point given to `edit_automation`.
const AT: &str = "A bar (from 1, fractions for within the bar) or seconds from the start.";

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_automation",
        description: "Automation: how a track's settings change over time as the song \
                      plays. Gives a track's automation (or every track's): its mode (Off, Read, Touch, \
                      Latch or Write) and its lanes, each with the setting it is on and its \
                      points in time order (place in the lane, bar, seconds, value in the \
                      setting's own unit, and that value as text). Points are joined by \
                      straight lines and held level beyond the ends. The track \"master\" is \
                      the master channel, listed when it has lanes.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string", "description": "Every track's if left out."}
            }})
        },
    },
    Tool {
        name: "edit_automation",
        description: "Automates a track setting so it changes over time as the song plays: \
                      a volume ride or fade, a filter sweep (a synth's instrument.cutoff, or \
                      a Filter effect's cutoff), a pan move, a send, effect or \
                      instrument setting rising or falling (a plugin instrument's \
                      parameters too, and each layer of a layered instrument: its level, \
                      pan and instrument's settings). Draws points on the setting's automation \
                      lane, which the setting follows from point to point. It changes one \
                      lane: adds it (for any setting \
                      list_parameters names that can be automated; it starts level at the \
                      setting's value now unless points are given), removes it, clears its \
                      points, and adds, moves and deletes points. Points are given by place \
                      in the lane as get_automation lists them, before this call changes \
                      anything; values are in the setting's own unit (dB for volume and \
                      sends, -1 to 1 for pan, 0 to 1 for plugins) and are kept in its \
                      range. Adding points to a lane that isn't there adds it. Done in the \
                      order: remove lane, clear, delete, move, add. Example, a fade in over \
                      two bars: param gain, add [{bar: 1, value: -60}, {bar: 3, value: 0}]. \
                      The track \"master\" automates the master channel's volume and \
                      effect settings (a fade out of the whole song).",
        access: Access::Edit,
        schema: || {
            let point = json!({"type": "object", "properties": {
                "bar": {"type": "number", "description": AT},
                "seconds": {"type": "number", "description": AT},
                "value": {"type": "number"}
            }, "required": ["value"]});
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "param": {"type": "string", "description": "The setting, as list_parameters names it."},
                "add_lane": {"type": "boolean"},
                "remove_lane": {"type": "boolean"},
                "clear": {"type": "boolean", "description": "Takes every point off the lane, which then leaves the setting as it is."},
                "delete": {"type": "array", "items": {"type": "integer"}, "description": "Points' places in the lane."},
                "move": {"type": "array", "items": {"type": "object", "properties": {
                    "point": {"type": "integer", "description": "The point's place in the lane."},
                    "bar": {"type": "number", "description": AT},
                    "seconds": {"type": "number", "description": AT},
                    "value": {"type": "number"}
                }, "required": ["point"]}, "description": "Only what is given changes."},
                "add": {"type": "array", "items": point, "description": "A point at a time that has one replaces it."}
            }, "required": ["track", "param"]})
        },
    },
    Tool {
        name: "set_automation_mode",
        description: "Sets a track's automation mode. Off: the lanes are ignored. Read: \
                      the settings follow their lanes. Touch: a control moved while \
                      playing is written while held. Latch: written from the first touch \
                      until play stops. Write: every lane on the track is written from \
                      play to stop. The mode belongs to this session of the app, not the \
                      project, so it is no undo step.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "mode": {"type": "string", "enum": ["off", "read", "touch", "latch", "write"]}
            }, "required": ["track", "mode"]})
        },
    },
];

/// The bar a frame is in, with how far through it as a fraction: the
/// reverse of the `bar` argument the tools take.
fn bar_at(map: &TempoMap, at: i64) -> f64 {
    let t = map.ticks_at(Frames(at.max(0)));
    let bar = map.bar_beat(t).bar;
    let from = map.bar_start(bar).0;
    let len = (map.bar_start(bar + 1).0 - from).max(1);
    bar as f64 + (t.0 - from) as f64 / len as f64
}

/// The frame a bar (from 1, with a fraction) starts at.
fn bar_frames(map: &TempoMap, bar: f64) -> i64 {
    let bar = bar.max(1.0);
    let whole = bar.floor() as i64;
    let from = map.bar_start(whole);
    let len = map.bar_start(whole + 1).0 - from.0;
    map.frames_at(Ticks(from.0 + ((bar - bar.floor()) * len as f64) as i64))
        .0
}

fn mode_named(s: &str) -> Option<Mode> {
    Mode::ALL
        .into_iter()
        .find(|m| m.name().eq_ignore_ascii_case(s))
}

impl App {
    /// Runs one of the tools here that may change things, or `None` if
    /// `name` isn't one.
    pub(crate) fn automation_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        Some(match name {
            "edit_automation" => self.edit_automation_tool(args),
            "set_automation_mode" => self.set_automation_mode_tool(args),
            _ => return None,
        })
    }

    /// The frame a point's `bar` or `seconds` gives, if it gives one.
    fn point_time(&self, map: &TempoMap, v: &Value) -> Option<i64> {
        match (v["bar"].as_f64(), v["seconds"].as_f64()) {
            (Some(bar), _) => Some(bar_frames(map, bar)),
            (None, Some(s)) => Some((s.max(0.0) * self.rate as f64).round() as i64),
            (None, None) => None,
        }
    }

    /// What `get_automation` answers.
    pub(crate) fn automation_value(&self, only: Option<&str>) -> Result<Value, String> {
        if let Some(k) = only
            && k != daw_project::Master::KEY
            && self.track_index(k).is_none()
        {
            return Err(format!("There is no track {k}."));
        }
        let map = self.tempo_map();
        let rate = self.rate.max(1) as f64;
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        // The master is listed when asked for, or when it has lanes.
        let master = (only == Some(daw_project::Master::KEY)
            || (only.is_none() && !self.master.automation.is_empty()))
        .then_some(&self.master);
        let tracks: Vec<Value> = (self.tracks.iter())
            .filter(|t| only.is_none_or(|k| t.key == k))
            .chain(master)
            .map(|t| {
                let lanes: Vec<Value> = (t.automation.iter())
                    .map(|l| {
                        let points: Vec<Value> = (l.points.iter().enumerate())
                            .map(|(i, p)| {
                                json!({
                                    "point": i,
                                    "bar": bar_at(&map, p.at),
                                    "seconds": p.at as f64 / rate,
                                    "value": p.value,
                                    "text": crate::automation::text_of(t, &l.param, p.value),
                                })
                            })
                            .collect();
                        json!({
                            "param": l.param,
                            "name": crate::automation::name_of(t, &buses, &l.param),
                            // A lane whose effect has gone does nothing.
                            "missing": crate::automation::control(t, &l.param).is_none(),
                            "points": points,
                        })
                    })
                    .collect();
                json!({
                    "track": t.key,
                    "name": t.name,
                    "mode": self.automation.mode(&t.key).name(),
                    "lanes": lanes,
                })
            })
            .collect();
        Ok(json!({ "tracks": tracks }))
    }

    fn edit_automation_tool(&mut self, args: &Value) -> Done {
        let key = args["track"].as_str().ok_or("Give the track's id.")?;
        let param = args["param"].as_str().ok_or("Give the setting's name.")?;
        let master = key == daw_project::Master::KEY;
        let t = if master {
            &self.master
        } else {
            let ti = self
                .track_index(key)
                .ok_or_else(|| format!("There is no track {key}."))?;
            &self.tracks[ti]
        };
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        if master && param == Lane::PAN {
            return Err(
                "The master has no pan; its volume and effect settings can be automated.".into(),
            );
        }
        let name = crate::automation::name_of(t, &buses, param);
        // The setting is there to automate: its value now, and its range.
        let here = crate::automation::control(t, param).zip(crate::automation::range(t, param));
        let had = t.automation.iter().any(|l| l.param == param);

        let flag = |k: &str| args[k].as_bool().unwrap_or(false);
        let list = |k: &str| -> Result<Vec<Value>, String> {
            match &args[k] {
                Value::Null => Ok(Vec::new()),
                Value::Array(a) => Ok(a.clone()),
                _ => Err(format!("Give {k} as a list.")),
            }
        };
        let delete: Vec<usize> = (list("delete")?.iter())
            .map(|v| v.as_u64().map(|i| i as usize))
            .collect::<Option<_>>()
            .ok_or("Give the points to delete by their places in the lane, from 0.")?;
        let moves = list("move")?;
        let adds = list("add")?;
        let (add_lane, remove_lane, clear) = (flag("add_lane"), flag("remove_lane"), flag("clear"));
        if !(add_lane || remove_lane || clear)
            && delete.is_empty()
            && moves.is_empty()
            && adds.is_empty()
        {
            return Err(
                "Say what to change: add_lane, remove_lane, clear, delete, move or add.".into(),
            );
        }

        let map = self.tempo_map();
        let clamp = |v: f64| match here {
            Some((_, r)) => v.clamp(r.min, r.max),
            None => v,
        };
        let value = |v: &Value, what: &str| -> Result<Option<f64>, String> {
            match &v["value"] {
                Value::Null => Ok(None),
                n => match n.as_f64() {
                    Some(x) if x.is_finite() => Ok(Some(clamp(x))),
                    _ => Err(format!("The value of {what} has to be a number.")),
                },
            }
        };
        // Points to add and moves, worked out before the project is
        // touched, so a bad one changes nothing.
        let mut added: Vec<AutoPoint> = Vec::with_capacity(adds.len());
        for (i, a) in adds.iter().enumerate() {
            let what = format!("point {i} to add");
            let at = self
                .point_time(&map, a)
                .ok_or_else(|| format!("Give a bar or seconds for {what}."))?;
            let v = value(a, &what)?.ok_or_else(|| format!("Give a value for {what}."))?;
            added.push(AutoPoint { at, value: v });
        }
        let mut moved: Vec<(usize, Option<i64>, Option<f64>)> = Vec::with_capacity(moves.len());
        for m in &moves {
            let j = m["point"]
                .as_u64()
                .ok_or("Give each point to move by its place in the lane, from 0.")?
                as usize;
            moved.push((
                j,
                self.point_time(&map, m),
                value(m, &format!("point {j}"))?,
            ));
        }
        if !remove_lane
            && here.is_none()
            && (!had || add_lane || !added.is_empty() || !moved.is_empty())
        {
            return Err(if had {
                format!(
                    "{name} isn't on {} any more; its lane can only be cleared or removed.",
                    t.name
                )
            } else {
                format!(
                    "{} has no setting {param} that can be automated; list_parameters names them.",
                    t.name
                )
            });
        }
        let track_name = t.name.clone();
        let now = here.map(|(v, _)| v);

        let v = self.edit_model(|p| {
            let lanes = if master {
                &mut p.master.automation
            } else {
                let t = (p.tracks.iter_mut())
                    .find(|t| t.id == key)
                    .ok_or_else(|| format!("There is no track {key}."))?;
                &mut t.automation
            };
            if remove_lane {
                let before = lanes.len();
                lanes.retain(|l| l.param != param);
                if lanes.len() == before {
                    return Err(format!("{track_name} has no {name} lane."));
                }
                return Ok(json!({"removed": param}));
            }
            let li = match lanes.iter().position(|l| l.param == param) {
                Some(i) => i,
                None if add_lane || !added.is_empty() => {
                    let mut lane = Lane::new(param);
                    // Level at the setting's value, as the pane starts
                    // one, unless points are given to draw it.
                    if added.is_empty()
                        && let Some(v) = now
                    {
                        lane.put(0, v);
                    }
                    lanes.push(lane);
                    lanes.len() - 1
                }
                None => return Err(format!("{track_name} has no {name} lane; add it first.")),
            };
            let lane = &mut lanes[li];
            if clear {
                lane.points.clear();
            }
            let n = lane.points.len();
            if let Some(&j) = delete
                .iter()
                .chain(moved.iter().map(|(j, ..)| j))
                .find(|&&j| j >= n)
            {
                return Err(format!("The {name} lane has no point {j}; it has {n}."));
            }
            let mut kept: Vec<Option<AutoPoint>> = lane.points.iter().copied().map(Some).collect();
            for &j in &delete {
                kept[j] = None;
            }
            for &(j, at, v) in &moved {
                let Some(q) = kept[j].as_mut() else {
                    return Err(format!("Point {j} is both deleted and moved."));
                };
                if let Some(at) = at {
                    q.at = at;
                }
                if let Some(v) = v {
                    q.value = v;
                }
            }
            lane.points.clear();
            for q in kept.into_iter().flatten().chain(added.iter().copied()) {
                lane.put(q.at, q.value);
            }
            Ok(json!({"param": param, "points": lane.points.len()}))
        })?;
        let label = if remove_lane {
            format!("Remove the {name} lane on {track_name}")
        } else if !had {
            format!("Add a {name} lane on {track_name}")
        } else {
            format!("Change the {name} lane on {track_name}")
        };
        Ok((v, Some(label)))
    }

    fn set_automation_mode_tool(&mut self, args: &Value) -> Done {
        let key = args["track"].as_str().ok_or("Give the track's id.")?;
        let mode = args["mode"].as_str().ok_or("Give the mode.")?;
        let mode = mode_named(mode).ok_or_else(|| {
            format!("There is no mode {mode}; it is off, read, touch, latch or write.")
        })?;
        if key != daw_project::Master::KEY {
            self.track_index(key)
                .ok_or_else(|| format!("There is no track {key}."))?;
        }
        if self.automation.mode(key) != mode {
            self.automation.set_mode(key, mode);
            self.rebuild();
        }
        Ok((json!({"track": key, "mode": mode.name()}), None))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;

    fn id(v: Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    /// A driver with one audio track, and that track's id.
    fn one_track() -> (Driver, String) {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        (d, t)
    }

    fn lanes(d: &mut Driver, t: &str) -> Value {
        d.call("get_automation", json!({"track": t})).unwrap()["tracks"][0]["lanes"].clone()
    }

    #[test]
    fn a_lane_is_added_drawn_on_and_shown() {
        let (mut d, t) = one_track();
        d.call(
            "set_parameter",
            json!({"track": t, "param": "gain", "value": -3.0}),
        )
        .unwrap();
        // Added alone, it starts level at the setting's value.
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "add_lane": true}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        assert_eq!(l[0]["param"], "gain");
        assert_eq!(l[0]["name"], "Volume");
        assert_eq!(l[0]["points"][0]["value"], -3.0);
        assert_eq!(l[0]["points"][0]["bar"], 1.0);

        // Points by bar and by seconds, kept in the range and in order.
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "add": [
                {"bar": 3, "value": -12.0},
                {"seconds": 1.0, "value": 100.0}
            ]}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        let p = l[0]["points"].as_array().unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[1]["seconds"], 1.0);
        assert_eq!(p[1]["value"], 12.0, "kept to the top of the range");
        assert_eq!(p[2]["bar"], 3.0);
        assert_eq!(p[2]["value"], -12.0);
        assert_eq!(p[2]["text"], "-12.0 dB");
        let proj = d.project();
        assert_eq!(proj["tracks"][0]["automated"][0], "gain");

        // A new lane given points has only those.
        d.call(
            "edit_automation",
            json!({"track": t, "param": "pan", "add": [{"bar": 2.5, "value": -0.5}]}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        assert_eq!(l[1]["param"], "pan");
        assert_eq!(l[1]["points"].as_array().unwrap().len(), 1);
        assert_eq!(l[1]["points"][0]["bar"], 2.5);
    }

    #[test]
    fn points_are_moved_and_deleted_and_lanes_cleared_and_removed() {
        let (mut d, t) = one_track();
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "add": [
                {"bar": 1, "value": 0.0}, {"bar": 2, "value": -6.0}, {"bar": 3, "value": -12.0}
            ]}),
        )
        .unwrap();
        // Places are as listed before the call: move the first past the
        // last, and delete the middle one.
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "delete": [1],
                   "move": [{"point": 0, "bar": 4}, {"point": 2, "value": -20.0}]}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        let p = l[0]["points"].as_array().unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0]["bar"].clone(), p[0]["value"].clone()),
            (json!(3.0), json!(-20.0))
        );
        assert_eq!(
            (p[1]["bar"].clone(), p[1]["value"].clone()),
            (json!(4.0), json!(0.0))
        );

        let err = d
            .call(
                "edit_automation",
                json!({"track": t, "param": "gain", "delete": [5]}),
            )
            .unwrap_err();
        assert!(err.contains("no point 5"), "{err}");

        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "clear": true}),
        )
        .unwrap();
        assert_eq!(lanes(&mut d, &t)[0]["points"], json!([]));
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "remove_lane": true}),
        )
        .unwrap();
        assert_eq!(lanes(&mut d, &t), json!([]));
    }

    #[test]
    fn each_edit_is_one_undo_step() {
        let (mut d, t) = one_track();
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "add": [{"bar": 1, "value": -6.0}]}),
        )
        .unwrap();
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "add": [{"bar": 2, "value": -9.0}, {"bar": 3, "value": -1.0}]}),
        )
        .unwrap();
        assert_eq!(lanes(&mut d, &t)[0]["points"].as_array().unwrap().len(), 3);
        d.command("undo");
        assert_eq!(lanes(&mut d, &t)[0]["points"].as_array().unwrap().len(), 1);
        d.command("undo");
        assert_eq!(lanes(&mut d, &t), json!([]));
        d.command("redo");
        assert_eq!(lanes(&mut d, &t)[0]["points"].as_array().unwrap().len(), 1);
        // Playing the lane into the volume control is no step of its own.
        let undo = d.call("get_history", json!({})).unwrap()["undo"].clone();
        assert!(
            (undo.as_array().unwrap().iter())
                .all(|s| s["label"].as_str().unwrap().starts_with("Assistant")),
            "{undo}"
        );
        assert_eq!(d.project()["tracks"][0]["gain_db"], 0.0);
    }

    #[test]
    fn effect_settings_and_sends_can_be_automated() {
        let (mut d, t) = one_track();
        let bus = id(
            d.call("add_track", json!({"kind": "bus"})).unwrap(),
            "track",
        );
        d.call("set_send", json!({"track": t, "to": bus})).unwrap();
        let fx = id(
            d.call("add_effect", json!({"track": t, "effect": "eq"}))
                .unwrap(),
            "effect",
        );
        let params = d.call("list_parameters", json!({"track": t})).unwrap();
        let fx_param = params[0]["params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["param"].as_str().unwrap().to_string())
            .find(|p| p.starts_with(&format!("fx:{fx}:")))
            .expect("the effect's settings are listed");
        let send = format!("send:{bus}");
        for param in [&fx_param, &send] {
            d.call(
                "edit_automation",
                json!({"track": t, "param": param, "add_lane": true}),
            )
            .unwrap();
        }
        let l = lanes(&mut d, &t);
        assert_eq!(l[0]["param"], fx_param.as_str());
        assert_eq!(l[0]["missing"], false);
        assert!(l[1]["name"].as_str().unwrap().starts_with("Send to"));
        let listed = d.call("list_parameters", json!({"track": t})).unwrap();
        let automated = listed[0]["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["param"] == send.as_str())
            .unwrap()["automated"]
            .clone();
        assert_eq!(automated, true);

        // Settings that can't be automated are refused.
        for param in ["mute", "nothing", "fx:none:gain"] {
            let err = d
                .call(
                    "edit_automation",
                    json!({"track": t, "param": param, "add_lane": true}),
                )
                .unwrap_err();
            assert!(err.contains("list_parameters"), "{param}: {err}");
        }
    }

    #[test]
    fn the_mode_decides_whether_the_engine_plays_the_lanes() {
        let (mut d, t) = one_track();
        d.call(
            "edit_automation",
            json!({"track": t, "param": "gain", "add": [{"bar": 1, "value": -6.0}]}),
        )
        .unwrap();
        let specs = |d: &mut Driver| {
            let app = d.app();
            let track = app.tracks.iter().find(|x| x.key == t).unwrap();
            app.lane_specs(track).len()
        };
        assert_eq!(specs(&mut d), 1);
        let a = d.call("get_automation", json!({})).unwrap();
        assert_eq!(a["tracks"][0]["mode"], "Read");

        d.call("set_automation_mode", json!({"track": t, "mode": "off"}))
            .unwrap();
        assert_eq!(specs(&mut d), 0);
        let a = d.call("get_automation", json!({"track": t})).unwrap();
        assert_eq!(a["tracks"][0]["mode"], "Off");
        d.call("set_automation_mode", json!({"track": t, "mode": "Latch"}))
            .unwrap();
        assert_eq!(specs(&mut d), 1);
        assert!(
            d.call("set_automation_mode", json!({"track": t, "mode": "loud"}))
                .is_err()
        );
    }

    #[test]
    fn looking_only_may_read_but_not_change() {
        let (mut d, t) = one_track();
        d.access = crate::agent::Access::Read;
        assert!(d.call("get_automation", json!({})).is_ok());
        assert!(
            d.call(
                "edit_automation",
                json!({"track": t, "param": "gain", "add_lane": true})
            )
            .is_err()
        );
        assert!(
            d.call("set_automation_mode", json!({"track": t, "mode": "off"}))
                .is_err()
        );
    }

    /// A synth track playing a bright note in each of two bars.
    fn synth_note() -> (Driver, String) {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "poly-synth"})).unwrap(),
            "track",
        );
        let c = id(
            d.call("add_note_clip", json!({"track": t, "bar": 1, "bars": 2}))
                .unwrap(),
            "clip",
        );
        d.call(
            "edit_notes",
            // One a bar, as a render from bar 2 doesn't hear a note
            // started before it.
            json!({"clip": c, "add": [
                {"beat": 0, "beats": 4, "pitch": 48},
                {"beat": 4, "beats": 4, "pitch": 48}
            ]}),
        )
        .unwrap();
        for (param, value) in [
            ("instrument.cutoff", 18_000.0),
            ("instrument.filter_env", 0.0),
        ] {
            d.call(
                "set_parameter",
                json!({"track": t, "param": param, "value": value}),
            )
            .unwrap();
        }
        (d, t)
    }

    /// The level above 2 kHz of the track over its second half (the
    /// second bar), in dB.
    fn highs(d: &mut Driver, t: &str) -> f64 {
        let r = d
            .call("analyse", json!({"track": t, "from_bar": 2, "to_bar": 3}))
            .unwrap();
        let power: f64 = (r["third_octave_bands"].as_array().unwrap().iter())
            .filter(|b| b["hz"].as_f64().unwrap() >= 2000.0)
            .map(|b| 10f64.powf(b["db"].as_f64().unwrap() / 10.0))
            .sum();
        10.0 * power.max(1e-12).log10()
    }

    /// The wish from the third song test: a synth's own filter swept
    /// down by a lane, which the export (here, analyse) follows.
    #[test]
    fn a_synths_cutoff_can_be_swept_by_a_lane() {
        let (mut d, t) = synth_note();
        let open = highs(&mut d, &t);
        let listed = d
            .call(
                "list_parameters",
                json!({"track": t, "prefix": "instrument.cutoff"}),
            )
            .unwrap();
        assert_eq!(listed[0]["params"][0]["automated"], false);

        d.call(
            "edit_automation",
            json!({"track": t, "param": "instrument.cutoff", "add": [
                {"bar": 1, "value": 18_000.0},
                {"bar": 2, "value": 200.0}
            ]}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        assert_eq!(l[0]["param"], "instrument.cutoff");
        assert_eq!(l[0]["missing"], false);
        assert!(l[0]["name"].as_str().unwrap().ends_with(": Cutoff"), "{l}");
        assert_eq!(l[0]["points"][1]["text"], "200 Hz");
        let listed = d
            .call(
                "list_parameters",
                json!({"track": t, "prefix": "instrument.cutoff"}),
            )
            .unwrap();
        assert_eq!(listed[0]["params"][0]["automated"], true);

        // The second bar plays at 200 Hz: far less above 2 kHz, though
        // the control itself still says 18 kHz.
        let swept = highs(&mut d, &t);
        assert!(swept < open - 30.0, "{open} {swept}");
        let s = d
            .call(
                "list_parameters",
                json!({"track": t, "prefix": "instrument.cutoff"}),
            )
            .unwrap();
        assert_eq!(s[0]["params"][0]["value"], 18_000.0);

        // With the mode off, the lane is ignored again; and an undo
        // takes the lane away.
        d.call("set_automation_mode", json!({"track": t, "mode": "off"}))
            .unwrap();
        assert!((highs(&mut d, &t) - open).abs() < 1.0);
        d.call("set_automation_mode", json!({"track": t, "mode": "read"}))
            .unwrap();
        d.call("undo_steps", json!({"undo": 1})).unwrap();
        assert!(lanes(&mut d, &t).as_array().unwrap().is_empty());
        assert!((highs(&mut d, &t) - open).abs() < 1.0);

        // A setting the instrument hasn't is refused.
        let err = d
            .call(
                "edit_automation",
                json!({"track": t, "param": "instrument.nothing", "add_lane": true}),
            )
            .unwrap_err();
        assert!(err.contains("list_parameters"), "{err}");
    }

    /// The simple synth's settings can be automated too.
    #[test]
    fn the_simple_synths_cutoff_can_be_automated() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        d.call(
            "edit_automation",
            json!({"track": t, "param": "instrument.cutoff", "add": [{"bar": 1, "value": 300.0}]}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        assert_eq!(l[0]["points"][0]["text"], "300 Hz");
        let app = d.app();
        let track = app.tracks.iter().find(|x| x.key == t).unwrap();
        assert_eq!(
            app.lane_specs(track)[0].target,
            daw_engine::automation::Target::Instrument(5)
        );
    }

    /// The filter effect sweeps from 20 kHz down to 20 Hz, the whole
    /// range the third song test missed on the EQ's cuts.
    #[test]
    fn the_filter_effect_reaches_the_whole_range_and_can_be_swept() {
        let (mut d, t) = synth_note();
        let kinds = d.call("list_effect_kinds", json!({})).unwrap();
        assert!(
            (kinds["builtin"].as_array().unwrap().iter()).any(|k| k["id"] == "filter"),
            "{kinds}"
        );
        let open = highs(&mut d, &t);
        let fx = id(
            d.call("add_effect", json!({"track": t, "effect": "filter"}))
                .unwrap(),
            "effect",
        );
        let cutoff = format!("fx:{fx}:cutoff");
        let p = d
            .call("list_parameters", json!({"track": t, "prefix": cutoff}))
            .unwrap();
        let p = &p[0]["params"][0];
        assert_eq!(
            (p["min"].as_f64(), p["max"].as_f64()),
            (Some(20.0), Some(20_000.0))
        );
        let mode = d
            .call(
                "list_parameters",
                json!({"track": t, "prefix": format!("fx:{fx}:mode")}),
            )
            .unwrap();
        assert_eq!(
            mode[0]["params"][0]["choices"],
            json!(["Low-pass", "High-pass", "Band-pass", "Notch"])
        );
        d.call(
            "edit_automation",
            json!({"track": t, "param": cutoff, "add": [
                {"bar": 1, "value": 20_000.0},
                {"bar": 2, "value": 300.0}
            ]}),
        )
        .unwrap();
        let swept = highs(&mut d, &t);
        assert!(swept < open - 30.0, "{open} {swept}");
    }

    #[test]
    fn the_master_has_lanes_too() {
        let mut d = Driver::new();
        // A fade out of the whole song.
        d.call(
            "edit_automation",
            json!({"track": "master", "param": "gain", "add": [
                {"bar": 1, "value": 0.0},
                {"bar": 3, "value": -60.0}
            ]}),
        )
        .unwrap();
        let l = lanes(&mut d, "master");
        assert_eq!(l[0]["name"], "Volume");
        assert_eq!(l[0]["points"].as_array().unwrap().len(), 2);
        let all = d.call("get_automation", json!({})).unwrap();
        assert_eq!(
            all["tracks"].as_array().unwrap().last().unwrap()["track"],
            "master"
        );
        assert_eq!(d.project()["master"]["automated"][0], "gain");
        let params = d
            .call("list_parameters", json!({"track": "master"}))
            .unwrap();
        let gain = (params[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == "gain")
            .unwrap();
        assert_eq!(gain["automated"], true);
        assert!(
            d.call(
                "edit_automation",
                json!({"track": "master", "param": "pan", "add_lane": true})
            )
            .is_err()
        );

        // The engine plays it, unless the master's mode is off.
        assert_eq!(d.app().session().master_automation.len(), 1);
        d.call(
            "set_automation_mode",
            json!({"track": "master", "mode": "off"}),
        )
        .unwrap();
        assert!(d.app().session().master_automation.is_empty());
        d.call(
            "set_automation_mode",
            json!({"track": "master", "mode": "read"}),
        )
        .unwrap();

        // It is in the document, and one undo step.
        assert_eq!(d.app().proj.doc.get().master.automation.len(), 1);
        d.command("undo");
        assert!(d.app().master.automation.is_empty());
        assert!(d.app().session().master_automation.is_empty());
    }

    /// The level of a track over the whole song, by its sample peak.
    fn peak(d: &mut Driver, t: &str) -> f64 {
        let r = d.call("analyse", json!({"track": t})).unwrap();
        r["sample_peak_dbfs"].as_f64().unwrap()
    }

    /// The wish from the third song test: a plugin instrument's
    /// parameter takes a lane, which the export (here, analyse) follows.
    #[test]
    fn a_plugin_instruments_parameter_follows_its_lane() {
        use crate::freeze::test_plugin;
        let mut d = Driver::new();
        let app = d.app();
        app.effects.helper = Some(test_plugin::helper());
        let bundle = test_plugin::bundle();
        app.effects.list.bundles = vec![daw_plugins::scan::BundleEntry {
            result: Ok(daw_plugins::scan::scan_here(&bundle).unwrap()),
            path: bundle,
            modified: 0,
        }];
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        d.call(
            "set_instrument",
            json!({"track": t, "instrument": daw_test_plugin::SYNTH_NAME}),
        )
        .unwrap();
        let c = id(
            d.call("add_note_clip", json!({"track": t, "bar": 1, "bars": 2}))
                .unwrap(),
            "clip",
        );
        d.call(
            "edit_notes",
            json!({"clip": c, "add": [{"beat": 4, "beats": 1, "pitch": 60}]}),
        )
        .unwrap();

        // The plugin's gain is listed as the instrument's, by its id.
        let listed = d
            .call(
                "list_parameters",
                json!({"track": t, "prefix": "instrument."}),
            )
            .unwrap();
        let gain = (listed[0]["params"].as_array().unwrap().iter())
            .find(|p| p["name"].as_str().unwrap().ends_with("Gain"))
            .unwrap_or_else(|| panic!("{listed}"))
            .clone();
        let param = gain["param"].as_str().unwrap().to_string();
        assert_eq!(param, "instrument.0");
        assert_eq!(gain["automated"], false);
        assert_eq!(
            (gain["min"].as_f64(), gain["max"].as_f64()),
            (Some(0.0), Some(1.0))
        );
        let own = gain["value"].as_f64().unwrap();
        let loud = peak(&mut d, &t);

        // A lane at a quarter of the gain plays a quarter as loud.
        d.call(
            "edit_automation",
            json!({"track": t, "param": param, "add": [{"bar": 1, "value": own / 4.0}]}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        assert_eq!(l[0]["param"], "instrument.0");
        assert_eq!(l[0]["missing"], false);
        assert!(l[0]["name"].as_str().unwrap().ends_with(": Gain"), "{l}");
        let quiet = peak(&mut d, &t);
        assert!((loud - quiet - 12.04).abs() < 0.5, "{loud} {quiet}");
        let listed = d
            .call(
                "list_parameters",
                json!({"track": t, "prefix": "instrument.0"}),
            )
            .unwrap();
        assert_eq!(listed[0]["params"][0]["automated"], true);

        // With the lane off, set by value, the plugin takes it. (As with
        // a plugin effect, the plugin keeps the lane's last value until
        // it is set.)
        d.call("set_automation_mode", json!({"track": t, "mode": "off"}))
            .unwrap();
        d.call(
            "set_parameter",
            json!({"track": t, "param": param, "value": own / 2.0}),
        )
        .unwrap();
        assert!((loud - peak(&mut d, &t) - 6.02).abs() < 0.5);
    }

    /// The same wish for a layered instrument: each layer's level, pan
    /// and instrument settings take lanes, and the track's instrument's
    /// lanes go with it when it becomes the first layer.
    #[test]
    fn a_layers_level_and_instrument_settings_follow_their_lanes() {
        let (mut d, t) = synth_note();
        let open = highs(&mut d, &t);
        let loud = peak(&mut d, &t);
        // A sweep on the synth's own filter, before it is layered.
        d.call(
            "edit_automation",
            json!({"track": t, "param": "instrument.cutoff", "add": [
                {"bar": 1, "value": 18_000.0},
                {"bar": 2, "value": 200.0}
            ]}),
        )
        .unwrap();
        let swept = highs(&mut d, &t);
        assert!(swept < open - 30.0, "{open} {swept}");

        // Layered: the synth is layer 1, and its lane moves with it.
        let second = id(
            d.call("edit_layer", json!({"track": t, "add": true}))
                .unwrap(),
            "layer",
        );
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        let first = i["layers"][0]["id"].as_str().unwrap().to_string();
        // Layer 2 off, so only the synth is heard.
        d.call(
            "edit_layer",
            json!({"track": t, "layer": second, "on": false}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        let cutoff = format!("layer:{first}:instrument.cutoff");
        assert_eq!(l[0]["param"], cutoff.as_str(), "{l}");
        assert_eq!(l[0]["missing"], false);
        let name = l[0]["name"].as_str().unwrap();
        assert!(
            name.starts_with("Layer 1 ") && name.ends_with(": Cutoff"),
            "{name}"
        );
        assert!((highs(&mut d, &t) - swept).abs() < 3.0);

        // Every layer's settings are listed for lanes.
        let listed = d
            .call("list_parameters", json!({"track": t, "prefix": "layer:"}))
            .unwrap();
        let params = listed[0]["params"].as_array().unwrap();
        let level = format!("layer:{first}:gain");
        let pan = format!("layer:{second}:pan");
        for want in [&level, &pan, &cutoff] {
            assert!(
                params.iter().any(|p| p["param"] == want.as_str()),
                "{want}: {listed}"
            );
        }
        let lp = params
            .iter()
            .find(|p| p["param"] == level.as_str())
            .unwrap();
        assert_eq!(lp["name"], "Layer 1: Level");
        assert_eq!(
            (lp["min"].as_f64(), lp["max"].as_f64()),
            (Some(-60.0), Some(12.0))
        );

        // A lane takes layer 1 down 20 dB; its control stays at 0 dB.
        let before = peak(&mut d, &t);
        d.call(
            "edit_automation",
            json!({"track": t, "param": level, "add": [{"bar": 1, "value": -20.0}]}),
        )
        .unwrap();
        let l = lanes(&mut d, &t);
        assert_eq!(l[1]["name"], "Layer 1: Level", "{l}");
        assert_eq!(l[1]["points"][0]["text"], "-20.0 dB");
        let quiet = peak(&mut d, &t);
        assert!((before - quiet - 20.0).abs() < 1.0, "{before} {quiet}");
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["layers"][0]["gain_db"].as_f64(), Some(0.0), "{i}");

        // Off, the lanes are ignored; back to one instrument, the cutoff
        // lane goes back onto the synth.
        d.call("set_automation_mode", json!({"track": t, "mode": "off"}))
            .unwrap();
        assert!((peak(&mut d, &t) - loud).abs() < 1.0);
        assert!((highs(&mut d, &t) - open).abs() < 1.0);
        d.call("set_automation_mode", json!({"track": t, "mode": "read"}))
            .unwrap();
        d.command("single-instrument");
        let l = lanes(&mut d, &t);
        assert_eq!(l[0]["param"], "instrument.cutoff", "{l}");
        assert!((highs(&mut d, &t) - swept).abs() < 3.0);
    }
}
