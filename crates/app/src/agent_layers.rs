// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for instruments (roadmap 2.11): which instrument a
//! track or a layer plays, layered instruments (roadmap 3.5), the
//! effects on a drum machine's pads, and where an instrument's extra
//! outputs go. Each is one undo step, as every edit by an assistant is.
//!
//! Choosing an instrument makes it the way the instrument window does;
//! the rest edits the project as the document has it and puts the app
//! in step with it (see [`App::edit_model`]).

use daw_instruments::{Kind, drum_machine as dm};
use daw_project::Project;
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::instrument::Slot;

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

const INSTRUMENT: &str = "The instrument: synth (the simple synth), one that comes with the \
                          app by id (poly-synth, sampler, drum-sampler, drum-machine, slicer), \
                          or a plugin instrument by name (list_effect_kinds gives them).";

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_instrument",
        description: "An instrument track's instrument, or its layers (each with its \
                      instrument, level, pan, note and velocity ranges, transpose and \
                      effects), the effects on a drum machine's pads, and the instrument's \
                      extra outputs with the bus each goes to; for a sampler playing an \
                      SFZ library, the library: its keys, articulations and controls \
                      (mic levels and the like, each a setting). Its settings are in \
                      list_parameters.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "settings": {"type": "boolean", "description": "Also each layer's built-in instrument's settings (the track's own are in list_parameters)."}
            }, "required": ["track"]})
        },
    },
    Tool {
        name: "set_instrument",
        description: "Changes the instrument an instrument track plays, or one of its \
                      layers. For the track, its layers go (with their effects); \"none\" \
                      plays no instrument, sending its notes only to a MIDI output.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "layer": {"type": "string", "description": "A layer's id, to change only that layer's instrument."},
                "instrument": {"type": "string", "description": INSTRUMENT}
            }, "required": ["track", "instrument"]})
        },
    },
    Tool {
        name: "edit_layer",
        description: "Adds, changes, moves or removes a layer of an instrument track's \
                      layered instrument: several instruments playing the same notes, each \
                      those in its note and velocity ranges, moved by its transpose, at its \
                      own level and pan, through its own effects (add_effect with the \
                      layer's id as container). Adding the first layer makes the track's \
                      instrument layer 1. Only what is given changes. Gives the layer's id.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "layer": {"type": "string", "description": "The layer's id; leave out with add."},
                "add": {"type": "boolean", "description": "Adds a layer (the simple synth unless instrument says)."},
                "remove": {"type": "boolean", "description": "Removes the layer and its effects."},
                "instrument": {"type": "string", "description": INSTRUMENT},
                "index": {"type": "integer", "description": "Its place among the layers, from 0."},
                "on": {"type": "boolean"},
                "gain_db": {"type": "number", "description": "-60 to 12."},
                "pan": {"type": "number", "description": "-1 (left) to 1 (right)."},
                "keys": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2,
                         "description": "The lowest and highest notes it plays, 0 to 127 (60 is middle C)."},
                "velocities": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2,
                               "description": "The softest and loudest notes it plays, 1 to 127."},
                "transpose": {"type": "integer", "description": "Semitones, -48 to 48."},
                "settings": {"type": "object", "additionalProperties": {"type": "number"},
                             "description": "The layer's built-in instrument's settings by key, as get_instrument with settings lists them; each kept in its range."}
            }, "required": ["track"]})
        },
    },
    Tool {
        name: "edit_pad_effect",
        description: "Adds, changes or removes an effect on one pad of a drum machine (the \
                      track's instrument, or a layer's). get_instrument lists the pads' \
                      effects; list_effect_kinds gives the kinds (not containers, nor \
                      those that use a file). Gives the effect's id.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "layer": {"type": "string", "description": "A layer's id, for a drum machine in a layer."},
                "pad": {"type": "integer", "description": "The pad, 0 to 15 (from note 36), as set_sound counts them; needed with add."},
                "add": {"type": "string", "description": "The kind of effect to add, by id."},
                "effect": {"type": "string", "description": "The pad effect's id, to change or remove it."},
                "on": {"type": "boolean"},
                "remove": {"type": "boolean"},
                "index": {"type": "integer", "description": "Its place among the pad's effects, from 0."},
                "settings": {"type": "object", "additionalProperties": {"type": "number"},
                             "description": "Settings by key (as list_effect_kinds gives them), each kept in its range."}
            }, "required": ["track"]})
        },
    },
    Tool {
        name: "set_instrument_output",
        description: "Sends one of an instrument's extra outputs (a drum machine's pad \
                      outputs and sends, or a plugin instrument's extra outputs, as \
                      get_instrument lists them) to a bus, or back through the track with \
                      \"none\".",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "output": {"type": "integer", "description": "The output's number, from 1."},
                "to": {"type": "string", "description": "A bus's id, or \"none\"."}
            }, "required": ["track", "output", "to"]})
        },
    },
];

