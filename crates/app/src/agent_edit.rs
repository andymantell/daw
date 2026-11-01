// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for what the panes do by hand (roadmap 2.11):
//! tracks, routing and sends, and effects. Each is one undo step, as
//! every edit by an assistant is (see `agent.rs`).
//!
//! Most edit the project as the document has it, then put the app in
//! step with it, as undo does; adding a track or an effect makes it the
//! way the panes do.

use daw_project::{Project, TrackKind};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "list_effect_kinds",
        description: "The effects that can be added to a track: those that come with the \
                      app (by id, with what each does) and the plugins found on this \
                      computer (by name).",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "add_track",
        description: "Adds a track: audio, an instrument track (with one of the \
                      instruments that come with the app), a bus or a folder. Gives its id.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "kind": {"type": "string", "enum": ["audio", "instrument", "poly-synth",
                         "sampler", "drum-sampler", "drum-machine", "slicer", "bus", "folder"],
                         "description": "instrument has the simple synth; poly-synth the bigger one."},
                "name": {"type": "string"},
                "index": {"type": "integer", "description": "Where in the track list, from 0; the end if left out."}
            }, "required": ["kind"]})
        },
    },
    Tool {
        name: "edit_track",
        description: "Changes a track: its name, where its audio goes (a bus's id, or \
                      \"master\"), the folder it sits in (a folder's id, or \"none\"), or its \
                      place in the list. Only what is given changes.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "name": {"type": "string"},
                "output": {"type": "string"},
                "folder": {"type": "string"},
                "index": {"type": "integer"}
            }, "required": ["track"]})
        },
    },
    Tool {
        name: "delete_track",
        description: "Deletes a track with its clips and effects. Tracks that went to it, \
                      or sent to it, go to the master instead, or stop sending.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"}
            }, "required": ["track"]})
        },
    },
    Tool {
        name: "set_send",
        description: "Sends a copy of a track's audio to a bus, or changes or removes that \
                      send. Only what is given changes; a new send starts at 0 dB after the \
                      fader.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "to": {"type": "string", "description": "The bus's id."},
                "gain_db": {"type": "number"},
                "pre_fader": {"type": "boolean"},
                "mute": {"type": "boolean"},
                "remove": {"type": "boolean"}
            }, "required": ["track", "to"]})
        },
    },
    Tool {
        name: "add_effect",
        description: "Adds an effect to a track's chain: one that comes with the app, by \
                      id, or a plugin, by name (list_effect_kinds gives both). Gives its \
                      id; list_parameters then gives its settings.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "effect": {"type": "string"},
                "index": {"type": "integer", "description": "Where in the chain, from 0; the end if left out."}
            }, "required": ["track", "effect"]})
        },
    },
    Tool {
        name: "edit_effect",
        description: "Switches an effect off or on (bypass) or moves it in its track's \
                      chain. Its settings are set with set_parameter.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "effect": {"type": "string", "description": "The effect's id."},
                "bypass": {"type": "boolean"},
                "index": {"type": "integer"}
            }, "required": ["track", "effect"]})
        },
    },
    Tool {
        name: "remove_effect",
        description: "Takes an effect off a track, with what is inside it if it is a \
                      container.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "effect": {"type": "string"}
            }, "required": ["track", "effect"]})
        },
    },
];

fn text<'a>(args: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("Give {what}."))
}

fn track_at(p: &Project, key: &str) -> Result<usize, String> {
    (p.tracks.iter())
        .position(|t| t.id == key)
        .ok_or_else(|| format!("There is no track {key}."))
}

/// Moves item `from` to `to` (clamped), keeping the rest in order.
fn move_to<T>(v: &mut Vec<T>, from: usize, to: i64) {
    let t = v.remove(from);
    let to = (to.max(0) as usize).min(v.len());
    v.insert(to, t);
}

