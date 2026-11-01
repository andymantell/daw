// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for the monitor section (roadmap 2.3, 2.11): the
//! headphone mixes, which are part of the project, and the speakers'
//! controls and where things are plugged in, which belong to this
//! computer.

use daw_audio_io::Speakers;
use daw_engine::monitor::{MAX_CUES, MAX_SPEAKERS};
use daw_project::{CUE_OFF_DB, CueLevel, CueMix, Project};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_monitor",
        description: "The monitor section: each headphone mix (its level, mute, whether it \
                      is the same as the main mix, talkback, click, and each track's level, \
                      pan and mute in it), what the speakers play and their controls, and \
                      where speakers, headphones and the talkback microphone are plugged in.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "edit_headphone_mixes",
        description: "Adds, changes or removes headphone mixes (a mix of its own for each \
                      performer), sets tracks' levels in them, or copies the main mix's \
                      levels into one. One undo step.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "add": {"type": "array", "items": {"type": "string"},
                        "description": "New mixes' names."},
                "change": {"type": "array", "items": {"type": "object", "properties": {
                    "mix": {"type": "string", "description": "The mix's id."},
                    "name": {"type": "string"},
                    "gain_db": {"type": "number", "description": "-60 to 12."},
                    "mute": {"type": "boolean"},
                    "same_as_main": {"type": "boolean"},
                    "talkback": {"type": "boolean"},
                    "click_db": {"type": ["number", "null"],
                                 "description": "The click's level, -40 to 12; null for no click."}
                }, "required": ["mix"]}},
                "levels": {"type": "array", "items": {"type": "object", "properties": {
                    "mix": {"type": "string"},
                    "track": {"type": "string"},
                    "gain_db": {"type": ["number", "null"], "description": "-60 to 12; null for off."},
                    "pan": {"type": "number", "description": "-1 (left) to 1 (right)."},
                    "mute": {"type": "boolean"}
                }, "required": ["mix", "track"]}},
                "copy_main": {"type": "string", "description": "A mix's id: set every track as in the main mix."},
                "remove": {"type": "array", "items": {"type": "string"}}
            }})
        },
    },
    Tool {
        name: "set_monitor",
        description: "The speakers' controls (what they play, level, dim, mono, cut, which \
                      speakers, talkback), and on this computer where speakers and \
                      headphone mixes are plugged in and the talkback microphone's input. \
                      These belong to the computer, not the project, so there is no undo step.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "listen": {"type": "string", "description": "\"main\", or a headphone mix's id."},
                "gain_db": {"type": "number", "description": "Speaker level, -60 to 12."},
                "dim": {"type": "boolean"},
                "dim_db": {"type": "number", "description": "How far Dim turns down, 0 to 40."},
                "mono": {"type": "boolean"},
                "cut": {"type": "boolean"},
                "speakers": {"type": "integer", "description": "0 for the main outputs' speakers, then the extra sets in order."},
                "talk": {"type": "boolean", "description": "Talkback on or off."},
                "latch": {"type": "boolean", "description": "The Talk button stays on when let go."},
                "talk_db": {"type": "number", "description": "-40 to 12."},
                "talk_dim_db": {"type": "number", "description": "How far talkback dims the speakers, 0 to 40."},
                "main_trim_db": {"type": "number", "description": "-12 to 12."},
                "add_speakers": {"type": "object", "properties": {
                    "name": {"type": "string"},
                    "outputs": {"type": "array", "items": {"type": "integer"},
                                "description": "Left and right outputs, from 1."}
                }},
                "change_speakers": {"type": "object", "properties": {
                    "index": {"type": "integer", "description": "1 for the first extra set."},
                    "name": {"type": "string"},
                    "outputs": {"type": "array", "items": {"type": "integer"}},
                    "trim_db": {"type": "number"},
                    "remove": {"type": "boolean"}
                }, "required": ["index"]},
                "phones": {"type": "object", "properties": {
                    "mix": {"type": "string"},
                    "outputs": {"type": ["array", "null"], "items": {"type": "integer"},
                                "description": "Left and right outputs, from 1; null for nowhere."}
                }, "required": ["mix"]},
                "talkback_input": {"type": ["integer", "null"],
                                   "description": "The talkback microphone's input, from 1; null for none."}
            }})
        },
    },
];

/// A number from `v`, which must lie in `lo..=hi`.
fn within(v: &Value, what: &str, lo: f64, hi: f64) -> Result<Option<f64>, String> {
    match v.as_f64() {
        None => Ok(None),
        Some(x) if (lo..=hi).contains(&x) => Ok(Some(x)),
        Some(_) => Err(format!("{what} goes from {lo} to {hi}.")),
    }
}