fn text<'a>(args: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("Give {what}."))
}

/// A pair of numbers in `lo..=hi`, lowest first.
fn range(v: &Value, lo: i64, hi: i64, what: &str) -> Result<Option<(u8, u8)>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let bad = || format!("Give {what} as two whole numbers, {lo} to {hi}.");
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(bad)?;
    let (x, y) = (
        a[0].as_i64().ok_or_else(bad)?,
        a[1].as_i64().ok_or_else(bad)?,
    );
    let (x, y) = (x.clamp(lo, hi) as u8, y.clamp(lo, hi) as u8);
    Ok(Some((x.min(y), x.max(y))))
}

/// The track at `key` in `p`, and the instrument to act on: the
/// track's, or layer `layer`'s.
fn instrument_in<'a>(
    p: &'a mut Project,
    key: &str,
    layer: Option<&str>,
) -> Result<&'a mut daw_project::Effect, String> {
    let t = (p.tracks.iter_mut())
        .find(|t| t.id == key)
        .ok_or_else(|| format!("There is no track {key}."))?;
    match layer {
        Some(l) => (t.layers.iter_mut())
            .find(|x| x.id == l)
            .map(|x| &mut x.instrument)
            .ok_or_else(|| format!("The track has no layer {l}.")),
        None => t.instrument.as_mut().ok_or_else(|| {
            if t.layers.is_empty() {
                "The track has no instrument.".to_string()
            } else {
                "The track is layered: give the layer.".to_string()
            }
        }),
    }
}

/// The ids of effects inside `container` on a track, and inside those.
pub(crate) fn inside(effects: &[daw_project::Effect], container: &str) -> Vec<String> {
    let mut gone = vec![container.to_string()];
    let mut k = 0;
    while k < gone.len() {
        let more: Vec<String> = (effects.iter())
            .filter(|e| e.container.as_deref() == Some(&gone[k]))
            .map(|e| e.id.clone())
            .collect();
        gone.extend(more);
        k += 1;
    }
    gone.remove(0);
    gone
}

