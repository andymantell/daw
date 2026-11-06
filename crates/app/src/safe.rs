// SPDX-License-Identifier: GPL-3.0-or-later
//! Keeping a project safe to open and light to play (phase 4, Set C):
//!
//! - **Plugins off after a crash.** When the app didn't close properly
//!   last time (see [`crate::crash::Running`]), plugins saved in the
//!   projects it opens are kept as they are but not started, so a plugin
//!   that crashed it can't stop the project opening again. Each can be
//!   turned back on from its effects window, or all at once from the note
//!   along the bottom or the `plugins-on` command.
//! - **A track turned off** stays in the project with everything on it,
//!   but plays nothing: its clips aren't read, and its instrument and
//!   effects stay loaded but do no processing.
//! - **A solo-safe track** stays heard when others are soloed, as a
//!   reverb bus or a click track should.

use daw_engine::engine::Command;
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::{APP_NAME, App};

/// A plugin switched off after a crash.
pub(crate) struct Held {
    /// The track it is on, by key ("master" for the master's).
    pub(crate) track: String,
    /// Its effect id in the project.
    pub(crate) id: String,
    pub(crate) name: String,
}

impl App {
    /// The track a track command acts on: the one given to an
    /// assistant's command or whose instrument is open, the one in the
    /// inspector, the one whose effects are open, the first chosen in
    /// the mixer, or the one with a chosen clip.
    pub(crate) fn acted_track(&self) -> Option<usize> {
        let ok = |i: &usize| *i < self.tracks.len();
        let selected = (self.tracks.iter()).position(|t| {
            t.clips
                .iter()
                .any(|c| self.timeline.selected.contains(&c.id))
        });
        self.instrument_open
            .filter(ok)
            .or_else(|| (self.inspector.track.as_deref()).and_then(|k| self.track_index(k)))
            .or(self.effects.open_track.filter(ok))
            .or_else(|| (self.desk.selected.first()).and_then(|k| self.track_index(k)))
            .or(selected)
    }

    /// Turns track `i` off, or on again. Turning it off disarms it and
    /// stops it monitoring. The undo step's name.
    pub(crate) fn set_track_off(&mut self, i: usize, off: bool) -> Option<String> {
        let t = self.tracks.get_mut(i)?;
        t.off = off;
        if off {
            t.arm = false;
            t.monitor = false;
        }
        let label = format!("Turn {} {}", t.name, if off { "off" } else { "on" });
        self.rebuild();
        Some(label)
    }

    /// Makes track `i` solo-safe, or not. The undo step's name.
    pub(crate) fn set_solo_safe(&mut self, i: usize, on: bool) -> Option<String> {
        let t = self.tracks.get_mut(i)?;
        t.mix.solo_safe = on;
        let mix = t.heard();
        let label = format!(
            "{} {} solo-safe",
            if on { "Make" } else { "Stop making" },
            t.name
        );
        self.send(Command::SetMix { track: i, mix });
        Some(label)
    }

    /// The plugins kept switched off after a crash, in the order of the
    /// tracks: instruments, layers and effects.
    pub(crate) fn plugins_held(&self) -> Vec<Held> {
        let fx = &self.effects;
        if !fx.plugins_off {
            return Vec::new();
        }
        let mut out = Vec::new();
        let tracks = (self.tracks.iter()).chain(std::iter::once(&self.master));
        for t in tracks {
            let slots = (t.instrument.iter()).chain(t.layers.iter().map(|l| &l.slot));
            for s in slots.filter(|s| s.is_missing() && fx.holds(&s.key)) {
                out.push(Held {
                    track: t.key.clone(),
                    id: s.key.clone(),
                    name: s.name.clone(),
                });
            }
            for (_, e) in (self.proj.missing_fx.get(&t.key).into_iter().flatten())
                .filter(|(_, e)| e.is_plugin() && fx.holds(&e.id))
            {
                out.push(Held {
                    track: t.key.clone(),
                    id: e.id.clone(),
                    name: e.name.clone(),
                });
            }
        }
        out
    }

