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
                      \"master\"), the folder it sits in (a folder's id, or \"none\"), the \
                      control group whose fader and mute act on it (a group's id from \
                      get_project, or \"none\"; add one with the add-control-group \
                      command), or its place in the list. Only what is given changes.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "name": {"type": "string"},
                "output": {"type": "string"},
                "folder": {"type": "string"},
                "group": {"type": "string"},
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
        description: "Adds an effect to a track's chain, or to a layer's or a container's \
                      chain on the track: one that comes with the app, by id, or a plugin, \
                      by name (list_effect_kinds gives both). Gives its id; list_parameters \
                      then gives its settings.",
        access: Access::Edit,
        schema: || {
            let mut props = json!({
                "track": {"type": "string"},
                "effect": {"type": "string"},
                "index": {"type": "integer", "description": "Where in the track's list of effects, from 0; the end if left out. Effects in one chain play in the list's order."}
            });
            extend(&mut props, crate::agent_routing::place_schema());
            json!({"type": "object", "properties": props, "required": ["track", "effect"]})
        },
    },
    Tool {
        name: "edit_effect",
        description: "Switches an effect off or on (bypass), moves it in its track's list \
                      or into another chain (a layer's, a container's, or the track's own), \
                      and wires it: the track it listens to, a plugin's extra outputs heard, \
                      and the file a reverb, cabinet or amp capture player uses. get_effects \
                      shows all of these. Only what is given changes; settings are set with \
                      set_parameter.",
        access: Access::Edit,
        schema: || {
            let mut props = json!({
                "track": {"type": "string"},
                "effect": {"type": "string", "description": "The effect's id."},
                "bypass": {"type": "boolean"},
                "index": {"type": "integer"}
            });
            extend(&mut props, crate::agent_routing::place_schema());
            extend(&mut props, crate::agent_routing::wiring_schema());
            json!({"type": "object", "properties": props, "required": ["track", "effect"]})
        },
    },
    Tool {
        name: "edit_control_group",
        description: "Changes a control group (one fader and mute over several tracks, \
                      without summing their audio): its name, its fader, its mute; or \
                      deletes it, letting its tracks go. Add one with the \
                      add-control-group command, and put tracks in it with edit_track.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "group": {"type": "string", "description": "Its id, from get_project."},
                "name": {"type": "string"},
                "gain_db": {"type": "number"},
                "mute": {"type": "boolean"},
                "delete": {"type": "boolean"}
            }, "required": ["group"]})
        },
    },
    Tool {
        name: "set_sound",
        description: "Puts a sound into one of a built-in instrument's sound slots, as \
                      its window's Sound menus do. Slots: the drum sampler's \"sample\"; \
                      a drum machine pad's layer, \"pad0.layer0\" (pads 0 to 15 from note \
                      36, layers 0 to 3; the layer is set to play the sound, all of \
                      it: shorten it with its decay); the \
                      sampler's zones, \"zone0\" on (a new zone after the last; its root \
                      note and key range are then set with set_parameter, as \
                      instrument.zone0.root, .lo and .hi). The sound \
                      is a full path to a sound file (wav, flac, mp3, ogg, opus, aiff, \
                      m4a), or one the app makes: made:keys, made:pluck, made:kick, \
                      made:snare, made:hat, made:loop; empty to clear the slot.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "layer": {"type": "string", "description": "A layer's id, for a layered \
                          instrument; the track's own instrument if left out."},
                "slot": {"type": "string"},
                "sound": {"type": "string"}
            }, "required": ["track", "slot", "sound"]})
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

/// A drum machine's pad and layer, from its slot's name ("pad3.layer0").
fn pad_layer(slot: &str) -> Option<(usize, usize)> {
    use daw_instruments::drum_machine::{LAYERS, PADS};
    let (pad, layer) = slot.strip_prefix("pad")?.split_once(".layer")?;
    let (pad, layer) = (pad.parse().ok()?, layer.parse().ok()?);
    (pad < PADS && layer < LAYERS).then_some((pad, layer))
}