impl App {
    /// Runs one of the tools here, or `None` if `name` isn't one.
    pub(crate) fn layers_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        Some(match name {
            "set_instrument" => self.set_instrument_tool(args),
            "edit_layer" => self.edit_layer_tool(args),
            "edit_pad_effect" => self.edit_pad_effect_tool(args),
            "set_instrument_output" => self.set_instrument_output_tool(args),
            _ => return None,
        })
    }

    /// Answers one of the reading tools here, or `None` if `name` isn't
    /// one.
    pub(crate) fn layers_read(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        Some(match name {
            "get_instrument" => self.instrument_value(args),
            _ => return None,
        })
    }

    /// The instrument track at `key`.
    fn note_track(&self, key: &str) -> Result<usize, String> {
        let i = self
            .track_index(key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        if !self.tracks[i].kind.plays_notes() {
            return Err(format!(
                "{} isn't an instrument track.",
                self.tracks[i].name
            ));
        }
        Ok(i)
    }

    /// A new instrument as `which` names it, its audio side made.
    fn new_instrument(&mut self, which: &str) -> Result<Slot, String> {
        let s = match which {
            crate::instrument::SYNTH | "simple-synth" | "instrument" => Slot::synth(),
            other => match Kind::from_id(other) {
                Some(k) => Slot::new(k),
                None => {
                    let a = (self.effects.list.plugins().into_iter())
                        .filter(|p| p.enabled && p.class.is_instrument())
                        .find(|p| p.class.name.eq_ignore_ascii_case(other))
                        .ok_or_else(|| {
                            format!("There is no instrument {other}; list_effect_kinds gives the plugins.")
                        })?;
                    return Slot::new_plugin(&mut self.effects, &a, self.rate)
                        .map_err(|e| format!("{}: {e}", a.class.name));
                }
            },
        };
        s.make_into(self.rate, &mut self.pending_instruments);
        Ok(s)
    }

    fn instrument_value(&mut self, args: &Value) -> Result<Value, String> {
        let key = text(args, "track", "the track's id")?;
        let i = self.note_track(key)?;
        let p = self.project();
        let pt = &p.tracks[i];
        let t = &self.tracks[i];
        let instrument = |e: &daw_project::Effect| {
            let kind = if e.is_plugin() {
                "plugin"
            } else {
                e.builtin.as_str()
            };
            let pads: Vec<Value> = (e.inner.iter())
                .map(|f| {
                    json!({"id": f.id, "pad": f.branch, "kind": f.builtin,
                           "on": !f.bypass, "settings": f.params.iter()
                               .map(|(k, v)| (k.clone(), json!(v)))
                               .collect::<serde_json::Map<_, _>>()})
                })
                .collect();
            json!({"id": e.id, "name": e.name, "kind": kind, "pad_effects": pads})
        };
        let effects_of = |id: &str| {
            let mut ids = inside(&pt.effects, id);
            ids.sort_by_key(|x| pt.effects.iter().position(|e| &e.id == x));
            ids
        };
        let with_settings = args["settings"].as_bool() == Some(true);
        let layers: Vec<Value> = (pt.layers.iter().zip(&t.layers))
            .map(|(l, live)| {
                let mut v = json!({
                    "id": l.id,
                    "instrument": instrument(&l.instrument),
                    "on": !l.mute,
                    "gain_db": l.gain_db,
                    "pan": l.pan,
                    "keys": [l.keys.0, l.keys.1],
                    "velocities": [l.velocities.0, l.velocities.1],
                    "transpose": l.transpose,
                    "effects": effects_of(&l.id),
                });
                if with_settings {
                    v["settings"] = (live.slot.settings().into_iter())
                        .map(|(k, name, v, min, max, text)| {
                            json!({"key": k, "name": name, "value": v, "min": min,
                                   "max": max, "text": text})
                        })
                        .collect();
                }
                v
            })
            .collect();
        let outputs: Vec<Value> = (t.instrument.as_ref())
            .map(Slot::outputs_in_use)
            .unwrap_or_default()
            .into_iter()
            .map(|(o, name)| {
                let to = (pt.instrument_outputs.iter())
                    .find(|x| x.output == o)
                    .map(|x| x.to.clone());
                json!({"output": o, "name": name, "to": to})
            })
            .collect();
        let library = (t.instrument.as_ref())
            .and_then(Slot::builtin_ref)
            .and_then(|b| b.library().map(|l| (b, l)))
            .map(|(b, l)| {
                let p = b.params();
                let controls: Vec<Value> = (p.list().iter().enumerate())
                    .filter(|(_, x)| x.key.starts_with("cc"))
                    .map(|(i, x)| {
                        json!({"param": format!("instrument.{}", x.key), "name": x.name,
                               "value": p.get(i)})
                    })
                    .collect();
                let switches: Vec<Value> = (l.sfz.key_switches().into_iter())
                    .map(|(k, name)| json!({"key": k, "name": name}))
                    .collect();
                let keys: Vec<Value> = (0..128u8)
                    .filter(|&k| !l.by_key[k as usize].is_empty())
                    .map(|k| {
                        let name = l.key_name(k);
                        json!({"key": k, "name": name, "regions": l.by_key[k as usize].len()})
                    })
                    .collect();
                json!({
                    "file": l.path.display().to_string(),
                    "name": l.name(),
                    "regions": l.sfz.regions.len(),
                    "sounds": l.sources.len(),
                    "memory_mb": l.bytes >> 20,
                    "from_disk": l.streamed,
                    "keys": keys,
                    "articulations": switches,
                    "articulation": p.get_key("keyswitch"),
                    "controls": controls,
                    "normalise": p.get_key("normalise").map(|v| v >= 0.5),
                    "not_played": l.sfz.unsupported.keys().collect::<Vec<_>>(),
                    "problems": l.problems,
                })
            });
        Ok(json!({
            "track": key,
            "instrument": pt.instrument.as_ref().map(instrument),
            "library": library,
            "layers": layers,
            "outputs": outputs,
        }))
    }

    fn set_instrument_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?;
        let which = text(args, "instrument", "the instrument")?;
        let ti = self.note_track(key)?;
        let layer = args["layer"].as_str();
        let n = match layer {
            Some(l) => Some(
                (self.tracks[ti].layers.iter())
                    .position(|x| x.key == l)
                    .ok_or_else(|| format!("The track has no layer {l}."))?,
            ),
            None => None,
        };
        let slot = match which {
            "none" if n.is_none() => None,
            "none" => return Err("A layer has to have an instrument; remove it instead.".into()),
            w => Some(self.new_instrument(w)?),
        };
        let name = slot
            .as_ref()
            .map_or("no instrument".into(), |s| s.name.clone());
        let id = slot.as_ref().map(|s| s.key.clone());
        let t = &mut self.tracks[ti];
        match (n, slot) {
            (Some(n), Some(s)) => t.layers[n].slot = s,
            (_, s) => {
                crate::layers::drop_layers(t);
                if s.as_ref().is_none_or(Slot::is_plugin) {
                    // Where another instrument's outputs went means
                    // nothing to this one.
                    t.instrument_outputs.clear();
                }
                t.instrument = s;
            }
        }
        self.rebuild();
        Ok((
            json!({"track": key, "layer": layer, "instrument": id, "name": name}),
            Some(format!("Play {name}")),
        ))
    }

    fn edit_layer_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let ti = self.note_track(&key)?;
        let mut label = "Change a layer";
        let layer = if args["add"].as_bool() == Some(true) {
            let which = args["instrument"]
                .as_str()
                .unwrap_or(crate::instrument::SYNTH);
            let slot = self.new_instrument(which)?;
            let t = &mut self.tracks[ti];
            if t.layers.is_empty()
                && let Some(first) = t.instrument.take()
            {
                t.layers.push(crate::layers::Layer::new(first));
            }
            let l = crate::layers::Layer::new(slot);
            let id = l.key.clone();
            t.layers.push(l);
            self.rebuild();
            label = "Add a layer";
            id
        } else {
            let l = text(args, "layer", "the layer's id, or add")?.to_string();
            let n = (self.tracks[ti].layers.iter())
                .position(|x| x.key == l)
                .ok_or_else(|| format!("The track has no layer {l}."))?;
            if let Some(which) = args["instrument"].as_str() {
                let s = self.new_instrument(which)?;
                self.tracks[ti].layers[n].slot = s;
                self.rebuild();
            }
            l
        };
        if args["remove"].as_bool() == Some(true) {
            self.edit_model(|p| {
                let t = &mut p.tracks[ti];
                let gone = inside(&t.effects, &layer);
                t.effects.retain(|e| !gone.contains(&e.id));
                t.layers.retain(|l| l.id != layer);
                Ok(json!({"removed": layer, "effects_removed": gone}))
            })
            .map(|v| (v, Some("Remove a layer".into())))
        } else {
            let settings = match args["settings"].as_object() {
                None => Vec::new(),
                Some(m) => {
                    let n = (self.tracks[ti].layers.iter())
                        .position(|x| x.key == layer)
                        .expect("the layer is there");
                    let have = self.tracks[ti].layers[n].slot.settings();
                    let mut out = Vec::new();
                    for (k, v) in m {
                        let (_, _, _, min, max, _) = (have.iter())
                            .find(|x| &x.0 == k)
                            .ok_or_else(|| format!("The layer's instrument has no setting {k}."))?;
                        let v = v.as_f64().ok_or("Settings are numbers.")?;
                        out.push((k.clone(), v.clamp(*min, *max)));
                    }
                    out
                }
            };
            let keys = range(&args["keys"], 0, 127, "the notes")?;
            let vels = range(&args["velocities"], 1, 127, "the velocities")?;
            self.edit_model(|p| {
                let t = &mut p.tracks[ti];
                let n = (t.layers.iter())
                    .position(|l| l.id == layer)
                    .expect("the layer is there");
                let l = &mut t.layers[n];
                if let Some(on) = args["on"].as_bool() {
                    l.mute = !on;
                }
                if let Some(g) = args["gain_db"].as_f64() {
                    l.gain_db = g.clamp(-60.0, 12.0);
                }
                if let Some(pan) = args["pan"].as_f64() {
                    l.pan = pan.clamp(-1.0, 1.0);
                }
                if let Some(k) = keys {
                    l.keys = k;
                }
                if let Some(v) = vels {
                    l.velocities = v;
                }
                for (k, v) in settings {
                    let params = &mut l.instrument.params;
                    params.retain(|(x, _)| *x != k);
                    params.push((k, v));
                }
                if let Some(s) = args["transpose"].as_i64() {
                    let most = daw_engine::layers::MAX_TRANSPOSE as i64;
                    l.transpose = s.clamp(-most, most) as i32;
                }
                if let Some(to) = args["index"].as_i64() {
                    let l = t.layers.remove(n);
                    let to = (to.max(0) as usize).min(t.layers.len());
                    t.layers.insert(to, l);
                }
                let index = t.layers.iter().position(|l| l.id == layer);
                Ok(json!({"layer": layer, "index": index}))
            })
            .map(|v| (v, Some(label.into())))
        }
    }

    fn edit_pad_effect_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        self.note_track(&key)?;
        let layer = args["layer"].as_str();
        let pad = match args["pad"].as_i64() {
            Some(p) if (0..dm::PADS as i64).contains(&p) => Some(p as u32),
            Some(_) => return Err(format!("A pad is 0 to {}.", dm::PADS - 1)),
            None => None,
        };
        let mut label = "Change a pad effect";
        self.edit_model(|p| {
            let e = instrument_in(p, &key, layer)?;
            if e.builtin != Kind::DrumMachine.id() {
                return Err("Only a drum machine has pad effects.".into());
            }
            let fx = &mut e.inner;
            let id = if let Some(kind) = args["add"].as_str() {
                let k = daw_effects::Kind::from_id(kind)
                    .filter(|k| !k.is_container() && k.uses() == daw_effects::Uses::Nothing)
                    .ok_or_else(|| format!("{kind} can't go on a pad."))?;
                let pad = pad.ok_or("Give the pad, 0 to 15.")?;
                let id = daw_project::new_id();
                fx.push(daw_project::Effect {
                    id: id.clone(),
                    name: k.name().into(),
                    builtin: k.id().into(),
                    params: k
                        .params()
                        .iter()
                        .map(|p| (p.key.into(), p.default))
                        .collect(),
                    branch: pad,
                    ..daw_project::Effect::default()
                });
                label = "Add a pad effect";
                id
            } else {
                text(args, "effect", "the pad effect's id, or add")?.to_string()
            };
            let j = (fx.iter().position(|f| f.id == id))
                .ok_or_else(|| format!("There is no pad effect {id}."))?;
            if args["remove"].as_bool() == Some(true) {
                fx.remove(j);
                label = "Remove a pad effect";
                return Ok(json!({"removed": id}));
            }
            let k = daw_effects::Kind::from_id(&fx[j].builtin)
                .ok_or("That effect isn't known to this version.")?;
            if let Some(on) = args["on"].as_bool() {
                fx[j].bypass = !on;
            }
            if let Some(s) = args["settings"].as_object() {
                for (name, v) in s {
                    let p = (k.params().iter())
                        .find(|p| p.key == name)
                        .ok_or_else(|| format!("{} has no setting {name}.", k.name()))?;
                    let v = v
                        .as_f64()
                        .ok_or("Settings are numbers.")?
                        .clamp(p.min, p.max);
                    let params = &mut fx[j].params;
                    match params.iter_mut().find(|x| x.0 == name.as_str()) {
                        Some(x) => x.1 = v,
                        None => params.push((name.clone(), v)),
                    }
                }
            }
            if let Some(to) = args["index"].as_i64() {
                // Among the same pad's effects.
                let pad = fx[j].branch;
                let f = fx.remove(j);
                let same: Vec<usize> = (0..fx.len()).filter(|&i| fx[i].branch == pad).collect();
                let at = match same.get(to.max(0) as usize) {
                    Some(&i) => i,
                    None => same.last().map_or(fx.len(), |&i| i + 1),
                };
                fx.insert(at, f);
            }
            let f = fx.iter().find(|f| f.id == id).expect("still there");
            Ok(json!({"effect": id, "pad": f.branch, "on": !f.bypass}))
        })
        .map(|v| (v, Some(label.into())))
    }

    fn set_instrument_output_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?.to_string();
        let to = text(args, "to", "a bus's id, or none")?.to_string();
        let ti = self.note_track(&key)?;
        let output = args["output"]
            .as_u64()
            .filter(|&o| o >= 1)
            .ok_or("Give the output's number, from 1.")? as u32;
        let known = (self.tracks[ti].instrument.as_ref())
            .map(|s| {
                let mut v: Vec<u32> = s.outputs_in_use().into_iter().map(|(o, _)| o).collect();
                // A plugin's outputs are all there, used or not.
                if let Some(p) = s.plugin().and_then(|fx| fx.plugin()) {
                    v.extend(p.buses().extra_outputs().map(|(i, _)| i as u32));
                }
                v
            })
            .unwrap_or_default();
        if !known.contains(&output) {
            return Err(format!(
                "The instrument has no output {output} in use; get_instrument lists them."
            ));
        }
        self.edit_model(|p| {
            if to != "none" {
                let b = (p.tracks.iter())
                    .find(|t| t.id == to)
                    .ok_or_else(|| format!("There is no track {to}."))?;
                if !b.kind.is_bus() || b.id == key {
                    return Err(format!("{to} isn't a bus."));
                }
            }
            let outs = &mut p.tracks[ti].instrument_outputs;
            outs.retain(|o| o.output != output);
            if to != "none" {
                outs.push(daw_project::InstrumentOutput {
                    output,
                    to: to.clone(),
                });
                outs.sort_by_key(|o| o.output);
            }
            Ok(json!({"output": output, "to": (to != "none").then_some(&to)}))
        })
        .map(|v| (v, Some("Route an instrument output".into())))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;

    fn id(v: Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    fn instrument(d: &mut Driver, t: &str) -> Value {
        d.call("get_instrument", json!({"track": t})).unwrap()
    }

    #[test]
    fn layers_are_added_set_given_effects_and_removed_each_in_one_step() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        let bass = id(
            d.call(
                "edit_layer",
                json!({"track": t, "add": true, "instrument": "poly-synth",
                       "keys": [0, 59], "transpose": -12, "gain_db": -6.0}),
            )
            .unwrap(),
            "layer",
        );
        let i = instrument(&mut d, &t);
        let layers = i["layers"].as_array().unwrap();
        // The simple synth the track had is layer 1; the new one is 2.
        assert_eq!(layers.len(), 2, "{i}");
        assert_eq!(layers[0]["instrument"]["kind"], "synth");
        assert_eq!(layers[1]["id"], bass);
        assert_eq!(layers[1]["instrument"]["kind"], "poly-synth");
        assert_eq!(layers[1]["keys"], json!([0, 59]));
        assert_eq!(layers[1]["transpose"], -12);
        assert_eq!(layers[1]["gain_db"], -6.0);
        assert_eq!(i["instrument"], json!(null));
        // The app plays them: the engine has both, with their ranges.
        let app = d.app();
        let ti = app.track_index(&t).unwrap();
        assert_eq!(app.tracks[ti].layers[1].controls.keys(), (0, 59));
        assert_eq!(app.tracks[ti].layers[1].controls.transpose(), -12);

        // Its instrument's settings, by value.
        let i = d
            .call("get_instrument", json!({"track": t, "settings": true}))
            .unwrap();
        let s = i["layers"][1]["settings"][0].clone();
        let k = s["key"].as_str().unwrap().to_string();
        let max = s["max"].as_f64().unwrap();
        d.call(
            "edit_layer",
            json!({"track": t, "layer": bass, "settings": {k.clone(): max + 1.0}}),
        )
        .unwrap();
        let i = d
            .call("get_instrument", json!({"track": t, "settings": true}))
            .unwrap();
        assert_eq!(i["layers"][1]["settings"][0]["value"], max, "kept in range");
        assert!(
            d.call(
                "edit_layer",
                json!({"track": t, "layer": bass, "settings": {"no-such": 1.0}})
            )
            .is_err()
        );

        // A layer has its own effects.
        let eq = id(
            d.call(
                "add_effect",
                json!({"track": t, "effect": "eq", "container": bass}),
            )
            .unwrap(),
            "effect",
        );
        assert_eq!(instrument(&mut d, &t)["layers"][1]["effects"], json!([eq]));
        let fx = d.call("get_effects", json!({"track": t})).unwrap();
        assert_eq!(fx["effects"][0]["container"], bass);

        // Moved first, switched off, then changed to another instrument.
        d.call(
            "edit_layer",
            json!({"track": t, "layer": bass, "index": 0, "on": false,
                   "velocities": [100, 20], "instrument": "sampler"}),
        )
        .unwrap();
        let l = &instrument(&mut d, &t)["layers"][0];
        assert_eq!(
            (l["id"].clone(), l["on"].clone(), l["velocities"].clone()),
            (json!(bass), json!(false), json!([20, 100]))
        );
        assert_eq!(l["instrument"]["kind"], "sampler");
        assert_eq!(l["effects"], json!([eq]));

        // Removing it takes its effects; undo brings both back.
        d.call(
            "edit_layer",
            json!({"track": t, "layer": bass, "remove": true}),
        )
        .unwrap();
        let i = instrument(&mut d, &t);
        assert_eq!(i["layers"].as_array().unwrap().len(), 1);
        let p = d.project();
        assert_eq!(p["tracks"][0]["effects"], json!([]));
        d.command("undo");
        let i = instrument(&mut d, &t);
        assert_eq!(i["layers"][0]["id"], bass);
        assert_eq!(i["layers"][0]["effects"], json!([eq]));
        assert!(
            d.call(
                "edit_layer",
                json!({"track": t, "layer": "nope", "on": true})
            )
            .is_err()
        );
    }

    #[test]
    fn a_track_changes_instrument_and_a_drum_pad_gets_effects() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        d.call("edit_layer", json!({"track": t, "add": true}))
            .unwrap();
        let r = d
            .call(
                "set_instrument",
                json!({"track": t, "instrument": "drum-machine"}),
            )
            .unwrap();
        assert_eq!(r["name"], "Drum machine", "{r}");
        let i = instrument(&mut d, &t);
        assert_eq!(i["layers"], json!([]), "the layers go");
        assert_eq!(i["instrument"]["kind"], "drum-machine");
        assert!(
            d.call("set_instrument", json!({"track": t, "instrument": "kazoo"}))
                .is_err()
        );

        // An effect on pad 2, set, moved, then switched off.
        let k = daw_effects::Kind::Saturation;
        let p0 = k.params()[0];
        let e = id(
            d.call(
                "edit_pad_effect",
                json!({"track": t, "pad": 1, "add": "saturation",
                       "settings": {p0.key: p0.max + 100.0}}),
            )
            .unwrap(),
            "effect",
        );
        let pads = &instrument(&mut d, &t)["instrument"]["pad_effects"];
        assert_eq!(pads[0]["pad"], 1, "{pads}");
        assert_eq!(pads[0]["kind"], "saturation");
        assert_eq!(pads[0]["settings"][p0.key], p0.max, "kept in its range");
        // The live instrument has it on that pad.
        let app = d.app();
        let ti = app.track_index(&t).unwrap();
        let b = app.tracks[ti]
            .instrument
            .as_ref()
            .unwrap()
            .builtin_ref()
            .unwrap();
        assert_eq!((b.fx.len(), b.fx[0].pad), (1, 1));

        d.call(
            "edit_pad_effect",
            json!({"track": t, "effect": e, "on": false}),
        )
        .unwrap();
        assert_eq!(
            instrument(&mut d, &t)["instrument"]["pad_effects"][0]["on"],
            false
        );
        assert!(
            d.call(
                "edit_pad_effect",
                json!({"track": t, "pad": 1, "add": "parallel"})
            )
            .is_err(),
            "no containers on a pad"
        );
        d.call(
            "edit_pad_effect",
            json!({"track": t, "effect": e, "remove": true}),
        )
        .unwrap();
        assert_eq!(
            instrument(&mut d, &t)["instrument"]["pad_effects"],
            json!([])
        );
        d.command("undo");
        assert_eq!(
            instrument(&mut d, &t)["instrument"]["pad_effects"][0]["id"],
            e
        );

        // Back to no instrument.
        d.call("set_instrument", json!({"track": t, "instrument": "none"}))
            .unwrap();
        assert_eq!(instrument(&mut d, &t)["instrument"], json!(null));
        assert!(
            d.call(
                "edit_pad_effect",
                json!({"track": t, "pad": 1, "add": "eq"})
            )
            .is_err()
        );
    }

    #[test]
    fn a_drum_machine_output_goes_to_a_bus() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "drum-machine"}))
                .unwrap(),
            "track",
        );
        let bus = id(
            d.call("add_track", json!({"kind": "bus"})).unwrap(),
            "track",
        );
        // No pad uses output 3 yet.
        assert!(
            d.call(
                "set_instrument_output",
                json!({"track": t, "output": 3, "to": bus})
            )
            .is_err()
        );
        {
            use daw_instruments::drum_machine as dm;
            let app = d.app();
            let ti = app.track_index(&t).unwrap();
            let b = app.tracks[ti]
                .instrument
                .as_ref()
                .unwrap()
                .builtin_ref()
                .unwrap();
            b.params().set(dm::pad_at(0, dm::pad::OUTPUT), 3.0);
        }
        let outs = instrument(&mut d, &t)["outputs"].clone();
        assert_eq!(outs[0]["output"], 3, "{outs}");
        d.call(
            "set_instrument_output",
            json!({"track": t, "output": 3, "to": bus}),
        )
        .unwrap();
        assert_eq!(instrument(&mut d, &t)["outputs"][0]["to"], bus);
        assert!(
            d.call(
                "set_instrument_output",
                json!({"track": t, "output": 3, "to": t})
            )
            .is_err(),
            "only to a bus"
        );
        d.call(
            "set_instrument_output",
            json!({"track": t, "output": 3, "to": "none"}),
        )
        .unwrap();
        assert_eq!(instrument(&mut d, &t)["outputs"][0]["to"], json!(null));
    }
}

