// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for how a track's effects are wired (roadmap
//! 2.11): where each sits (the track's chain, a layer's, or a chain of
//! an effect container), what it listens to (a sidechain), a plugin's
//! extra outputs and presets, and the files a reverb, cabinet or amp
//! capture player uses. `add_effect` and `edit_effect` (agent_edit.rs)
//! take the same placing and wiring through the helpers here.

use std::path::Path;

use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::agent_layers::inside;
use crate::fx::FxSlot;

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_effects",
        description: "A track's effects as the effects window shows them, in order: each \
                      with where it sits (container: a layer's or a container's id, and \
                      branch: which of the container's chains, from 0), its chain count if \
                      it is a container (and for a switching container, heard: the one \
                      chain playing, from 0; set it with set_parameter, as its \
                      fx:<id>:heard setting, or the next-chain and hear-chain-N commands), \
                      the track it listens to (sidechain) if it can, a \
                      plugin's extra outputs and which are heard, the file a reverb, \
                      cabinet or amp capture player uses, and a plugin's presets and \
                      programs. Also the recorded rooms a reverb can use.",
        access: Access::Read,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"}
            }, "required": ["track"]})
        },
    },
    Tool {
        name: "plugin_preset",
        description: "Loads, saves or deletes a plugin effect's preset, or picks one of its \
                      programs, as get_effects lists them. Saving keeps the plugin's \
                      settings now under a name of the user's own; only those can be \
                      deleted.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "track": {"type": "string"},
                "effect": {"type": "string", "description": "The plugin effect's id."},
                "load": {"type": "string", "description": "A preset's name."},
                "save": {"type": "string", "description": "A name for a new preset."},
                "delete": {"type": "string", "description": "A preset of the user's own, by name."},
                "program": {"type": "string", "description": "A program's name."},
                "program_list": {"type": "string", "description": "Which program list, by name, if the plugin has more than one."}
            }, "required": ["track", "effect"]})
        },
    },
];

/// The schema of `add_effect`'s and `edit_effect`'s placing.
pub(crate) fn place_schema() -> Value {
    json!({
        "container": {"type": "string", "description": "Puts it in a layer's chain (the layer's \
                      id) or a chain of an effect container on the track (the container's id), \
                      or \"none\" for the track's own chain."},
        "branch": {"type": "integer", "description": "Which of the container's chains, from 0."}
    })
}

/// The schema of `edit_effect`'s wiring.
pub(crate) fn wiring_schema() -> Value {
    json!({
        "sidechain": {"type": "string", "description": "The track it listens to (for a compressor, \
                      gate or plugin with a side input), or \"none\" for its own audio."},
        "outputs": {"type": "array", "items": {"type": "integer"}, "description": "Which of a \
                    plugin's extra outputs are mixed into the track, by number as get_effects \
                    lists them."},
        "file": {"type": "string", "description": "For a convolution reverb or speaker cabinet, \
                 its impulse response; for an amp capture player, its capture (.nam): a full \
                 path, a recorded room as \"room:\" and its id, or \"\" for its own rooms or \
                 cabinets (or no capture)."}
    })
}

fn text<'a>(args: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("Give {what}."))
}