fn mix_at(p: &Project, id: &str) -> Result<usize, String> {
    (p.cue_mixes.iter())
        .position(|m| m.id == id)
        .ok_or_else(|| format!("There is no headphone mix {id}."))
}

impl App {
    pub(crate) fn monitor_read(&mut self, name: &str) -> Option<Result<Value, String>> {
        (name == "get_monitor").then(|| Ok(self.monitor_value()))
    }

    pub(crate) fn monitor_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        Some(match name {
            "edit_headphone_mixes" => self.edit_headphone_mixes(args),
            "set_monitor" => self.set_monitor(args),
            _ => return None,
        })
    }

    fn monitor_value(&self) -> Value {
        let p = self.project();
        let mixes: Vec<Value> = (p.cue_mixes.iter())
            .map(|m| {
                let levels: Vec<Value> = (p.tracks.iter())
                    .map(|t| {
                        let l = m.level(t);
                        json!({"track": t.id, "name": t.name,
                               "gain_db": (l.gain_db > CUE_OFF_DB).then_some(l.gain_db),
                               "pan": l.pan, "mute": l.mute})
                    })
                    .collect();
                json!({"id": m.id, "name": m.name, "gain_db": m.gain_db, "mute": m.mute,
                       "same_as_main": m.follow_main, "talkback": m.talkback,
                       "click_db": m.click_db, "levels": levels})
            })
            .collect();
        let c = &self.cues;
        let pair = |(l, r): (usize, usize)| json!([l + 1, r + 1]);
        let device = self.profile.as_ref().map(|d| {
            let m = &d.monitor;
            json!({
                "main_outputs": pair(d.main),
                "main_trim_db": m.main_trim_db,
                "speakers": (m.speakers.iter()).map(|s| json!({
                    "name": s.name, "outputs": pair(s.out), "trim_db": s.trim_db
                })).collect::<Vec<_>>(),
                "phones": (p.cue_mixes.iter().enumerate()).map(|(k, x)| json!({
                    "mix": x.id, "outputs": m.phones.get(k).copied().flatten().map(pair)
                })).collect::<Vec<_>>(),
                "talkback_input": m.talkback.map(|ch| ch + 1),
            })
        });
        json!({
            "headphone_mixes": mixes,
            "speakers": {
                "listen": c.listen.clone().unwrap_or("main".into()),
                "gain_db": c.gain_db, "dim": c.dim, "dim_db": c.dim_db, "mono": c.mono,
                "cut": c.cut, "speakers": c.speakers, "talk": c.talk, "latch": c.latch,
                "talk_db": c.talk_db, "talk_dim_db": c.talk_dim_db,
            },
            "this_computer": device,
        })
    }

    fn edit_headphone_mixes(&mut self, args: &Value) -> Done {
        let v = self.edit_model(|p| {
            let mut added = Vec::new();
            for name in args["add"].as_array().into_iter().flatten() {
                if p.cue_mixes.len() >= MAX_CUES {
                    return Err(format!("{MAX_CUES} headphone mixes is the most."));
                }
                let name = (name.as_str())
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map_or_else(
                        || format!("Headphones {}", p.cue_mixes.len() + 1),
                        str::to_string,
                    );
                let m = CueMix::new(daw_project::new_id(), name);
                added.push(m.id.clone());
                p.cue_mixes.push(m);
            }
            for c in args["change"].as_array().into_iter().flatten() {
                let k = mix_at(p, c["mix"].as_str().unwrap_or(""))?;
                let m = &mut p.cue_mixes[k];
                if let Some(n) = c["name"].as_str() {
                    m.name = n.into();
                }
                if let Some(g) = within(&c["gain_db"], "A mix's level", -60.0, 12.0)? {
                    m.gain_db = g;
                }
                if let Some(b) = c["mute"].as_bool() {
                    m.mute = b;
                }
                if let Some(b) = c["same_as_main"].as_bool() {
                    m.follow_main = b;
                }
                if let Some(b) = c["talkback"].as_bool() {
                    m.talkback = b;
                }
                if c.get("click_db").is_some() {
                    m.click_db = within(&c["click_db"], "The click's level", -40.0, 12.0)?;
                }
            }
            for l in args["levels"].as_array().into_iter().flatten() {
                let k = mix_at(p, l["mix"].as_str().unwrap_or(""))?;
                let id = l["track"].as_str().unwrap_or("");
                let t = (p.tracks.iter())
                    .find(|t| t.id == id)
                    .ok_or_else(|| format!("There is no track {id}."))?;
                let mut level = p.cue_mixes[k].level(t);
                if l.get("gain_db").is_some() {
                    level.gain_db =
                        within(&l["gain_db"], "A level", -60.0, 12.0)?.unwrap_or(CUE_OFF_DB);
                }
                if let Some(x) = within(&l["pan"], "Pan", -1.0, 1.0)? {
                    level.pan = x;
                }
                if let Some(b) = l["mute"].as_bool() {
                    level.mute = b;
                }
                p.cue_mixes[k].set_level(level);
            }
            if let Some(id) = args["copy_main"].as_str() {
                let k = mix_at(p, id)?;
                // A bus that tracks go through is heard through them
                // already, as the pane's Copy main mix has it.
                let levels: Vec<CueLevel> = (p.tracks.iter())
                    .map(|t| {
                        let group = (p.tracks.iter()).any(|o| {
                            o.output.as_deref().or(o.parent.as_deref()) == Some(t.id.as_str())
                        });
                        CueLevel {
                            track: t.id.clone(),
                            gain_db: if t.mute || group {
                                CUE_OFF_DB
                            } else {
                                t.gain_db
                            },
                            pan: t.pan,
                            mute: false,
                        }
                    })
                    .collect();
                p.cue_mixes[k].levels = levels;
                p.cue_mixes[k].follow_main = false;
            }
            for id in args["remove"].as_array().into_iter().flatten() {
                let k = mix_at(p, id.as_str().unwrap_or(""))?;
                p.cue_mixes.remove(k);
            }
            Ok(json!({"added": added}))
        })?;
        // A removed mix can't be listened to, and its outputs pass to
        // the mix that takes its place, as in the pane.
        if let Some(l) = &self.cues.listen
            && !self.cues.mixes.iter().any(|m| &m.id == l)
        {
            self.cues.listen = None;
        }
        self.send_monitor();
        Ok((v, Some("Change headphone mixes".into())))
    }

    fn set_monitor(&mut self, args: &Value) -> Done {
        let outs = self.output_count();
        let ins = self.input_count();
        let pair = |v: &Value| -> Result<(usize, usize), String> {
            let a: Vec<i64> = (v.as_array().into_iter().flatten())
                .filter_map(Value::as_i64)
                .collect();
            match a[..] {
                [l, r] if l >= 1 && r >= 1 && (l.max(r) as usize) <= outs => {
                    Ok((l as usize - 1, r as usize - 1))
                }
                _ => Err(format!("Give two outputs from 1 to {outs}.")),
            }
        };
        let mut c = std::mem::take(&mut self.cues);
        let room = (|| -> Result<(), String> {
            if let Some(l) = args["listen"].as_str() {
                c.listen = match l {
                    "main" => None,
                    id if c.mixes.iter().any(|m| m.id == id) => Some(id.into()),
                    id => return Err(format!("There is no headphone mix {id}.")),
                };
            }
            if let Some(x) = within(&args["gain_db"], "The speaker level", -60.0, 12.0)? {
                c.gain_db = x as f32;
            }
            if let Some(x) = within(&args["dim_db"], "Dim", 0.0, 40.0)? {
                c.dim_db = x as f32;
            }
            if let Some(x) = within(&args["talk_db"], "The talkback level", -40.0, 12.0)? {
                c.talk_db = x as f32;
            }
            if let Some(x) = within(&args["talk_dim_db"], "Talkback's dim", 0.0, 40.0)? {
                c.talk_dim_db = x as f32;
            }
            for (key, flag) in [
                ("dim", &mut c.dim),
                ("mono", &mut c.mono),
                ("cut", &mut c.cut),
                ("latch", &mut c.latch),
                ("talk", &mut c.talk),
            ] {
                if let Some(b) = args[key].as_bool() {
                    *flag = b;
                }
            }
            Ok(())
        })();
        let sets = 1 + self
            .profile
            .as_ref()
            .map_or(0, |p| p.monitor.speakers.len());
        if let Some(s) = args["speakers"].as_u64() {
            if s as usize >= sets {
                self.cues = c;
                return Err(format!("There are {sets} sets of speakers, from 0."));
            }
            c.speakers = s as usize;
        }
        self.cues = c;
        room?;
        let device = [
            "main_trim_db",
            "add_speakers",
            "change_speakers",
            "phones",
            "talkback_input",
        ];
        if device.iter().any(|k| args.get(*k).is_some()) {
            let mixes: Vec<String> = self.cues.mixes.iter().map(|m| m.id.clone()).collect();
            let Some(p) = &mut self.profile else {
                return Err("Choose an audio device first (Settings, Audio device).".into());
            };
            let mut m = p.monitor.clone();
            if let Some(x) = within(&args["main_trim_db"], "Trim", -12.0, 12.0)? {
                m.main_trim_db = x as f32;
            }
            if let Some(a) = args.get("add_speakers") {
                if m.speakers.len() >= MAX_SPEAKERS - 1 {
                    return Err(format!(
                        "{} extra sets of speakers is the most.",
                        MAX_SPEAKERS - 1
                    ));
                }
                m.speakers.push(Speakers {
                    name: (a["name"].as_str()).map_or_else(
                        || format!("Speakers {}", m.speakers.len() + 2),
                        str::to_string,
                    ),
                    out: pair(&a["outputs"])?,
                    trim_db: 0.0,
                });
            }
            if let Some(a) = args.get("change_speakers") {
                let i = a["index"].as_u64().unwrap_or(0) as usize;
                if i == 0 || i > m.speakers.len() {
                    return Err(format!(
                        "There are {} extra sets of speakers, from 1.",
                        m.speakers.len()
                    ));
                }
                if a["remove"].as_bool() == Some(true) {
                    m.speakers.remove(i - 1);
                    self.cues.speakers = 0;
                } else {
                    let s = &mut m.speakers[i - 1];
                    if let Some(n) = a["name"].as_str() {
                        s.name = n.into();
                    }
                    if a.get("outputs").is_some() {
                        s.out = pair(&a["outputs"])?;
                    }
                    if let Some(x) = within(&a["trim_db"], "Trim", -12.0, 12.0)? {
                        s.trim_db = x as f32;
                    }
                }
            }
            if let Some(a) = args.get("phones") {
                let id = a["mix"].as_str().unwrap_or("");
                let k = (mixes.iter())
                    .position(|m| m == id)
                    .ok_or_else(|| format!("There is no headphone mix {id}."))?;
                let out = if a["outputs"].is_null() {
                    None
                } else {
                    Some(pair(&a["outputs"])?)
                };
                m.phones.resize(m.phones.len().max(k + 1), None);
                m.phones[k] = out;
                while m.phones.last() == Some(&None) {
                    m.phones.pop();
                }
            }
            if let Some(t) = args.get("talkback_input") {
                m.talkback = match t.as_u64() {
                    None if t.is_null() => None,
                    Some(ch) if ch >= 1 && ch as usize <= ins => Some(ch as usize - 1),
                    _ => return Err(format!("Give an input from 1 to {ins}, or null.")),
                };
            }
            p.monitor = m;
            self.save_monitor_device();
        }
        self.send_monitor();
        Ok((self.monitor_value()["speakers"].clone(), None))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    #[test]
    fn headphone_mixes_and_speakers() {
        let mut d = Driver::new();
        d.call("add_track", json!({"kind": "audio", "name": "Guitar"}))
            .unwrap();
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        let r = d
            .call("edit_headphone_mixes", json!({"add": ["Drummer"]}))
            .unwrap();
        let mix = r["added"][0].as_str().unwrap().to_string();
        d.call(
            "edit_headphone_mixes",
            json!({"change": [{"mix": mix, "gain_db": -6, "click_db": -3}],
                   "levels": [{"mix": mix, "track": t, "gain_db": -12, "pan": -0.5}]}),
        )
        .unwrap();
        let m = d.call("get_monitor", json!({})).unwrap();
        let h = &m["headphone_mixes"][0];
        assert_eq!(h["name"], "Drummer");
        assert_eq!(h["gain_db"], -6.0);
        assert_eq!(h["click_db"], -3.0);
        assert_eq!(h["levels"][0]["gain_db"], -12.0);
        assert_eq!(h["levels"][0]["pan"], -0.5);

        d.call(
            "set_monitor",
            json!({"listen": mix, "dim": true, "mono": true}),
        )
        .unwrap();
        let s = &d.call("get_monitor", json!({})).unwrap()["speakers"];
        assert_eq!(s["listen"], mix.as_str());
        assert_eq!(s["dim"], true);
        assert_eq!(s["mono"], true);
        assert!(d.call("set_monitor", json!({"listen": "nope"})).is_err());
        assert!(d.call("set_monitor", json!({"gain_db": 40})).is_err());

        d.call("edit_headphone_mixes", json!({"remove": [mix]}))
            .unwrap();
        let m = d.call("get_monitor", json!({})).unwrap();
        assert_eq!(m["headphone_mixes"].as_array().unwrap().len(), 0);
        assert_eq!(
            m["speakers"]["listen"], "main",
            "a removed mix isn't listened to"
        );
        d.command("undo");
        let m = d.call("get_monitor", json!({})).unwrap();
        assert_eq!(
            m["headphone_mixes"].as_array().unwrap().len(),
            1,
            "undo brings it back"
        );
    }
}