/// An assistant loading an SFZ library onto a sampler and mixing it.
#[cfg(test)]
mod sfz_tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    fn kit() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-sfz-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, level) in [("close.wav", 0.5f32), ("room.wav", 0.25)] {
            let mut w = hound::WavWriter::create(
                dir.join(name),
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 48_000,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                },
            )
            .unwrap();
            for i in 0..96_000 {
                w.write_sample(level * (i as f32 * 0.05).sin()).unwrap();
            }
            w.finalize().unwrap();
        }
        std::fs::write(
            dir.join("Drums.sfz"),
            "<control> label_cc70=Kick close set_cc70=127 label_cc71=Kick room set_cc71=127\n\
             <global> loop_mode=one_shot global_label=Kick\n\
             <region> key=36 sample=close.wav amplitude_cc70=100 locc70=1\n\
             <region> key=36 sample=room.wav amplitude_cc71=100 locc71=1\n",
        )
        .unwrap();
        dir
    }

    fn rms(d: &mut Driver, t: &str) -> f64 {
        d.call(
            "analyse",
            json!({"track": t, "from_seconds": 0.0, "to_seconds": 1.0}),
        )
        .unwrap()["rms_dbfs"]
            .as_f64()
            .unwrap()
    }

    #[test]
    fn an_assistant_loads_an_sfz_library_and_sets_its_mics() {
        let dir = kit();
        let mut d = Driver::new();
        let path = dir.join("Drums.sfz");
        let r = d
            .call("file_command", json!({"id": "load-sfz", "path": path}))
            .unwrap();
        assert!(r["message"].as_str().unwrap().contains("2 regions"), "{r}");
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(d.project()["tracks"][0]["name"], "Drums");
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        let lib = &i["library"];
        assert_eq!(lib["regions"], 2, "{i}");
        assert_eq!(lib["keys"][0]["key"], 36);
        assert_eq!(lib["keys"][0]["name"], "Kick", "{lib}");
        assert_eq!(lib["controls"][0]["param"], "instrument.cc70");
        assert_eq!(lib["controls"][1]["name"], "Kick room");
        // A kick on the first beat.
        {
            let a = d.app();
            let tr = &mut a.tracks[0];
            let mut c = daw_project::NoteClip::new("n".into(), "Kick".into(), 0, 960);
            c.notes.push(daw_project::Note::new(0, 480, 36, 127));
            tr.note_clips.push(c);
            a.rebuild();
        }
        let both = rms(&mut d, &t);
        assert!(both > -40.0, "{both}");
        // The room mic off: quieter, and one undo step back.
        d.call(
            "set_parameter",
            json!({"track": t, "param": "instrument.cc71", "value": 0}),
        )
        .unwrap();
        let close = rms(&mut d, &t);
        assert!(close < both - 1.0 && close > -40.0, "{both} {close}");
        d.call("undo_steps", json!({"undo": 1})).unwrap();
        let back = rms(&mut d, &t);
        assert!((back - both).abs() < 0.5, "{both} {back}");
        // Back to its own zones, and the library again by set_sound.
        d.call("run_command", json!({"id": "unload-sfz", "track": t}))
            .unwrap();
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert!(i["library"].is_null(), "{i}");
        d.call(
            "set_sound",
            json!({"track": t, "slot": "sfz", "sound": path}),
        )
        .unwrap();
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["library"]["sounds"], 2, "{i}");
    }
}

