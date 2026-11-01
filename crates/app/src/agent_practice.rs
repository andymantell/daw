// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tool for the practice controls on the transport bar
//! (roadmap 1.4, 2.11): speed, transpose, fine pitch, tuning detection,
//! the metronome, count-in and the tempo trainer.

use daw_engine::engine::Command;
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[Tool {
    name: "set_practice",
    description: "The practice controls: speed (without changing pitch), transpose and \
                  fine pitch (without changing speed; +1 plays a half-step-down recording \
                  in standard tuning), the metronome (on, clicks per beat, level), \
                  count-in, and the tempo trainer (speed up by a step every few loops, up \
                  to a target). detect_tuning measures how far a track's newest clip is \
                  from concert pitch, in cents. Tempo is set_tempo; the loop is set_loop.",
    access: Access::Edit,
    schema: || {
        json!({"type": "object", "properties": {
            "speed_pct": {"type": "number", "description": "50 to 150."},
            "semitones": {"type": "integer", "description": "-12 to 12."},
            "cents": {"type": "integer", "description": "-50 to 50."},
            "metronome": {"type": "object", "properties": {
                "on": {"type": "boolean"},
                "subdivision": {"type": "integer", "description": "Clicks per beat: 1, 2, 3 (triplets) or 4."},
                "level_db": {"type": "number", "description": "-40 to 0."}
            }},
            "count_in": {"type": "boolean"},
            "trainer": {"type": "object", "properties": {
                "on": {"type": "boolean"},
                "step_pct": {"type": "number", "description": "1 to 25."},
                "every_loops": {"type": "integer", "description": "1 to 32."},
                "target_pct": {"type": "number", "description": "50 to 150."}
            }},
            "detect_tuning": {"type": "string", "description": "A track's id."}
        }})
    },
}];

/// A number from `v`, which must lie in `lo..=hi`.
fn within(v: &Value, what: &str, lo: f64, hi: f64) -> Result<Option<f64>, String> {
    match v.as_f64() {
        None => Ok(None),
        Some(x) if (lo..=hi).contains(&x) => Ok(Some(x)),
        Some(_) => Err(format!("{what} goes from {lo} to {hi}.")),
    }
}

impl App {
    pub(crate) fn practice_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        (name == "set_practice").then(|| self.set_practice(args))
    }

    fn set_practice(&mut self, args: &Value) -> Done {
        // Check everything before changing anything.
        let speed = within(&args["speed_pct"], "Speed", 50.0, 150.0)?;
        let st = within(&args["semitones"], "Transpose", -12.0, 12.0)?;
        let cents = within(&args["cents"], "Fine pitch", -50.0, 50.0)?;
        let m = &args["metronome"];
        let sub = match m["subdivision"].as_u64() {
            None => None,
            Some(n @ 1..=4) => Some(n as u32),
            Some(_) => return Err("Clicks per beat are 1, 2, 3 or 4.".into()),
        };
        let level = within(&m["level_db"], "The click's level", -40.0, 0.0)?;
        let t = &args["trainer"];
        let step = within(&t["step_pct"], "The trainer's step", 1.0, 25.0)?;
        let every = within(&t["every_loops"], "Loops between steps", 1.0, 32.0)?;
        let target = within(&t["target_pct"], "The trainer's target", 50.0, 150.0)?;
        let source = match args["detect_tuning"].as_str() {
            None => None,
            Some(id) => {
                let t = (self.tracks.iter())
                    .find(|t| t.key == id)
                    .ok_or_else(|| format!("There is no track {id}."))?;
                let c = (t.clips.iter().rev())
                    .find(|c| !c.muted)
                    .ok_or("That track has no clip playing to measure.")?;
                Some(c.source.clone())
            }
        };

        let before = (self.speed_pct, self.semitones, self.cents);
        if let Some(x) = speed {
            self.speed_pct = x;
        }
        if let Some(x) = st {
            self.semitones = x as i32;
        }
        if let Some(x) = cents {
            self.cents = x as i32;
        }
        if (self.speed_pct, self.semitones, self.cents) != before {
            let c = self.practice();
            self.send(c);
        }
        let mut met = self.metronome;
        if let Some(b) = m["on"].as_bool() {
            met.enabled = b;
        }
        if let Some(n) = sub {
            met.subdivision = n as _;
        }
        if let Some(x) = level {
            met.level_db = x as f32;
        }
        if met != self.metronome {
            self.metronome = met;
            self.send(Command::SetMetronome(met));
        }
        if let Some(b) = args["count_in"].as_bool() {
            self.count_in = b;
        }
        if let Some(b) = t["on"].as_bool() {
            self.trainer.on = b;
        }
        if let Some(x) = step {
            self.trainer.step = x;
        }
        if let Some(x) = every {
            self.trainer.every = x as u64;
        }
        if let Some(x) = target {
            self.trainer.target = x;
        }
        let mut v = json!({
            "speed_pct": self.speed_pct, "semitones": self.semitones, "cents": self.cents,
            "metronome": {"on": self.metronome.enabled, "subdivision": self.metronome.subdivision,
                          "level_db": self.metronome.level_db},
            "count_in": self.count_in,
            "trainer": {"on": self.trainer.on, "step_pct": self.trainer.step,
                        "every_loops": self.trainer.every, "target_pct": self.trainer.target},
        });
        if let Some(src) = source {
            // The first minute and a half is plenty, as in the menu.
            let off = (src.read_mono(90 * src.rate as usize).ok())
                .and_then(|x| daw_engine::tuner::tuning_offset(&x, src.rate, self.tuner.a4));
            v["tuning_cents"] = json!(off);
            v["tuning_note"] = json!(match off {
                Some(c) if c.abs() >= 2.0 =>
                    format!("{c:+.0} cents off; cents {:+.0} corrects it", -c),
                Some(_) => "at concert pitch".into(),
                None => "no clear pitch to measure".into(),
            });
        }
        Ok((v, None))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    #[test]
    fn practice_controls_are_set_by_value() {
        let mut d = Driver::new();
        let r = d
            .call(
                "set_practice",
                json!({"speed_pct": 75, "semitones": 1, "cents": -10,
                       "metronome": {"on": true, "subdivision": 2, "level_db": -6},
                       "count_in": true,
                       "trainer": {"on": true, "step_pct": 5, "every_loops": 2, "target_pct": 100}}),
            )
            .unwrap();
        assert_eq!(r["metronome"]["subdivision"], 2);
        assert_eq!(r["trainer"]["on"], true);
        let t = d.call("get_transport", json!({})).unwrap();
        assert_eq!(t["speed_pct"], 75.0);
        assert_eq!(t["semitones"], 1);
        assert_eq!(t["cents"], -10);
        assert_eq!(t["metronome"], true);
        assert_eq!(t["count_in"], true);
        assert!(d.call("set_practice", json!({"speed_pct": 20})).is_err());
        assert!(
            d.call("set_practice", json!({"metronome": {"subdivision": 7}}))
                .is_err()
        );
        assert_eq!(
            d.call("get_transport", json!({})).unwrap()["speed_pct"],
            75.0,
            "a refused call changes nothing"
        );
    }
}
