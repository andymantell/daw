// SPDX-License-Identifier: GPL-3.0-or-later
//! Switching containers (Phase 4, B4): an effect container whose chains
//! are heard one at a time, such as a clean and a driven amp channel.
//! The chain heard is one of the container's settings ("heard"), so it
//! can be automated, tied to a MIDI control (a footswitch steps through
//! the chains with each press: see `MidiMap::step`) and set by an
//! assistant like any other; the commands here switch it from a key.
//!
//! The commands act on one track: the one an assistant names (lent as
//! the open instrument window), the one whose effects window is open, the
//! one with a selected clip, an armed or monitoring one, or else the first
//! with a switching container, in that order, skipping any without one.
//! Every switching container on that track switches together.

use daw_effects::Kind;

use crate::{App, FxSlot, Track};

/// The switching containers on a track.
fn switches(t: &Track) -> impl Iterator<Item = &daw_effects::Effect> {
    (t.fx.iter())
        .filter_map(FxSlot::builtin)
        .filter(|b| b.kind() == Kind::Switch)
}

/// Whether `param` on the track `track` (by key) is the chain a
/// switching container plays.
pub(crate) fn is_heard(tracks: &[Track], track: &str, param: &str) -> bool {
    let Some(t) = tracks.iter().find(|t| t.key == track) else {
        return false;
    };
    match daw_project::Lane::new(param).target() {
        daw_project::LaneTarget::Fx {
            effect,
            key: "heard",
        } => (t.fx.iter())
            .find(|s| s.key == effect)
            .and_then(FxSlot::builtin)
            .is_some_and(|b| b.kind() == Kind::Switch),
        _ => false,
    }
}

impl App {
    /// The track the switching commands act on, if any has a switching
    /// container.
    pub(crate) fn switch_track(&self) -> Option<usize> {
        let has = |i: &usize| {
            self.tracks
                .get(*i)
                .is_some_and(|t| switches(t).next().is_some())
        };
        let selected = (self.tracks.iter()).position(|t| {
            t.clips
                .iter()
                .any(|c| self.timeline.selected.contains(&c.id))
                || (self.timeline.note_sel.as_ref())
                    .is_some_and(|k| t.note_clips.iter().any(|n| &n.id == k))
        });
        let live = (self.tracks.iter().enumerate())
            .filter(|(_, t)| t.arm || t.monitor)
            .map(|(i, _)| i);
        [self.instrument_open, self.effects.open_track, selected]
            .into_iter()
            .flatten()
            .chain(live)
            .chain(0..self.tracks.len())
            .find(has)
    }

    /// How many chains the track's switching containers have (the most,
    /// if it has several).
    pub(crate) fn switch_chains(&self, track: usize) -> usize {
        (self.tracks.get(track).into_iter())
            .flat_map(switches)
            .map(|b| b.chains())
            .max()
            .unwrap_or(0)
    }

    /// Hears each switching container's next chain, going round.
    pub(crate) fn next_chain(&mut self) {
        let Some(i) = self.switch_track() else {
            return;
        };
        let mut heard = Vec::new();
        for b in switches(&self.tracks[i]) {
            let now = b.heard().unwrap_or(0);
            b.set_heard((now + 1) % b.chains().max(1));
            heard.push(b.heard().unwrap_or(0) + 1);
        }
        self.say_heard(i, &heard);
    }

    /// Hears chain `n` (from 0) of each switching container that has it.
    pub(crate) fn hear_chain(&mut self, n: usize) {
        let Some(i) = self.switch_track() else {
            return;
        };
        let mut heard = Vec::new();
        for b in switches(&self.tracks[i]).filter(|b| n < b.chains()) {
            b.set_heard(n);
            heard.push(n + 1);
        }
        self.say_heard(i, &heard);
    }

