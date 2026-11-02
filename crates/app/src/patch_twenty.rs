// SPDX-License-Identifier: GPL-3.0-or-later
//! Patch Twenty, the semi-modular monosynth (classic synths plan, S7),
//! as an assistant uses it: its track command, its settings through
//! `list_parameters` and `set_parameters`, a sound voiced from them and
//! rendered, and its external input on the track.

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;

    /// The track's instrument settings from `list_parameters`.
    fn settings(d: &mut Driver, track: &str) -> Vec<Value> {
        let list = d.call("list_parameters", json!({"track": track})).unwrap();
        (list[0]["params"].as_array().unwrap().iter())
            .filter(|p| p["param"].as_str().unwrap().starts_with("instrument."))
            .cloned()
            .collect()
    }

    fn setting<'a>(list: &'a [Value], key: &str) -> &'a Value {
        (list.iter())
            .find(|p| p["param"] == format!("instrument.{key}"))
            .unwrap_or_else(|| panic!("no setting {key}"))
    }

    fn choice(list: &[Value], key: &str, name: &str) -> usize {
        let choices = setting(list, key)["choices"].as_array().unwrap();
        (choices.iter())
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("{key} has no choice {name}: {choices:?}"))
    }

    /// An assistant adds the synth, voices a screaming lead and a bass
    /// from the settings by name, plays notes and renders them.
    #[test]
    fn an_assistant_voices_and_plays_patch_twenty() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "patch-twenty"}))
            .unwrap();
        let t = t["track"].as_str().unwrap().to_string();
        let i = d.call("get_instrument", json!({"track": t})).unwrap();
        assert_eq!(i["instrument"]["kind"], "patch-twenty", "{i}");
        let list = settings(&mut d, &t);
        for key in [
            "hp_cutoff",
            "hp_peak",
            "lp_cutoff",
            "lp_peak",
            "mg_shape",
            "eg1_delay",
            "eg2_hold",
            "ext_in",
            "patch4_amount",
            "vintage",
        ] {
            setting(&list, key);
        }
        let ring = choice(&list, "osc2", "Ring mod");
        let keyboard = choice(&list, "patch1_from", "Keyboard");
        let both_cutoffs = choice(&list, "patch1_to", "Both cutoffs");
        let mg_tri = choice(&list, "patch2_from", "Mod generator triangle");
        let pulse_width = choice(&list, "patch2_to", "Pulse width");
        // A screaming lead: ring mod, both filters peaking, the
        // envelope opening the low-pass, the keyboard tracking both
        // filters and the mod generator moving the pulse width.
        d.call(
            "set_parameters",
            json!({"track": t, "changes": [
                {"param": "instrument.osc1", "value": 2},
                {"param": "instrument.osc2", "value": ring},
                {"param": "instrument.hp_cutoff", "value": 150.0},
                {"param": "instrument.hp_peak", "value": 0.7},
                {"param": "instrument.lp_cutoff", "value": 500.0},
                {"param": "instrument.lp_peak", "value": 0.95},
                {"param": "instrument.lp_eg2", "value": 3.0},
                {"param": "instrument.drive", "value": 0.8},
                {"param": "instrument.patch1_from", "value": keyboard},
                {"param": "instrument.patch1_to", "value": both_cutoffs},
                {"param": "instrument.patch1_amount", "value": 0.7},
                {"param": "instrument.patch2_from", "value": mg_tri},
                {"param": "instrument.patch2_to", "value": pulse_width},
                {"param": "instrument.patch2_amount", "value": 0.6},
            ]}),
        )
        .unwrap();
        d.call(
            "add_note_clip",
            json!({"track": t, "bar": 1, "bars": 1, "notes": [
                {"beat": 0.0, "beats": 1.5, "pitch": 62, "velocity": 110},
                {"beat": 2.0, "beats": 1.5, "pitch": 69, "velocity": 100}
            ]}),
        )
        .unwrap();
        let a = d
            .call("analyse", json!({"track": t, "from_bar": 1, "to_bar": 2}))
            .unwrap();
        assert!(a["sample_peak_dbfs"].as_f64().unwrap() > -30.0, "{a}");
        let now = settings(&mut d, &t);
        assert_eq!(setting(&now, "osc2")["text"], "Ring mod");
        assert_eq!(setting(&now, "patch1_to")["text"], "Both cutoffs");
        // A bass on a second track, from a few settings, and both
        // tracks render in the mix.
        let b = d
            .call("add_track", json!({"kind": "patch-twenty", "name": "Bass"}))
            .unwrap();
        let b = b["track"].as_str().unwrap().to_string();
        d.call(
            "set_instrument",
            json!({"track": b, "settings": {
                "osc1": 1, "osc1_octave": 1, "osc2": 1, "osc2_octave": 0,
                "lp_cutoff": 250.0, "lp_peak": 0.5, "lp_eg2": 2.5,
                "eg2_decay": 0.25, "eg2_sustain": 0.1, "drive": 0.5, "glide": 0.08
            }}),
        )
        .unwrap();
        d.call(
            "add_note_clip",
            json!({"track": b, "bar": 1, "bars": 1, "notes": [
                {"beat": 0.0, "beats": 0.75, "pitch": 36, "velocity": 120},
                {"beat": 1.0, "beats": 0.75, "pitch": 36, "velocity": 90},
                {"beat": 2.0, "beats": 0.75, "pitch": 43, "velocity": 120},
                {"beat": 3.0, "beats": 0.75, "pitch": 41, "velocity": 100}
            ]}),
        )
        .unwrap();
        let a = d
            .call("analyse", json!({"track": b, "from_bar": 1, "to_bar": 2}))
            .unwrap();
        assert!(a["sample_peak_dbfs"].as_f64().unwrap() > -30.0, "{a}");
        let now = settings(&mut d, &b);
        assert_eq!(setting(&now, "osc1_octave")["text"], "16'");
    }

    /// The command adds a track and undo takes it away, and an
    /// assistant can give the track an audio input (the guitar) where
    /// an ordinary instrument track refuses one.
    #[test]
    fn the_track_command_and_the_external_input_reach_the_assistant() {
        let mut d = Driver::new();
        d.command("add-patch-twenty-track");
        let p = d.project();
        let tracks = p["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["instrument"]["builtin"], "patch-twenty", "{p}");
        let t = tracks[0]["id"].as_str().unwrap().to_string();
        // Without an audio device there are no inputs to pick, so the
        // answer is about the device, not a refusal for being an
        // instrument track.
        let e = d
            .call("set_track_input", json!({"track": t, "input": 1}))
            .unwrap_err();
        assert!(e.contains("inputs"), "{e}");
        assert!(!e.contains("plays its notes"), "{e}");
        let s = d.call("add_track", json!({"kind": "poly-synth"})).unwrap();
        let s = s["track"].as_str().unwrap();
        let e = d
            .call("set_track_input", json!({"track": s, "input": 1}))
            .unwrap_err();
        assert!(e.contains("plays its notes"), "{e}");
        d.command("undo");
        d.command("undo");
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), 0);
    }

    /// The sounds that come with it are presets an assistant can add,
    /// and the guitar one arrives with the external input switched on.
    #[test]
    fn its_presets_come_with_the_app() {
        let mut d = Driver::new();
        let list = d.call("list_presets", json!({})).unwrap();
        let names: Vec<&str> = (list.as_array().unwrap().iter())
            .filter_map(|p| p["name"].as_str())
            .collect();
        for name in [
            "Patch Twenty screaming lead",
            "Patch Twenty bass",
            "Patch Twenty drone",
            "Guitar through Patch Twenty",
        ] {
            assert!(names.contains(&name), "{names:?}");
        }
        d.call("add_preset", json!({"name": "Guitar through Patch Twenty"}))
            .unwrap();
        let p = d.project();
        let t = &p["tracks"][0];
        assert_eq!(t["instrument"]["builtin"], "patch-twenty", "{t}");
        let key = t["id"].as_str().unwrap().to_string();
        let now = settings(&mut d, &key);
        assert_eq!(setting(&now, "ext_in")["text"], "On");
        assert_eq!(setting(&now, "osc1_level")["value"], 0.0);
    }
}
