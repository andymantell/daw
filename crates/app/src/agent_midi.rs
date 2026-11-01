// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for MIDI learn (roadmap 2.11): reading and changing
//! which control on a MIDI device is tied to which setting, as the MIDI
//! learn window does, and moving a control as if from a device, to try a
//! tie (a footswitch stepping a switching container's chains, say).

use daw_engine::midi::{Captured, Event, Kind};
use daw_project::MidiMap;
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::automation;

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_midi_maps",
        description: "The controls on MIDI devices tied to settings (MIDI learn): each \
                      tie's id, the controller's channel (1 to 16) and number, the track \
                      and setting (as list_parameters names it), and step: whether each \
                      press of a footswitch steps the setting on to its next choice, in \
                      place of following the control's position.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "edit_midi_maps",
        description: "Ties a MIDI device's controller to a track's setting (add; a \
                      controller tied before is tied afresh, as is the setting), unties \
                      one (remove, by id), or changes whether a tie steps (change). \
                      step suits a footswitch on a setting with choices, such as a \
                      switching container's heard chain (fx:<id>:heard), which it is \
                      for unless told otherwise. move acts as if a device's controller \
                      moved to a value (0 to 127), to try a tie: a footswitch press is \
                      a move to 127 after one below 64. Answers with the ties, as \
                      get_midi_maps does.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "add": {"type": "object", "properties": {
                    "channel": {"type": "integer", "description": "1 to 16."},
                    "cc": {"type": "integer", "description": "The controller's number, 0 to 127."},
                    "track": {"type": "string"},
                    "param": {"type": "string", "description": "A setting, as list_parameters names it."},
                    "step": {"type": "boolean"}
                }, "required": ["channel", "cc", "track", "param"]},
                "remove": {"type": "string", "description": "A tie's id."},
                "change": {"type": "object", "properties": {
                    "id": {"type": "string"},
                    "step": {"type": "boolean"}
                }, "required": ["id"]},
                "move": {"type": "object", "properties": {
                    "channel": {"type": "integer", "description": "1 to 16."},
                    "cc": {"type": "integer"},
                    "value": {"type": "integer", "description": "0 to 127."}
                }, "required": ["channel", "cc", "value"]}
            }})
        },
    },
];

/// A whole number from `v[key]` in `lo..=hi`.
fn number(v: &Value, key: &str, lo: i64, hi: i64) -> Result<i64, String> {
    match v[key].as_i64() {
        Some(n) if (lo..=hi).contains(&n) => Ok(n),
        _ => Err(format!("Give {key} as a whole number from {lo} to {hi}.")),
    }
}

impl App {
    pub(crate) fn midi_read(&mut self, name: &str) -> Option<Result<Value, String>> {
        (name == "get_midi_maps").then(|| Ok(self.midi_maps_value()))
    }

    pub(crate) fn midi_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        (name == "edit_midi_maps").then(|| self.edit_midi_maps(args))
    }

    fn midi_maps_value(&self) -> Value {
        let buses: Vec<(String, String)> = (self.tracks.iter())
            .filter(|t| t.kind.is_bus())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        let maps: Vec<Value> = (self.midi_maps.iter())
            .map(|m| {
                let name = (self.tracks.iter().find(|t| t.key == m.track))
                    .map(|t| automation::name_of(t, &buses, &m.param));
                json!({
                    "id": m.id,
                    "channel": m.channel + 1,
                    "cc": m.cc,
                    "track": m.track,
                    "param": m.param,
                    "name": name,
                    "step": m.step,
                })
            })
            .collect();
        json!({"maps": maps})
    }

    fn edit_midi_maps(&mut self, args: &Value) -> Done {
        let mut label = None;
        let add = &args["add"];
        if add.is_object() {
            let channel = number(add, "channel", 1, 16)? as u8 - 1;
            let cc = number(add, "cc", 0, 127)? as u8;
            let track = add["track"].as_str().ok_or("Give the track's id.")?;
            let param = add["param"].as_str().ok_or("Give the setting.")?;
            let t = (self.tracks.iter())
                .find(|t| t.key == track)
                .ok_or_else(|| format!("There is no track {track}."))?;
            if automation::range(t, param).is_none() || automation::control(t, param).is_none() {
                return Err(format!("{} has no setting {param}.", t.name));
            }
            let step = (add["step"].as_bool())
                .unwrap_or_else(|| crate::switch::is_heard(&self.tracks, track, param));
            self.midi_maps.retain(|m| {
                (m.channel, m.cc) != (channel, cc)
                    && (m.track.as_str(), m.param.as_str()) != (track, param)
            });
            self.midi_maps.push(MidiMap {
                id: daw_project::new_id(),
                channel,
                cc,
                track: track.into(),
                param: param.into(),
                step,
            });
            label = Some(format!("Tie controller {cc}"));
        }
        if let Some(id) = args["remove"].as_str() {
            let i = (self.midi_maps.iter())
                .position(|m| m.id == id)
                .ok_or_else(|| format!("There is no tie {id}."))?;
            let m = self.midi_maps.remove(i);
            label = Some(format!("Untie controller {}", m.cc));
        }
        let change = &args["change"];
        if change.is_object() {
            let id = change["id"].as_str().ok_or("Give the tie's id.")?;
            let m = (self.midi_maps.iter_mut())
                .find(|m| m.id == id)
                .ok_or_else(|| format!("There is no tie {id}."))?;
            if let Some(step) = change["step"].as_bool() {
                m.step = step;
            }
            label = Some(format!("Change the tie for controller {}", m.cc));
        }
        let moved = &args["move"];
        if moved.is_object() {
            let channel = number(moved, "channel", 1, 16)? as u8 - 1;
            let number_ = number(moved, "cc", 0, 127)? as u8;
            let value = number(moved, "value", 0, 127)? as u8;
            let c = Captured {
                port: 0,
                event: Event {
                    frame: 0,
                    channel,
                    kind: Kind::Control {
                        number: number_,
                        value,
                    },
                },
                at: 0,
                playing: false,
                recording: false,
            };
            self.midi_learn(&[c]);
            label = label.or(Some(format!("Move controller {number_}")));
        }
        if label.is_none() {
            return Err("Give add, remove, change or move.".into());
        }
        Ok((self.midi_maps_value(), label))
    }
}