    fn say_heard(&mut self, track: usize, heard: &[usize]) {
        let name = &self.tracks[track].name;
        self.message = match heard {
            [] => None,
            [n] => Some(format!("{name}: chain {n}.")),
            _ => Some(format!(
                "{name}: chains {}.",
                (heard.iter().map(|n| n.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;

    fn id(v: Value, key: &str) -> String {
        v[key].as_str().unwrap().to_string()
    }

    /// A track playing two seconds of a steady tone, brought in from a
    /// file.
    fn tone_track(d: &mut Driver, name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("daw-switch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let rate = d.app().rate;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..2 * rate {
            let x = (2.0 * std::f64::consts::PI * 220.0 * i as f64 / rate as f64).sin();
            w.write_sample((x * 8000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let before = d.project()["tracks"].as_array().unwrap().len();
        d.call("import_audio", json!({"path": path})).unwrap();
        for _ in 0..500 {
            let p = d.project();
            let tracks = p["tracks"].as_array().unwrap();
            if tracks.len() > before {
                return tracks[before]["id"].as_str().unwrap().to_string();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the file didn't come in");
    }

    fn switch_of(d: &mut Driver, t: &str) -> Value {
        let v = d.call("get_effects", json!({"track": t})).unwrap();
        (v["effects"].as_array().unwrap().iter())
            .find(|e| e["builtin"] == "switch")
            .cloned()
            .unwrap()
    }

    fn heard(d: &mut Driver, t: &str) -> u64 {
        switch_of(d, t)["heard"].as_u64().unwrap()
    }

    fn rms(d: &mut Driver, t: &str, from: f64, to: f64) -> f64 {
        d.call(
            "analyse",
            json!({"track": t, "from_seconds": from, "to_seconds": to}),
        )
        .unwrap()["rms_dbfs"]
            .as_f64()
            .unwrap()
    }

    fn press(d: &mut Driver, value: u8) {
        d.call(
            "edit_midi_maps",
            json!({"move": {"channel": 1, "cc": 64, "value": value}}),
        )
        .unwrap();
    }

    #[test]
    fn a_clean_and_a_driven_chain_switch_by_key_footswitch_and_automation() {
        let mut d = Driver::new();
        // A bass with no switch, then the guitar with one.
        d.call("add_track", json!({"kind": "audio"})).unwrap();
        let t = tone_track(&mut d, "keys");
        let e = d
            .call("run_command", json!({"id": "next-chain"}))
            .unwrap_err();
        assert!(e.contains("can't run now"), "{e}");
        let sw = id(
            d.call("add_effect", json!({"track": t, "effect": "switch"}))
                .unwrap(),
            "effect",
        );
        let s = switch_of(&mut d, &t);
        assert_eq!(
            (s["chains"].clone(), s["heard"].clone()),
            (json!(2), json!(0))
        );
        // The same amp in each chain, the second chain 20 dB down, so
        // which is heard shows in the level.
        for branch in 0..2 {
            d.call(
                "add_effect",
                json!({"track": t, "effect": "amp", "container": sw, "branch": branch}),
            )
            .unwrap();
        }
        let level2 = format!("fx:{sw}:level2");
        d.call(
            "set_parameter",
            json!({"track": t, "param": level2, "value": -20.0}),
        )
        .unwrap();
        let first = rms(&mut d, &t, 0.2, 1.8);

        // Keys: the commands find the track with the switch.
        d.command("next-chain");
        assert_eq!(heard(&mut d, &t), 1);
        let second = rms(&mut d, &t, 0.2, 1.8);
        assert!((first - second - 20.0).abs() < 0.5, "{first} {second}");
        d.command("next-chain");
        assert_eq!(heard(&mut d, &t), 0, "round to the first again");
        d.call("run_command", json!({"id": "hear-chain-2", "track": t}))
            .unwrap();
        assert_eq!(heard(&mut d, &t), 1);
        d.command("hear-chain-1");
        assert_eq!(heard(&mut d, &t), 0);
        assert!(
            d.call("run_command", json!({"id": "hear-chain-3"}))
                .is_err(),
            "there are only two chains"
        );
        // Each change is an undo step.
        d.call("undo_steps", json!({"undo": 1})).unwrap();
        assert_eq!(heard(&mut d, &t), 1);
        d.command("hear-chain-1");

        // A footswitch: each press (up past half way) steps on.
        let heard_param = format!("fx:{sw}:heard");
        let maps = d
            .call(
                "edit_midi_maps",
                json!({"add": {"channel": 1, "cc": 64, "track": t, "param": heard_param}}),
            )
            .unwrap();
        assert_eq!(maps["maps"][0]["step"], true, "it steps unless told not to");
        assert_eq!(
            maps["maps"][0]["name"],
            "Switch between chains: Chain heard"
        );
        press(&mut d, 127);
        assert_eq!(heard(&mut d, &t), 1);
        press(&mut d, 127);
        assert_eq!(heard(&mut d, &t), 1, "still held: not a new press");
        press(&mut d, 0);
        assert_eq!(heard(&mut d, &t), 1, "letting go changes nothing");
        press(&mut d, 127);
        assert_eq!(heard(&mut d, &t), 0);
        // Following the pedal instead: up is the last chain, down the first.
        let m = id(maps["maps"][0].clone(), "id");
        d.call(
            "edit_midi_maps",
            json!({"change": {"id": m, "step": false}}),
        )
        .unwrap();
        press(&mut d, 127);
        assert_eq!(heard(&mut d, &t), 1);
        press(&mut d, 0);
        assert_eq!(heard(&mut d, &t), 0);
        let got = d.call("get_midi_maps", json!({})).unwrap();
        assert_eq!(got["maps"][0]["step"], false);
        d.call("edit_midi_maps", json!({"remove": m})).unwrap();
        assert_eq!(
            d.call("get_midi_maps", json!({})).unwrap()["maps"],
            json!([])
        );

        // Automation: the first chain for the first second, then the
        // second.
        d.call(
            "edit_automation",
            json!({"track": t, "param": heard_param, "add": [
                {"seconds": 0.0, "value": 0.0},
                {"seconds": 0.99, "value": 0.0},
                {"seconds": 1.0, "value": 1.0},
            ]}),
        )
        .unwrap();
        d.call("set_automation_mode", json!({"track": t, "mode": "read"}))
            .unwrap();
        let before = rms(&mut d, &t, 0.2, 0.9);
        let after = rms(&mut d, &t, 1.2, 1.8);
        assert!((before - after - 20.0).abs() < 0.5, "{before} {after}");
        // Played from part way, the chain the lane gives is heard from
        // the start, with nothing of the other faded out.
        let short = rms(&mut d, &t, 1.2, 1.25);
        assert!((short - after).abs() < 0.5, "{short} {after}");
    }

    #[test]
    fn the_clean_and_driven_preset_holds_both_channels() {
        let mut d = Driver::new();
        let t = tone_track(&mut d, "preset");
        d.call(
            "add_preset",
            json!({"name": "Clean and driven guitar", "track": t}),
        )
        .unwrap();
        let v = d.call("get_effects", json!({"track": t})).unwrap();
        let fx = v["effects"].as_array().unwrap();
        let sw = (fx.iter()).find(|e| e["builtin"] == "switch").unwrap();
        let amps: Vec<&Value> = (fx.iter())
            .filter(|e| e["builtin"] == "amp" && e["container"] == sw["id"])
            .collect();
        assert_eq!(amps.len(), 2, "{fx:?}");
        assert_eq!(
            (amps[0]["branch"].clone(), amps[1]["branch"].clone()),
            (json!(0), json!(1))
        );
        assert_eq!(sw["heard"], 0);
        // Switching it is heard: the driven channel is louder.
        let clean = rms(&mut d, &t, 0.2, 1.8);
        d.command("next-chain");
        let driven = rms(&mut d, &t, 0.2, 1.8);
        assert!((clean - driven).abs() > 1.0, "{clean} {driven}");
    }
}