impl App {
    /// Runs one of the tools here, or `None` if `name` isn't one.
    pub(crate) fn routing_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        Some(match name {
            "plugin_preset" => self.plugin_preset_tool(args),
            _ => return None,
        })
    }

    /// Answers one of the reading tools here, or `None` if `name` isn't
    /// one.
    pub(crate) fn routing_read(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        Some(match name {
            "get_effects" => self.effects_value(args),
            _ => return None,
        })
    }

    /// Where `args` places an effect on track `ti` (`None` for the master
    /// channel): `None` if they don't say, else the container and chain
    /// (`None` for the track's own). `moving` is the effect being moved,
    /// which can't go inside itself.
    pub(crate) fn place_arg(
        &self,
        ti: Option<usize>,
        args: &Value,
        moving: Option<&str>,
    ) -> Result<Option<Option<(String, u32)>>, String> {
        let Some(c) = args["container"].as_str() else {
            if args["branch"].is_number() {
                return Err("Give the container with its branch.".into());
            }
            return Ok(None);
        };
        if c == "none" {
            return Ok(Some(None));
        }
        let t = ti.map_or(&self.master, |ti| &self.tracks[ti]);
        let branch = args["branch"].as_u64().unwrap_or(0) as u32;
        if t.layers.iter().any(|l| l.key == c) {
            if branch != 0 {
                return Err("A layer has one chain: branch 0.".into());
            }
            return Ok(Some(Some((c.into(), 0))));
        }
        let s = (t.fx.iter())
            .find(|s| s.key == c)
            .ok_or_else(|| format!("{c} is neither a layer nor an effect on the track."))?;
        let chains = s.chains();
        if chains == 0 {
            return Err(format!("{} isn't a container.", s.name));
        }
        if branch as usize >= chains {
            return Err(format!("{} has chains 0 to {}.", s.name, chains - 1));
        }
        if let Some(m) = moving {
            let p = self.project();
            let effects = ti.map_or(&p.master.effects, |ti| &p.tracks[ti].effects);
            let under = inside(effects, m);
            if c == m || under.iter().any(|x| x == c) {
                return Err("An effect can't go inside itself.".into());
            }
        }
        Ok(Some(Some((c.into(), branch))))
    }

    /// Checks `edit_effect`'s wiring for effect `slot` on track `ti`
    /// (`None` for the master channel).
    pub(crate) fn check_wiring(
        &self,
        ti: Option<usize>,
        slot: &FxSlot,
        args: &Value,
    ) -> Result<(), String> {
        if let Some(sc) = args["sidechain"].as_str() {
            if !slot.takes_key() {
                return Err(format!("{} doesn't listen to another track.", slot.name));
            }
            if ti.is_none() && sc != "none" {
                return Err("An effect on the master can't listen to a track.".into());
            }
            if sc != "none" {
                let j = self
                    .track_index(sc)
                    .ok_or_else(|| format!("There is no track {sc}."))?;
                if Some(j) == ti {
                    return Err("An effect listens to its own track's audio already.".into());
                }
            }
        }
        if !args["outputs"].is_null() {
            let outs = args["outputs"]
                .as_array()
                .ok_or("Give the outputs as a list.")?;
            let have = slot.extra_outputs();
            for o in outs {
                let o = o.as_u64().ok_or("Outputs are numbers.")? as u32;
                if !have.iter().any(|(i, _)| *i == o) {
                    return Err(format!("{} has no extra output {o}.", slot.name));
                }
            }
        }
        if let Some(f) = args["file"].as_str() {
            match slot.uses() {
                daw_effects::Uses::Nothing => {
                    return Err(format!("{} uses no file.", slot.name));
                }
                daw_effects::Uses::Capture if f.starts_with("room:") => {
                    return Err("An amp capture player takes a capture file.".into());
                }
                _ if f.starts_with("room:") => {
                    if slot.builtin().map(daw_effects::Effect::kind)
                        != Some(daw_effects::Kind::Convolution)
                        || daw_effects::library::find(f).is_none()
                    {
                        return Err(format!("There is no recorded room {f} for this effect."));
                    }
                }
                _ if !f.is_empty() && !Path::new(f).is_file() => {
                    return Err(format!("There is no file {f}."));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn effects_value(&mut self, args: &Value) -> Result<Value, String> {
        let key = text(args, "track", "the track's id")?;
        let ti = self
            .track_index(key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        let p = self.project();
        let dir = self.effects.presets_dir.clone();
        let t = &self.tracks[ti];
        let layers: Vec<Value> = (t.layers.iter().enumerate())
            .map(|(n, l)| json!({"id": l.key, "name": format!("Layer {}: {}", n + 1, l.slot.name)}))
            .collect();
        let effects: Vec<Value> = (p.tracks[ti].effects.iter())
            .map(|e| {
                let mut v = json!({
                    "id": e.id,
                    "name": e.name,
                    "builtin": (!e.builtin.is_empty()).then_some(&e.builtin),
                    "on": !e.bypass,
                    "container": e.container,
                    "branch": e.container.is_some().then_some(e.branch),
                });
                let Some(s) = t.fx.iter().find(|s| s.key == e.id) else {
                    v["missing"] = json!(true);
                    return v;
                };
                if let Some(why) = &s.crashed {
                    v["stopped"] = json!(why);
                }
                if s.chains() > 0 {
                    v["chains"] = json!(s.chains());
                }
                // A switching container: the one chain heard, from 0.
                if let Some(heard) = s.builtin().and_then(|b| b.heard()) {
                    v["heard"] = json!(heard);
                }
                if s.takes_key() {
                    v["sidechain"] = json!(e.sidechain);
                }
                let extra = s.extra_outputs();
                if !extra.is_empty() {
                    v["outputs"] = (extra.iter())
                        .map(|(i, name)| json!({"output": i, "name": name, "heard": e.outputs.contains(i)}))
                        .collect();
                }
                if s.uses() != daw_effects::Uses::Nothing {
                    v["file"] = json!(e.response);
                }
                if let Some((presets, programs)) = s.preset_choices(&dir) {
                    v["presets"] = (presets.iter())
                        .map(|p| json!({"name": p.name, "own": p.own}))
                        .collect();
                    v["programs"] = (programs.iter())
                        .filter(|l| !l.programs.is_empty())
                        .map(|l| json!({"list": l.name, "programs": l.programs}))
                        .collect();
                }
                v
            })
            .collect();
        let rooms: Vec<Value> = (daw_effects::library::ROOMS.iter())
            .map(|r| json!({"file": daw_effects::library::reference(r), "name": r.name}))
            .collect();
        Ok(json!({"track": key, "layers": layers, "effects": effects, "rooms": rooms}))
    }

    fn plugin_preset_tool(&mut self, args: &Value) -> Done {
        let key = text(args, "track", "the track's id")?;
        let id = text(args, "effect", "the effect's id")?;
        let ti = self
            .track_index(key)
            .ok_or_else(|| format!("There is no track {key}."))?;
        let dir = self.effects.presets_dir.clone();
        let si = (self.tracks[ti].fx.iter())
            .position(|s| s.key == id)
            .ok_or_else(|| format!("The track has no effect {id}."))?;
        let (presets, programs) = self.tracks[ti].fx[si]
            .preset_choices(&dir)
            .ok_or("Only a plugin has presets.")?;
        let find = |name: &str, own: bool| {
            (presets.iter())
                .find(|p| p.name.eq_ignore_ascii_case(name) && (p.own || !own))
                .cloned()
                .ok_or_else(|| format!("There is no preset {name}; get_effects lists them."))
        };
        if let Some(name) = args["save"].as_str() {
            let p = self.tracks[ti].fx[si].save_preset(&dir, name)?;
            return Ok((json!({"saved": p.name}), None));
        }
        if let Some(name) = args["delete"].as_str() {
            let p = find(name, true)?;
            self.tracks[ti].fx[si].delete_preset(&p)?;
            return Ok((json!({"deleted": p.name}), None));
        }
        // What was playing goes in the document first, so Undo brings it
        // back.
        let name = self.tracks[ti].fx[si].name.clone();
        let label = if let Some(name) = args["load"].as_str() {
            let p = find(name, false)?;
            self.write_plugin_states();
            self.tracks[ti].fx[si].load_preset(&p)?;
            format!("Load preset {}", p.name)
        } else if let Some(prog) = args["program"].as_str() {
            let want = args["program_list"].as_str();
            let (list, i) = (programs.iter())
                .filter(|l| want.is_none_or(|w| l.name.eq_ignore_ascii_case(w)))
                .find_map(|l| {
                    let i = l
                        .programs
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(prog))?;
                    Some((l.clone(), i))
                })
                .ok_or_else(|| format!("{name} has no program {prog}; get_effects lists them."))?;
            self.write_plugin_states();
            self.tracks[ti].fx[si].choose_program(&list, i)?;
            format!("Pick program {prog}")
        } else {
            return Err("Give load, save, delete or program.".into());
        };
        self.write_plugin_step(&format!("Assistant: {label} on {name}"));
        Ok((json!({"effect": id, "done": label}), None))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;
    use crate::freeze::test_plugin;

    fn id(v: Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    fn effects(d: &mut Driver, t: &str) -> Vec<Value> {
        let v = d.call("get_effects", json!({"track": t})).unwrap();
        v["effects"].as_array().unwrap().clone()
    }

    fn effect(d: &mut Driver, t: &str, e: &str) -> Value {
        (effects(d, t).into_iter()).find(|x| x["id"] == e).unwrap()
    }

    fn add(d: &mut Driver, args: Value) -> String {
        id(d.call("add_effect", args).unwrap(), "effect")
    }

    #[test]
    fn effects_go_in_container_chains_listen_to_a_sidechain_and_use_files() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let kick = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let par = add(&mut d, json!({"track": t, "effect": "parallel"}));
        assert!(effect(&mut d, &t, &par)["chains"].as_u64().unwrap() >= 2);
        let eq = add(
            &mut d,
            json!({"track": t, "effect": "eq", "container": par, "branch": 1}),
        );
        let e = effect(&mut d, &t, &eq);
        assert_eq!(
            (e["container"].clone(), e["branch"].clone()),
            (json!(par), json!(1))
        );
        // The engine has it in that chain.
        let app = d.app();
        let ti = app.track_index(&t).unwrap();
        assert_eq!(
            app.tracks[ti]
                .fx
                .iter()
                .find(|s| s.key == eq)
                .unwrap()
                .place,
            Some((par.clone(), 1))
        );
        assert!(
            d.call(
                "add_effect",
                json!({"track": t, "effect": "eq", "container": par, "branch": 99})
            )
            .is_err()
        );
        assert!(
            d.call(
                "add_effect",
                json!({"track": t, "effect": "eq", "container": eq})
            )
            .is_err(),
            "an EQ isn't a container"
        );
        // A container can't go inside one inside it.
        let inner = add(
            &mut d,
            json!({"track": t, "effect": "parallel", "container": par}),
        );
        assert!(
            d.call(
                "edit_effect",
                json!({"track": t, "effect": par, "container": inner})
            )
            .is_err()
        );
        d.call(
            "edit_effect",
            json!({"track": t, "effect": eq, "container": "none"}),
        )
        .unwrap();
        assert_eq!(effect(&mut d, &t, &eq)["container"], json!(null));

        // A compressor listens to the kick.
        let comp = add(&mut d, json!({"track": t, "effect": "compressor"}));
        d.call(
            "edit_effect",
            json!({"track": t, "effect": comp, "sidechain": kick}),
        )
        .unwrap();
        assert_eq!(effect(&mut d, &t, &comp)["sidechain"], kick);
        assert!(effect(&mut d, &t, &eq).get("sidechain").is_none());
        for bad in [
            json!({"track": t, "effect": eq, "sidechain": kick}),
            json!({"track": t, "effect": comp, "sidechain": t}),
            json!({"track": t, "effect": comp, "sidechain": "nope"}),
        ] {
            assert!(d.call("edit_effect", bad.clone()).is_err(), "{bad}");
        }
        d.command("undo");
        assert_eq!(effect(&mut d, &t, &comp)["sidechain"], json!(null));

        // Files: a recorded room, a file that isn't there, no capture.
        let verb = add(&mut d, json!({"track": t, "effect": "convolution"}));
        let rooms = d.call("get_effects", json!({"track": t})).unwrap()["rooms"].clone();
        if let Some(room) = rooms[0]["file"].as_str() {
            d.call(
                "edit_effect",
                json!({"track": t, "effect": verb, "file": room}),
            )
            .unwrap();
            assert_eq!(effect(&mut d, &t, &verb)["file"], room);
        }
        assert!(
            d.call(
                "edit_effect",
                json!({"track": t, "effect": verb, "file": "/no/such/room.wav"})
            )
            .is_err()
        );
        assert!(
            d.call("edit_effect", json!({"track": t, "effect": eq, "file": ""}))
                .is_err(),
            "an EQ uses no file"
        );
        let cap = add(&mut d, json!({"track": t, "effect": "amp-capture"}));
        assert!(
            d.call(
                "edit_effect",
                json!({"track": t, "effect": cap, "file": "/no/such/amp.nam"})
            )
            .is_err()
        );
        d.call(
            "edit_effect",
            json!({"track": t, "effect": cap, "file": ""}),
        )
        .unwrap();
        assert_eq!(effect(&mut d, &t, &cap)["file"], "");
    }

    #[test]
    fn settings_are_narrowed_to_an_effect_searched_and_paged() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let eq = add(&mut d, json!({"track": t, "effect": "eq"}));
        add(&mut d, json!({"track": t, "effect": "compressor"}));
        let only =
            |d: &mut Driver, args: Value| d.call("list_parameters", args).unwrap()[0].clone();
        let all = only(&mut d, json!({"track": t, "effect": eq}));
        let n = all["total"].as_u64().unwrap() as usize;
        assert_eq!(n, daw_effects::Kind::Eq.params().len(), "{all}");
        assert!(
            (all["params"].as_array().unwrap().iter())
                .all(|p| p["param"].as_str().unwrap().contains(&eq))
        );
        let page = only(
            &mut d,
            json!({"track": t, "effect": eq, "offset": 1, "limit": 2}),
        );
        assert_eq!(page["params"].as_array().unwrap().len(), 2.min(n - 1));
        assert_eq!(page["params"][0], all["params"][1]);
        let found = only(&mut d, json!({"track": t, "search": "VOLUME"}));
        assert!(found["total"].as_u64().unwrap() >= 1, "{found}");
    }

    /// The lo-fi effects (a worn record, a worn tape, a bit crusher) are
    /// added by id, and every one of their settings is listed and set by
    /// value.
    #[test]
    fn lofi_effects_are_reached_by_value() {
        let mut d = Driver::new();
        let t = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        for kind in [
            daw_effects::Kind::Vinyl,
            daw_effects::Kind::Tape,
            daw_effects::Kind::BitCrusher,
        ] {
            let kinds = d.call("list_effect_kinds", json!({})).unwrap();
            assert!(kinds.to_string().contains(kind.id()), "{kinds}");
            let e = add(&mut d, json!({"track": t, "effect": kind.id()}));
            let list = d
                .call("list_parameters", json!({"track": t, "effect": e}))
                .unwrap();
            let params = list[0]["params"].as_array().unwrap().clone();
            assert_eq!(params.len(), kind.params().len(), "{list}");
            for (p, want) in params.iter().zip(kind.params()) {
                let name = p["param"].as_str().unwrap();
                assert!(name.ends_with(want.key), "{name} {}", want.key);
                d.call(
                    "set_parameter",
                    json!({"track": t, "param": name, "value": want.max}),
                )
                .unwrap();
            }
            let list = d
                .call("list_parameters", json!({"track": t, "effect": e}))
                .unwrap();
            for (p, want) in list[0]["params"]
                .as_array()
                .unwrap()
                .iter()
                .zip(kind.params())
            {
                assert_eq!(p["value"].as_f64().unwrap(), want.max, "{p}");
            }
        }
    }

    /// A driver that has the test plugins, with presets kept in `name`'s
    /// own folder.
    fn with_plugins(name: &str) -> Driver {
        let mut d = Driver::new();
        let app = d.app();
        app.effects.helper = Some(test_plugin::helper());
        let bundle = test_plugin::bundle();
        app.effects.list.bundles = vec![daw_plugins::scan::BundleEntry {
            result: Ok(daw_plugins::scan::scan_here(&bundle).unwrap()),
            path: bundle,
            modified: 0,
        }];
        let dir =
            std::env::temp_dir().join(format!("daw-agent-routing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        app.effects.presets_dir = dir;
        d
    }

    fn gain(d: &mut Driver, t: &str, e: &str) -> (String, f64) {
        let v = d
            .call("list_parameters", json!({"track": t, "effect": e}))
            .unwrap();
        let p = &v[0]["params"][0];
        (
            p["param"].as_str().unwrap().into(),
            p["value"].as_f64().unwrap(),
        )
    }

    #[test]
    fn a_plugin_hears_its_extra_outputs_a_sidechain_and_its_presets() {
        let mut d = with_plugins("presets");
        let t = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let side = id(
            d.call("add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let router = add(
            &mut d,
            json!({"track": t, "effect": daw_test_plugin::ROUTER_NAME}),
        );
        let r = effect(&mut d, &t, &router);
        let out = r["outputs"][0]["output"].as_u64().expect("an extra output");
        assert_eq!(r["outputs"][0]["heard"], false);
        assert_eq!(r["sidechain"], json!(null), "it can listen to a track");
        d.call(
            "edit_effect",
            json!({"track": t, "effect": router, "outputs": [out], "sidechain": side}),
        )
        .unwrap();
        let r = effect(&mut d, &t, &router);
        assert_eq!(
            (r["outputs"][0]["heard"].clone(), r["sidechain"].clone()),
            (json!(true), json!(side))
        );
        let app = d.app();
        let ti = app.track_index(&t).unwrap();
        let s = app.tracks[ti].fx.iter().find(|s| s.key == router).unwrap();
        assert_eq!(s.outputs, [out as u32]);
        assert!(
            d.call(
                "edit_effect",
                json!({"track": t, "effect": router, "outputs": [42]})
            )
            .is_err()
        );

        // Presets: save the settings, change them, load them back.
        let g = add(&mut d, json!({"track": t, "effect": daw_test_plugin::NAME}));
        let (param, start) = gain(&mut d, &t, &g);
        d.call(
            "plugin_preset",
            json!({"track": t, "effect": g, "save": "Start"}),
        )
        .unwrap();
        let presets = effect(&mut d, &t, &g)["presets"].clone();
        assert_eq!(presets, json!([{"name": "Start", "own": true}]));
        let moved = if start > 0.5 { 0.1 } else { 0.9 };
        d.call(
            "set_parameter",
            json!({"track": t, "param": param, "value": moved}),
        )
        .unwrap();
        assert!((gain(&mut d, &t, &g).1 - moved).abs() < 1e-3);
        d.call(
            "plugin_preset",
            json!({"track": t, "effect": g, "load": "start"}),
        )
        .unwrap();
        assert!((gain(&mut d, &t, &g).1 - start).abs() < 1e-3);
        // Loading was a step: undo brings back the setting it replaced.
        d.command("undo");
        assert!((gain(&mut d, &t, &g).1 - moved).abs() < 1e-3);
        assert!(
            d.call(
                "plugin_preset",
                json!({"track": t, "effect": g, "program": "Anything"})
            )
            .is_err()
        );
        d.call(
            "plugin_preset",
            json!({"track": t, "effect": g, "delete": "Start"}),
        )
        .unwrap();
        assert_eq!(effect(&mut d, &t, &g)["presets"], json!([]));
        assert!(
            d.call(
                "plugin_preset",
                json!({"track": t, "effect": router, "load": "Start"})
            )
            .is_err()
        );
    }

    #[test]
    fn a_plugin_instrument_can_be_chosen_for_a_track_and_a_layer() {
        let mut d = with_plugins("instrument");
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        d.call(
            "set_instrument",
            json!({"track": t, "instrument": daw_test_plugin::SYNTH_NAME}),
        )
        .unwrap();
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["instrument"]["kind"], "plugin", "{i}");
        assert_eq!(i["instrument"]["name"], daw_test_plugin::SYNTH_NAME);
        let l = id(
            d.call("edit_layer", json!({"track": t, "add": true}))
                .unwrap(),
            "layer",
        );
        d.call(
            "set_instrument",
            json!({"track": t, "layer": l, "instrument": daw_test_plugin::SYNTH_NAME}),
        )
        .unwrap();
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["layers"][0]["instrument"]["kind"], "plugin");
        assert_eq!(i["layers"][1]["instrument"]["kind"], "plugin");
    }

    #[test]
    fn a_stopped_plugin_is_started_again() {
        let mut d = with_plugins("restart");
        let t = id(
            d.call("add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        );
        let g = add(&mut d, json!({"track": t, "effect": daw_test_plugin::NAME}));
        d.call(
            "set_instrument",
            json!({"track": t, "instrument": daw_test_plugin::SYNTH_NAME}),
        )
        .unwrap();
        // Running: nothing to restart.
        assert_eq!(effect(&mut d, &t, &g)["stopped"], json!(null));
        assert!(
            d.call(
                "edit_effect",
                json!({"track": t, "effect": g, "restart": true})
            )
            .is_err()
        );
        // As a crashed helper leaves them.
        let app = d.app();
        let ti = app.track_index(&t).unwrap();
        let tr = &mut app.tracks[ti];
        tr.fx.iter_mut().find(|s| s.key == g).unwrap().crashed = Some("It crashed".into());
        let inst = tr.instrument.as_mut().and_then(|i| i.plugin_mut()).unwrap();
        inst.crashed = Some("It crashed".into());

        assert_eq!(effect(&mut d, &t, &g)["stopped"], "It crashed");
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["stopped"], "It crashed", "{i}");
        let r = d
            .call(
                "edit_effect",
                json!({"track": t, "effect": g, "restart": true}),
            )
            .unwrap();
        assert_eq!(r["restarted"], true);
        assert_eq!(effect(&mut d, &t, &g)["stopped"], json!(null));
        d.call("edit_instrument", json!({"track": t, "restart": true}))
            .unwrap();
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["stopped"], json!(null), "{i}");
    }
}