/// Adds `more`'s properties to `props`.
fn extend(props: &mut Value, more: Value) {
    if let (Some(a), Value::Object(b)) = (props.as_object_mut(), more) {
        a.extend(b);
    }
}

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
            "set_sound" => self.set_sound_tool(args),
            "edit_control_group" => self.edit_group_tool(args),
            _ => {
                return self
                    .arrange_tool(name, args)
                    .or_else(|| self.files_tool(ctx, name, args, access))
                    .or_else(|| self.takes_tool(ctx, name, args))
                    .or_else(|| self.notes_tool(name, args))
                    .or_else(|| self.layers_tool(name, args))
                    .or_else(|| self.routing_tool(name, args));
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

    fn edit_group_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "group", "the control group's id")?.to_string();
        self.edit_model(|p| {
            let i = (p.control_groups.iter())
                .position(|g| g.id == key)
                .ok_or_else(|| format!("There is no control group {key}."))?;
            if args["delete"].as_bool() == Some(true) {
                p.control_groups.remove(i);
                for t in &mut p.tracks {
                    if t.group.as_deref() == Some(key.as_str()) {
                        t.group = None;
                    }
                }
                return Ok(json!({"group": key, "deleted": true}));
            }
            let g = &mut p.control_groups[i];
            if let Some(n) = args["name"].as_str() {
                g.name = n.into();
            }
            if let Some(db) = args["gain_db"].as_f64() {
                g.gain_db = db.clamp(-60.0, 12.0);
            }
            if let Some(m) = args["mute"].as_bool() {
                g.mute = m;
            }
            Ok(json!({"group": key}))
        })
        .map(|v| (v, Some("Change a control group".into())))
    }

    fn set_sound_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let slot = text(args, "slot", "the sound slot")?.to_string();
        let sound = text(args, "sound", "the sound")?.to_string();
        if !sound.is_empty()
            && daw_instruments::made::name(&sound).is_none()
            && !std::path::Path::new(&sound).is_file()
        {
            return Err(format!("There is no sound file {sound}."));
        }
        let layer = args["layer"].as_str().map(str::to_string);
        self.edit_model(|p| {
            let i = track_at(p, &key)?;
            let t = &mut p.tracks[i];
            let e = match &layer {
                Some(l) => {
                    &mut (t.layers.iter_mut())
                        .find(|x| &x.id == l)
                        .ok_or_else(|| format!("Track {key} has no layer {l}."))?
                        .instrument
                }
                None => t
                    .instrument
                    .as_mut()
                    .ok_or_else(|| format!("Track {key} has no instrument."))?,
            };
            let kind = daw_instruments::Kind::from_id(&e.builtin).ok_or(
                "That instrument has no sound slots: it isn't one that comes with the app.",
            )?;
            let fits = match kind {
                daw_instruments::Kind::DrumSampler => slot == "sample",
                daw_instruments::Kind::Slicer => {
                    return Err("The loop slicer's loop is chosen in its window for now.".into());
                }
                daw_instruments::Kind::Sampler => slot
                    .strip_prefix("zone")
                    .and_then(|n| n.parse::<usize>().ok())
                    .is_some(),
                daw_instruments::Kind::DrumMachine => pad_layer(&slot).is_some(),
                daw_instruments::Kind::Synth => false,
            };
            if !fits {
                return Err(format!("A {} has no sound slot {slot}.", kind.name()));
            }
            e.samples.retain(|(k, _)| *k != slot);
            if !sound.is_empty() {
                e.samples.push((slot.clone(), sound.clone()));
            }
            if let Some((pad, l)) = pad_layer(&slot)
                && !sound.is_empty()
            {
                // It plays the sound, all of it: a pad's own drum fades
                // early, which would cut a cymbal's tail.
                use daw_instruments::drum_machine::{NO_FADE, SAMPLE};
                for (k, v) in [("source", SAMPLE), ("decay", NO_FADE)] {
                    let k = format!("pad{pad}.layer{l}.{k}");
                    e.params.retain(|(x, _)| *x != k);
                    e.params.push((k, v));
                }
            }
            Ok(json!({"track": key, "slot": slot}))
        })
        .map(|v| (v, Some("Choose a sound".into())))
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
            if let Some(group) = args["group"].as_str() {
                p.tracks[i].group = match group {
                    "none" => None,
                    g if p.control_groups.iter().any(|x| x.id == g) => Some(g.into()),
                    g => return Err(format!("There is no control group {g}.")),
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
        let place = self.place_arg(ti, args, None)?.flatten();
        let rate = self.rate;
        let mut slot = match daw_effects::Kind::from_id(which) {
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
        slot.place = place.clone();
        let id = slot.key.clone();
        let name = slot.name.clone();
        let fx = &mut self.tracks[ti].fx;
        let at = (args["index"].as_i64()).map_or(fx.len(), |i| (i.max(0) as usize).min(fx.len()));
        fx.insert(at, slot);
        self.rebuild();
        Ok((
            json!({"effect": id, "name": name, "index": at,
                   "container": place.as_ref().map(|p| &p.0),
                   "branch": place.as_ref().map(|p| p.1)}),
            Some(format!("Add {name}")),
        ))
    }

    fn edit_effect_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let id = text(args, "effect", "the effect's id")?.to_string();
        let ti = self
            .track_index(&key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        let place = self.place_arg(ti, args, Some(&id))?;
        if let Some(s) = self.tracks[ti].fx.iter().find(|s| s.key == id) {
            self.check_wiring(ti, s, args)?;
        }
        self.edit_model(|p| {
            let i = track_at(p, &key)?;
            let fx = &mut p.tracks[i].effects;
            let j = (fx.iter().position(|e| e.id == id))
                .ok_or_else(|| format!("The track has no effect {id}."))?;
            if let Some(b) = args["bypass"].as_bool() {
                fx[j].bypass = b;
            }
            if let Some(place) = place {
                fx[j].branch = place.as_ref().map_or(0, |p| p.1);
                fx[j].container = place.map(|p| p.0);
            }
            if let Some(sc) = args["sidechain"].as_str() {
                fx[j].sidechain = (sc != "none").then(|| sc.to_string());
            }
            if let Some(outs) = args["outputs"].as_array() {
                let mut o: Vec<u32> = outs
                    .iter()
                    .filter_map(|v| v.as_u64())
                    .map(|v| v as u32)
                    .collect();
                o.sort_unstable();
                o.dedup();
                fx[j].outputs = o;
            }
            if let Some(f) = args["file"].as_str() {
                fx[j].response = f.to_string();
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
    fn a_command_acts_on_the_track_or_clip_given() {
        let mut d = Driver::new();
        let add = |d: &mut Driver| {
            let t = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
            t["track"].as_str().unwrap().to_string()
        };
        let (a, b) = (add(&mut d), add(&mut d));
        d.call("run_command", json!({"id": "add-arpeggiator", "track": b}))
            .unwrap();
        let p = d.project();
        assert_eq!(track(&p, &a)["note_effects"].as_array().unwrap().len(), 0);
        assert_eq!(track(&p, &b)["note_effects"].as_array().unwrap().len(), 1);

        // A note tool on a clip that isn't open anywhere.
        let c = d
            .call("add_note_clip", json!({"track": b, "bar": 1}))
            .unwrap();
        let c = c["clip"].as_str().unwrap().to_string();
        d.call(
            "edit_notes",
            json!({"clip": c, "add": [
                {"beat": 0.0, "beats": 0.25, "pitch": 60},
                {"beat": 2.0, "beats": 0.25, "pitch": 64}]}),
        )
        .unwrap();
        d.call("run_command", json!({"id": "legato-notes", "clip": c}))
            .unwrap();
        let p = d.app().project();
        let clip = (p.tracks.iter())
            .flat_map(|t| &t.note_clips)
            .find(|x| x.id == c)
            .unwrap();
        let first = clip.notes.iter().min_by_key(|n| n.start).unwrap();
        assert_eq!(first.len, daw_project::PPQ * 2, "it lasts to the next");
    }

    #[test]
    fn instrument_settings_are_set_by_value() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "drum-machine"}))
            .unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let list = d.call("list_parameters", json!({"track": t})).unwrap();
        let params = list[0]["params"].as_array().unwrap();
        let level = (params.iter())
            .find(|p| p["param"] == "instrument.pad0.level")
            .expect("a pad's level is listed");
        assert_eq!(level["max"], 12.0);
        let pad = d
            .call(
                "list_parameters",
                json!({"track": t, "prefix": "instrument.pad0."}),
            )
            .unwrap();
        let pad = pad[0]["params"].as_array().unwrap();
        assert!(!pad.is_empty() && pad.len() < 60, "{}", pad.len());
        assert!(
            pad.iter()
                .all(|p| p["param"].as_str().unwrap().starts_with("instrument.pad0."))
        );
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.pad0.level", "value": -9.0}),
        )
        .unwrap();
        let app = d.app();
        let i = app.tracks.iter().position(|x| x.key == t).unwrap();
        let s = app.tracks[i].instrument.as_ref().unwrap().settings();
        assert_eq!(s.iter().find(|x| x.0 == "pad0.level").unwrap().2, -9.0);
        assert!(
            d.call(
                "set_parameter",
                json!({"track": t, "param": "instrument.nothing", "value": 1.0}),
            )
            .is_err()
        );
        d.command("undo");
        let app = d.app();
        let s = app.tracks[i].instrument.as_ref().unwrap().settings();
        assert_eq!(s.iter().find(|x| x.0 == "pad0.level").unwrap().2, 0.0);
    }

    #[test]
    fn tracks_go_in_a_control_group() {
        let mut d = Driver::new();
        let t = d.call("add_track", json!({"kind": "audio"})).unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        d.command("add-control-group");
        let p = d.project();
        let g = p["control_groups"][0]["id"].as_str().unwrap().to_string();
        d.call("edit_track", json!({"track": t, "group": g}))
            .unwrap();
        d.call(
            "edit_control_group",
            json!({"group": g, "gain_db": -6.0, "name": "Drums"}),
        )
        .unwrap();
        let p = d.project();
        assert_eq!(track(&p, &t)["group"], g.as_str());
        assert_eq!(p["control_groups"][0]["gain_db"], -6.0);
        assert_eq!(p["control_groups"][0]["name"], "Drums");
        assert!(
            d.call("edit_track", json!({"track": t, "group": "nope"}))
                .is_err()
        );
        d.call("edit_control_group", json!({"group": g, "delete": true}))
            .unwrap();
        let p = d.project();
        assert_eq!(p["control_groups"].as_array().unwrap().len(), 0);
        assert!(track(&p, &t)["group"].is_null());
    }

    #[test]
    fn a_drum_pad_gets_a_sound_from_a_file() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "drum-machine"}))
            .unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let file = std::env::temp_dir().join(format!("pad-sound-{}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&file, spec).unwrap();
        for i in 0..4800 {
            w.write_sample(((i as f32 * 0.05).sin() * 8000.0) as i16)
                .unwrap();
        }
        w.finalize().unwrap();
        let path = file.to_string_lossy().into_owned();

        d.call(
            "set_sound",
            json!({"track": t, "slot": "pad2.layer0", "sound": path}),
        )
        .unwrap();
        let app = d.app();
        let i = app.tracks.iter().position(|x| x.key == t).unwrap();
        let b = app.tracks[i]
            .instrument
            .as_ref()
            .unwrap()
            .builtin_ref()
            .unwrap();
        let s = b.sample("pad2.layer0").expect("the pad has the sound");
        assert_eq!(s.reference, path);
        assert!(s.sample.is_some(), "and it was read");
        use daw_instruments::drum_machine::{NO_FADE, SAMPLE, layer, layer_at};
        assert_eq!(b.params().get(layer_at(2, 0, layer::SOURCE)), SAMPLE);
        assert_eq!(
            b.params().get(layer_at(2, 0, layer::DECAY)),
            NO_FADE,
            "played whole"
        );

        // Clearing it, and slots and files that aren't there.
        d.call(
            "set_sound",
            json!({"track": t, "slot": "pad2.layer0", "sound": ""}),
        )
        .unwrap();
        let app = d.app();
        let b = app.tracks[i]
            .instrument
            .as_ref()
            .unwrap()
            .builtin_ref()
            .unwrap();
        assert!(b.sample("pad2.layer0").is_none());
        d.command("undo");
        let app = d.app();
        let b = app.tracks[i]
            .instrument
            .as_ref()
            .unwrap()
            .builtin_ref()
            .unwrap();
        assert!(b.sample("pad2.layer0").is_some(), "undo brings it back");
        for (slot, sound) in [
            ("pad16.layer0", path.as_str()),
            ("zone0", path.as_str()),
            ("pad0.layer0", "/no/such.wav"),
        ] {
            assert!(
                d.call(
                    "set_sound",
                    json!({"track": t, "slot": slot, "sound": sound})
                )
                .is_err(),
                "{slot} {sound}"
            );
        }
        let _ = std::fs::remove_file(file);
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