/// The CC0 drum library the sampler was tested with, played by an
/// assistant: set DAW_BIG_RUSTY to its folder to run this, and
/// DAW_BIG_RUSTY_OUT to a file to keep what it played.
#[cfg(test)]
mod big_rusty {
    use serde_json::json;

    use crate::agent::driver::Driver;

    #[test]
    fn an_assistant_plays_a_beat_on_big_rusty_drums() {
        let Some(dir) = std::env::var_os("DAW_BIG_RUSTY").map(std::path::PathBuf::from) else {
            return;
        };
        let mut d = Driver::new();
        let path = dir.join("Programs/01-full.sfz");
        let r = d
            .call("file_command", json!({"id": "load-sfz", "path": path}))
            .unwrap();
        eprintln!("{}", r["message"]);
        let t = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        let lib = &i["library"];
        eprintln!(
            "{} keys, {} controls, not played: {}",
            lib["keys"].as_array().unwrap().len(),
            lib["controls"].as_array().unwrap().len(),
            lib["not_played"]
        );
        // Two bars at 120: kick, snare, eighth-note hats getting
        // harder, then a crash.
        {
            let a = d.app();
            let tr = &mut a.tracks[0];
            let mut c = daw_project::NoteClip::new("n".into(), "Beat".into(), 0, 8 * 960);
            for beat in 0..8i64 {
                let at = beat * 960;
                let kick = if beat % 2 == 0 { 36 } else { 38 };
                c.notes.push(daw_project::Note::new(at, 240, kick, 110));
                for half in 0..2 {
                    let v = (40 + beat * 10 + half * 5) as u8;
                    c.notes
                        .push(daw_project::Note::new(at + half * 480, 120, 42, v));
                }
            }
            c.notes
                .push(daw_project::Note::new(8 * 960 - 1, 240, 49, 120));
            tr.note_clips.push(c);
            a.rebuild();
        }
        let a = d
            .call(
                "analyse",
                json!({"track": t, "from_seconds": 0.0, "to_seconds": 6.0}),
            )
            .unwrap();
        eprintln!(
            "peak {} dBFS, true peak {}, rms {} dBFS, loudness {} LUFS",
            a["sample_peak_dbfs"], a["true_peak_dbtp"], a["rms_dbfs"], a["integrated_lufs"]
        );
        assert!(a["sample_peak_dbfs"].as_f64().unwrap() > -20.0, "{a}");
        assert!(a["sample_peak_dbfs"].as_f64().unwrap() < 0.0, "{a}");
        if let Some(out) = std::env::var_os("DAW_BIG_RUSTY_OUT") {
            let r = d
                .call(
                    "render_audio",
                    json!({"from_seconds": 0.0, "to_seconds": 6.0}),
                )
                .unwrap();
            std::fs::copy(r["file"].as_str().unwrap(), out).unwrap();
        }
    }
}