impl App {
    /// Runs one of the tools here, or `None` if `name` isn't one.
    pub(crate) fn pane_tool(
        &mut self,
        ctx: &eframe::egui::Context,
        name: &str,
        args: &Value,
        access: Access,
    ) -> Option<Done> {
        Some(match name {
            "add_track" => self.add_track_tool(ctx, args),
            "edit_track" => self.edit_track_tool(args),
            "delete_track" => self.delete_track_tool(args),
            "set_send" => self.set_send_tool(args),
            "add_effect" => self.add_effect_tool(args),
            "edit_effect" => self.edit_effect_tool(args),
            "remove_effect" => self.remove_effect_tool(args),
            _ => {
                return self
                    .arrange_tool(name, args)
                    .or_else(|| self.files_tool(ctx, name, args, access));
            }
        })
    }

    /// Edits the project as the document has it and puts the app in
    /// step; the change is written as the request's undo step.
    pub(crate) fn edit_model(
        &mut self,
        f: impl FnOnce(&mut Project) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let before = self.project();
        let mut p = before.clone();
        let v = f(&mut p)?;
        if p != before {
            self.reload_changed_fx(&before, &p);
            self.apply(p);
            // `apply` takes the project as written; it isn't yet.
            self.proj.committed = before;
        }
        Ok(v)
    }

    pub(crate) fn effect_kinds_value(&self) -> Value {
        let builtin: Vec<Value> = (daw_effects::Kind::ALL.iter())
            .map(|k| {
                json!({"id": k.id(), "name": k.name(), "about": k.about(),
                       "container": k.is_container()})
            })
            .collect();
        let plugins: Vec<Value> = (self.effects.list.plugins().into_iter())
            .filter(|p| p.enabled && !p.class.is_instrument())
            .map(|p| json!({"name": p.class.name}))
            .collect();
        json!({"builtin": builtin, "plugins": plugins})
    }

    fn add_track_tool(&mut self, ctx: &eframe::egui::Context, args: &Value) -> Done {
        use crate::commands::Cmd;
        let kind = text(args, "kind", "the kind of track")?;
        let cmd = match kind {
            "audio" => Cmd::AddAudioTrack,
            "instrument" => Cmd::AddInstrumentTrack,
            "bus" => Cmd::AddBus,
            "folder" => Cmd::AddFolder,
            other => {
                let k = (daw_instruments::Kind::ALL.iter())
                    .position(|k| k.id() == other)
                    .ok_or_else(|| format!("There is no kind of track {other}."))?;
                Cmd::AddInstrument(k as u8)
            }
        };
        let n = self.tracks.len();
        self.run(ctx, cmd);
        if self.tracks.len() != n + 1 {
            return Err(self.message.take().unwrap_or("No track was added.".into()));
        }
        let mut t = self.tracks.pop().expect("one was added");
        if let Some(name) = args["name"].as_str() {
            t.name = name.into();
        }
        let key = t.key.clone();
        let at = args["index"]
            .as_i64()
            .unwrap_or(n as i64)
            .clamp(0, n as i64) as usize;
        self.tracks.insert(at, t);
        self.rebuild();
        Ok((
            json!({"track": key, "index": at}),
            Some(format!("Add a {kind} track")),
        ))
    }

