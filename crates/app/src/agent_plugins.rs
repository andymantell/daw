// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for the plugin manager (roadmap 2.4, 2.11): the
//! plugins found on this computer, which are offered, where each runs,
//! scanning and checking them. These belong to the computer, not the
//! project, so there is no undo step.

use daw_plugins::host::Sandbox;
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_plugins",
        description: "The VST 3 and CLAP plugins found on this computer: each one's name, \
                      vendor, effect or instrument, format, whether it is offered when \
                      adding effects, and where it runs (app, shared helper process, own \
                      process); those that could not be loaded and why; and how a scan or \
                      check is going.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "manage_plugins",
        description: "Rescans for plugins, checks each one switched on (writes a report), \
                      tries a plugin that failed to load again, offers or hides plugins, \
                      sets where one runs, or sets how many plugin windows may be open \
                      at once (0 for no limit). Plugins are named as get_plugins names them.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "rescan": {"type": "boolean"},
                "check": {"type": "boolean"},
                "retry": {"type": "string", "description": "A failed bundle's path, as get_plugins gives it."},
                "offer": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}, "on": {"type": "boolean"}
                }, "required": ["name", "on"]}},
                "runs_in": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"},
                    "where": {"type": "string", "enum": ["app", "shared", "own"]}
                }, "required": ["name", "where"]}},
                "window_limit": {"type": "integer", "description": "0 to 32."}
            }})
        },
    },
    Tool {
        name: "turn_plugins_on",
        description: "When the app didn't close properly last time, plugins in the \
                      projects it opens start switched off (get_plugins and get_project \
                      list them under plugins_off), so one that crashed it can't stop the \
                      project opening. This starts them again with their settings: the \
                      one given, by its effect id, or all of them.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "effect": {"type": "string", "description": "One plugin's effect id, as plugins_off.held lists it; all of them if left out."}
            }})
        },
    },
];

impl App {
    pub(crate) fn plugins_read(&mut self, name: &str) -> Option<Result<Value, String>> {
        (name == "get_plugins").then(|| Ok(self.plugins_value()))
    }

    pub(crate) fn plugins_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        match name {
            "manage_plugins" => Some(self.manage_plugins(args)),
            "turn_plugins_on" => Some(self.turn_plugins_on(args["effect"].as_str()).map(|done| {
                (
                    json!({"done": done, "plugins_off": self.plugins_off_value()}),
                    None,
                )
            })),
            _ => None,
        }
    }

    fn plugins_value(&mut self) -> Value {
        let off = self.plugins_off_value();
        let fx = &mut self.effects;
        let checking = fx.poll_check();
        let plugins: Vec<Value> = (fx.list.plugins().iter())
            .map(|p| {
                json!({
                    "name": p.class.name, "vendor": p.class.vendor,
                    "kind": if p.class.is_instrument() { "instrument" } else { "effect" },
                    "format": p.format(), "offered": p.enabled, "runs_in": p.sandbox.key(),
                    "bundle": p.bundle.display().to_string(),
                })
            })
            .collect();
        let failed: Vec<Value> = (fx.list.failures())
            .map(|(p, e)| json!({"bundle": p.display().to_string(), "error": e}))
            .collect();
        json!({
            "plugins": plugins,
            "plugins_off": off,
            "could_not_load": failed,
            "scanning": fx.scanning(),
            "checking": checking,
            "check": fx.check_status.clone().map(|r| match r {
                Ok(s) => json!({"status": s}),
                Err(e) => json!({"error": e}),
            }),
            "check_report": fx.check_report_path().display().to_string(),
            "window_limit": fx.list.editors,
            "helper_process": fx.helper.is_some(),
        })
    }

    fn manage_plugins(&mut self, args: &Value) -> Done {
        let plugins = self.effects.list.plugins();
        let find = |name: &str| -> Result<_, String> {
            let mut hits = plugins.iter().filter(|p| p.class.name == name);
            match (hits.next(), hits.next()) {
                (Some(p), None) => Ok(p.class.cid),
                (Some(_), Some(_)) => Err(format!("More than one plugin is called {name}.")),
                (None, _) => Err(format!("There is no plugin {name}.")),
            }
        };
        // Check everything before changing anything.
        let mut offer = Vec::new();
        for o in args["offer"].as_array().into_iter().flatten() {
            let cid = find(o["name"].as_str().unwrap_or(""))?;
            offer.push((cid, o["on"].as_bool().unwrap_or(true)));
        }
        let mut runs = Vec::new();
        for r in args["runs_in"].as_array().into_iter().flatten() {
            let cid = find(r["name"].as_str().unwrap_or(""))?;
            let w = r["where"].as_str().unwrap_or("");
            let sb = Sandbox::from_key(w).ok_or("Where is app, shared or own.")?;
            runs.push((cid, sb));
        }
        let limit = match args["window_limit"].as_u64() {
            None => None,
            Some(n @ 0..=32) => Some(n as u32),
            Some(_) => return Err("The window limit goes from 0 to 32.".into()),
        };
        let retry = match args["retry"].as_str() {
            None => None,
            Some(b) => {
                let p = (self.effects.list.failures())
                    .map(|(p, _)| p.to_owned())
                    .find(|p| p.display().to_string() == b)
                    .ok_or_else(|| format!("No plugin failed to load from {b}."))?;
                Some(p)
            }
        };
        let fx = &mut self.effects;
        let changed = !offer.is_empty() || !runs.is_empty() || limit.is_some();
        for (cid, on) in offer {
            fx.list.set_enabled(&cid, on);
        }
        for (cid, sb) in runs {
            fx.list.set_sandbox(&cid, sb);
        }
        if let Some(n) = limit {
            fx.list.editors = n;
        }
        if changed && let Err(e) = fx.save() {
            return Err(format!("Could not save the plugin list: {e}"));
        }
        if let Some(p) = retry {
            fx.list.forget(&p);
            fx.start_scan();
        } else if args["rescan"].as_bool() == Some(true) {
            fx.start_scan();
        }
        if args["check"].as_bool() == Some(true) {
            if fx.helper.is_none() {
                return Err("There is no helper process here to check plugins in.".into());
            }
            if fx.scanning().is_some() {
                return Err("Wait for the scan to finish before checking plugins.".into());
            }
            if !fx.poll_check() {
                fx.start_check();
            }
        }
        Ok((self.plugins_value(), None))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    #[test]
    fn the_plugin_list_is_read_and_refuses_unknown_plugins() {
        let mut d = Driver::new();
        let v = d.call("get_plugins", json!({})).unwrap();
        assert!(v["plugins"].is_array());
        assert!(v["window_limit"].is_number());
        assert!(
            d.call(
                "manage_plugins",
                json!({"offer": [{"name": "Nope", "on": false}]})
            )
            .is_err()
        );
        assert!(
            d.call("manage_plugins", json!({"window_limit": 99}))
                .is_err()
        );
        assert!(
            d.call("manage_plugins", json!({"retry": "/no/such/bundle.vst3"}))
                .is_err()
        );
    }
}