    /// Starts the plugins switched off after a crash: the one with
    /// effect id `only`, or all of them. What was done, or why not.
    pub(crate) fn turn_plugins_on(&mut self, only: Option<&str>) -> Result<String, String> {
        if !self.effects.plugins_off {
            return Err("No plugins are switched off.".into());
        }
        let held = self.plugins_held();
        let label = match only {
            Some(id) => {
                let h = (held.iter().find(|h| h.id == id))
                    .ok_or_else(|| format!("No plugin {id} is switched off."))?;
                self.effects.turned_on.insert(id.to_string());
                format!("Turned on {}", h.name)
            }
            None => {
                self.effects.plugins_off = false;
                self.effects.turned_on.clear();
                match held.len() {
                    1 => "Turned on 1 plugin".to_string(),
                    n => format!("Turned on {n} plugins"),
                }
            }
        };
        // Instruments kept as saved are made afresh; effects kept as
        // saved are made again as the project is applied.
        let p = self.project();
        let fx = &self.effects;
        for t in &mut self.tracks {
            if (t.instrument.as_ref()).is_some_and(|s| s.is_missing() && !fx.holds(&s.key)) {
                t.instrument = None;
            }
            t.layers
                .retain(|l| !(l.slot.is_missing() && !fx.holds(&l.slot.key)));
        }
        self.apply(p);
        if self.plugins_held().is_empty() {
            self.effects.plugins_off = false;
            self.effects.turned_on.clear();
        }
        Ok(label)
    }

    /// What assistants are told of plugins switched off after a crash.
    pub(crate) fn plugins_off_value(&self) -> Value {
        json!({
            "after_crash": self.effects.plugins_off,
            "held": self.plugins_held().iter().map(|h| json!({
                "track": h.track, "effect": h.id, "name": h.name,
            })).collect::<Vec<_>>(),
        })
    }