    fn edit_track_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        self.edit_model(|p| {
            let i = track_at(p, &key)?;
            if let Some(name) = args["name"].as_str() {
                p.tracks[i].name = name.into();
            }
            if let Some(out) = args["output"].as_str() {
                p.tracks[i].output = match out {
                    "master" => None,
                    bus => {
                        let b = track_at(p, bus)?;
                        if !p.tracks[b].kind.is_bus() || b == i {
                            return Err(format!("{bus} isn't a bus this track can go to."));
                        }
                        Some(bus.into())
                    }
                };
            }
            if let Some(folder) = args["folder"].as_str() {
                p.tracks[i].parent = match folder {
                    "none" => None,
                    f => {
                        let fi = track_at(p, f)?;
                        if p.tracks[fi].kind != TrackKind::Folder || fi == i {
                            return Err(format!("{f} isn't a folder this track can go in."));
                        }
                        Some(f.into())
                    }
                };
            }
            if let Some(to) = args["index"].as_i64() {
                move_to(&mut p.tracks, i, to);
            }
            Ok(json!({"track": key}))
        })
        .map(|v| (v, Some("Change a track".into())))
    }

    fn delete_track_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        self.edit_model(|p| {
            let i = track_at(p, &key)?;
            let gone = p.tracks.remove(i);
            for t in &mut p.tracks {
                if t.output.as_deref() == Some(&key) {
                    t.output = None;
                }
                if t.parent.as_deref() == Some(&key) {
                    t.parent = gone.parent.clone();
                }
                t.sends.retain(|s| s.to != key);
                for e in t.effects.iter_mut() {
                    if e.sidechain.as_deref() == Some(&key) {
                        e.sidechain = None;
                    }
                }
            }
            Ok(json!({"deleted": key, "name": gone.name}))
        })
        .map(|v| (v, Some("Delete a track".into())))
    }

    fn set_send_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let to = text(args, "to", "the bus's id")?.to_string();
        self.edit_model(|p| {
            let i = track_at(p, &key)?;
            let b = track_at(p, &to)?;
            if !p.tracks[b].kind.is_bus() || b == i {
                return Err(format!("{to} isn't a bus this track can send to."));
            }
            let sends = &mut p.tracks[i].sends;
            if args["remove"].as_bool() == Some(true) {
                sends.retain(|s| s.to != to);
                return Ok(json!({"removed": to}));
            }
            let s = match sends.iter().position(|s| s.to == to) {
                Some(j) => &mut sends[j],
                None => {
                    sends.push(daw_project::Send::to(to.clone()));
                    sends.last_mut().expect("just pushed")
                }
            };
            if let Some(g) = args["gain_db"].as_f64() {
                s.gain_db = g.clamp(-120.0, 12.0);
            }
            if let Some(b) = args["pre_fader"].as_bool() {
                s.pre_fader = b;
            }
            if let Some(m) = args["mute"].as_bool() {
                s.mute = m;
            }
            Ok(json!({"to": to, "gain_db": s.gain_db, "pre_fader": s.pre_fader, "mute": s.mute}))
        })
        .map(|v| (v, Some("Change a send".into())))
    }

    fn add_effect_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?;
        let which = text(args, "effect", "the effect: an id or a plugin's name")?;
        let ti = self
            .track_index(key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        let rate = self.rate;
        let slot = match daw_effects::Kind::from_id(which) {
            Some(k) => self.effects.create_builtin(k, rate),
            None => {
                let a = (self.effects.list.plugins().into_iter())
                    .filter(|p| p.enabled && !p.class.is_instrument())
                    .find(|p| p.class.name.eq_ignore_ascii_case(which))
                    .ok_or_else(|| {
                        format!("There is no effect {which}; list_effect_kinds gives them.")
                    })?;
                self.effects
                    .create(&a, rate)
                    .map_err(|e| format!("{}: {e}", a.class.name))?
            }
        };
        let id = slot.key.clone();
        let name = slot.name.clone();
        let fx = &mut self.tracks[ti].fx;
        let at = (args["index"].as_i64()).map_or(fx.len(), |i| (i.max(0) as usize).min(fx.len()));
        fx.insert(at, slot);
        self.rebuild();
        Ok((
            json!({"effect": id, "name": name, "index": at}),
            Some(format!("Add {name}")),
        ))
    }

    fn edit_effect_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let id = text(args, "effect", "the effect's id")?.to_string();
        self.edit_model(|p| {
            let i = track_at(p, &key)?;
            let fx = &mut p.tracks[i].effects;
            let j = (fx.iter().position(|e| e.id == id))
                .ok_or_else(|| format!("The track has no effect {id}."))?;
            if let Some(b) = args["bypass"].as_bool() {
                fx[j].bypass = b;
            }
            if let Some(to) = args["index"].as_i64() {
                move_to(fx, j, to);
            }
            Ok(json!({"effect": id}))
        })
        .map(|v| (v, Some("Change an effect".into())))
    }

    fn remove_effect_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let id = text(args, "effect", "the effect's id")?.to_string();
        self.edit_model(|p| {
            let i = track_at(p, &key)?;
            let fx = &mut p.tracks[i].effects;
            if !fx.iter().any(|e| e.id == id) {
                return Err(format!("The track has no effect {id}."));
            }
            // A container goes with what's inside it, and so on down.
            let mut gone = vec![id.clone()];
            let mut k = 0;
            while k < gone.len() {
                let inside: Vec<String> = (fx.iter())
                    .filter(|e| e.container.as_deref() == Some(&gone[k]))
                    .map(|e| e.id.clone())
                    .collect();
                gone.extend(inside);
                k += 1;
            }
            fx.retain(|e| !gone.contains(&e.id));
            Ok(json!({"removed": gone}))
        })
        .map(|v| (v, Some("Remove an effect".into())))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    fn track<'a>(p: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        (p["tracks"].as_array().unwrap().iter())
            .find(|t| t["id"] == id)
            .unwrap()
    }

    #[test]
    fn tracks_are_added_routed_and_deleted_each_in_one_step() {
        let mut d = Driver::new();
        let audio = d
            .call("add_track", json!({"kind": "audio", "name": "Guitar"}))
            .unwrap();
        let audio = audio["track"].as_str().unwrap().to_string();
        let bus = d
            .call(
                "add_track",
                json!({"kind": "bus", "name": "Verb", "index": 0}),
            )
            .unwrap();
        let bus = bus["track"].as_str().unwrap().to_string();
        let p = d.project();
        assert_eq!(p["tracks"][0]["name"], "Verb");
        assert_eq!(track(&p, &audio)["kind"], "audio");

        d.call(
            "edit_track",
            json!({"track": audio, "output": bus, "name": "Lead"}),
        )
        .unwrap();
        d.call(
            "set_send",
            json!({"track": audio, "to": bus, "gain_db": -12.0}),
        )
        .unwrap();
        let p = d.project();
        let t = track(&p, &audio);
        assert_eq!(
            (t["name"].clone(), t["output"].clone()),
            (json!("Lead"), json!(bus))
        );
        assert_eq!(t["sends"][0]["gain_db"], -12.0);
        assert!(
            d.call("edit_track", json!({"track": bus, "output": audio}))
                .is_err()
        );

        d.call("delete_track", json!({"track": bus})).unwrap();
        let p = d.project();
        let t = track(&p, &audio);
        assert_eq!(t["output"], json!(null));
        assert_eq!(t["sends"], json!([]));

        // Each was a step: undoing the delete brings the bus back, routed.
        d.command("undo");
        let p = d.project();
        assert_eq!(track(&p, &audio)["output"], json!(bus));
        assert_eq!(p["tracks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn effects_are_added_switched_off_moved_and_removed() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "audio"})).unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let kinds = d.call("list_effect_kinds", json!({})).unwrap();
        assert!(
            kinds["builtin"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k["id"] == "eq")
        );
        let eq = d
            .call("add_effect", json!({"track": t, "effect": "eq"}))
            .unwrap();
        let eq = eq["effect"].as_str().unwrap().to_string();
        let comp = d
            .call(
                "add_effect",
                json!({"track": t, "effect": "compressor", "index": 0}),
            )
            .unwrap();
        let comp = comp["effect"].as_str().unwrap().to_string();
        let ids = |d: &mut Driver| -> Vec<String> {
            let p = d.project();
            (track(&p, &t)["effects"].as_array().unwrap().iter())
                .map(|e| e["id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(ids(&mut d), [comp.clone(), eq.clone()]);
        // Its settings can then be set by value.
        let params = d.call("list_parameters", json!({"track": t})).unwrap();
        assert!(params.to_string().contains(&eq), "{params}");

        d.call(
            "edit_effect",
            json!({"track": t, "effect": eq, "index": 0, "bypass": true}),
        )
        .unwrap();
        assert_eq!(ids(&mut d), [eq.clone(), comp.clone()]);
        assert_eq!(track(&d.project(), &t)["effects"][0]["bypass"], true);
        d.call("remove_effect", json!({"track": t, "effect": comp}))
            .unwrap();
        assert_eq!(ids(&mut d), [eq]);
        assert!(
            d.call("add_effect", json!({"track": t, "effect": "no such"}))
                .is_err()
        );
        d.command("undo");
        assert_eq!(ids(&mut d).len(), 2);
    }
}