    /// The note along the bottom while plugins are switched off.
    pub(crate) fn plugins_off_note(&mut self, ui: &mut egui::Ui) {
        let held = self.plugins_held().len();
        ui.horizontal_wrapped(|ui| {
            let what = match held {
                0 => "plugins in the projects you open start switched off".to_string(),
                1 => "1 plugin in this project is switched off".to_string(),
                n => format!("{n} plugins in this project are switched off"),
            };
            ui.label(format!(
                "{APP_NAME} didn't close properly last time, so {what}. Their settings are \
                 kept; turn each on from its effects window, or all of them here."
            ));
            if ui
                // ai: plugins-on
                .button("Turn plugins back on")
                .clicked()
                && let Err(e) = self.turn_plugins_on(None)
            {
                self.message = Some(e);
            }
            // ai: view
            if ui.button("Keep them off").clicked() {
                self.effects.off_note_hidden = true;
            }
        });
        if held > 0 {
            ui.label(
                RichText::new(
                    (self.plugins_held().iter())
                        .map(|h| h.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .weak(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    /// The track ids get_project lists, in order.
    fn ids(d: &mut Driver) -> Vec<String> {
        (d.project()["tracks"].as_array().unwrap().iter())
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn an_assistant_turns_a_track_off_and_on_and_makes_a_bus_solo_safe() {
        let mut d = Driver::new();
        d.command("add-synth-track");
        d.command("add-bus");
        let ids = ids(&mut d);
        let (synth, bus) = (ids[0].clone(), ids[1].clone());

        // Off: kept with its instrument, but the engine runs nothing on it.
        d.call(
            "set_parameter",
            json!({"track": synth, "param": "off", "value": 1}),
        )
        .unwrap();
        assert_eq!(d.project()["tracks"][0]["off"], true);
        let app = d.app();
        assert!(app.tracks[0].instrument.is_some(), "its instrument stays");
        let s = app.session();
        assert!(s.tracks[0].off && s.tracks[0].clips.is_empty());
        assert!(
            s.tracks[0].instrument.is_some(),
            "kept in the graph, not run"
        );
        // The undo step says what it was, and undoing turns it back on.
        d.command("undo");
        assert_eq!(d.project()["tracks"][0]["off"], false);
        // The command turns it off, then on.
        d.call("run_command", json!({"id": "track-off", "track": synth}))
            .unwrap();
        assert_eq!(d.project()["tracks"][0]["off"], true);
        assert!(d.app().tracks[0].off);
        d.call("run_command", json!({"id": "track-off", "track": synth}))
            .unwrap();
        assert_eq!(d.project()["tracks"][0]["off"], false);
        assert!(!d.app().session().tracks[0].off);

        // Solo-safe: the bus stays heard when the synth is soloed.
        d.call("run_command", json!({"id": "solo-safe", "track": bus}))
            .unwrap();
        assert_eq!(d.project()["tracks"][1]["solo_safe"], true);
        d.call(
            "set_parameter",
            json!({"track": synth, "param": "solo", "value": 1}),
        )
        .unwrap();
        let s = d.app().session();
        assert!(s.tracks[1].mix.solo_safe && s.tracks[0].mix.solo);
        let params = d
            .call("list_parameters", json!({"track": bus, "search": "solo"}))
            .unwrap();
        let safe = (params[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == "solo_safe")
            .cloned()
            .unwrap();
        assert_eq!(safe["value"], 1);
        d.call(
            "set_parameter",
            json!({"track": bus, "param": "solo_safe", "value": 0}),
        )
        .unwrap();
        assert_eq!(d.project()["tracks"][1]["solo_safe"], false);
    }

    #[test]
    fn after_a_crash_plugins_start_switched_off_until_an_assistant_turns_them_on() {
        let mut d = Driver::new();
        // As the app starts after a run that didn't close properly.
        d.app().effects.plugins_off = true;
        let mut p = daw_project::Project {
            rate: d.app().rate,
            ..Default::default()
        };
        let mut t = daw_project::Track::new("g".into(), "Guitar".into());
        t.effects = vec![
            daw_project::Effect {
                id: "plug".into(),
                name: "Some Plugin".into(),
                class: vec![7; 16],
                bundle: "/nowhere/Some Plugin.vst3".into(),
                ..Default::default()
            },
            daw_project::Effect {
                id: "eq".into(),
                name: "EQ".into(),
                builtin: "eq".into(),
                ..Default::default()
            },
        ];
        p.tracks.push(t);
        d.app().new_project_from(p);
        d.h.run();

        // The plugin is kept, not started; the built-in effect runs.
        let off = d.call("get_plugins", json!({})).unwrap()["plugins_off"].clone();
        assert_eq!(off["after_crash"], true);
        assert_eq!(off["held"][0]["effect"], "plug");
        assert_eq!(off["held"][0]["track"], "g");
        let proj = d.project();
        assert_eq!(proj["plugins_off"]["held"].as_array().unwrap().len(), 1);
        assert_eq!(proj["tracks"][0]["effects"].as_array().unwrap().len(), 2);
        assert_eq!(d.app().tracks[0].fx.len(), 1, "only the EQ is made");
        assert!(d.app().message.is_none(), "the note says it, not an error");

        // Turned on, it is tried again: not installed here, it is kept
        // as missing, and nothing is held any more.
        let done = d
            .call("turn_plugins_on", json!({"effect": "plug"}))
            .unwrap();
        assert_eq!(done["done"], "Turned on Some Plugin");
        assert_eq!(done["plugins_off"]["held"].as_array().unwrap().len(), 0);
        assert!(!d.app().effects.plugins_off);
        assert!(d.app().proj.missing_fx.contains_key("g"), "kept as saved");
        assert_eq!(
            d.project()["tracks"][0]["effects"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let again = d.call("turn_plugins_on", json!({})).unwrap_err();
        assert!(again.contains("No plugins"), "{again}");
    }

    #[test]
    fn the_command_turns_every_plugin_back_on() {
        let mut d = Driver::new();
        d.app().effects.plugins_off = true;
        let mut p = daw_project::Project {
            rate: d.app().rate,
            ..Default::default()
        };
        let mut t = daw_project::Track::new("k".into(), "Keys".into());
        t.kind = daw_project::TrackKind::Instrument;
        t.instrument = Some(daw_project::Effect {
            id: "inst".into(),
            name: "A Plugin Synth".into(),
            class: vec![3; 16],
            bundle: "/nowhere/Synth.vst3".into(),
            ..Default::default()
        });
        p.tracks.push(t);
        d.app().new_project_from(p);
        d.h.run();
        let held = d.app().plugins_held();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].name, "A Plugin Synth");
        d.command("plugins-on");
        assert!(!d.app().effects.plugins_off);
        assert!(d.app().plugins_held().is_empty());
        let inst = &d.project()["tracks"][0]["instrument"];
        assert_eq!(inst["name"], "A Plugin Synth", "kept as saved");
    }
}
