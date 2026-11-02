// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for this computer's audio device (Settings → Audio)
//! and measuring its round trip with a loopback cable (Settings →
//! Latency). These belong to the computer, not the project.

use std::sync::atomic::Ordering;

use daw_audio_io::DeviceProfile;
use serde_json::{Value, json};

use crate::agent::{Access, Tool};
use crate::{App, Audio};

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

/// Buffer sizes offered, as in the device window.
const BUFFERS: [u32; 7] = [32, 64, 128, 256, 512, 1024, 2048];

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "set_audio_device",
        description: "This computer's audio device (Settings → Audio): with no arguments, \
                      the drivers and their devices (channels, sample rates, buffer sizes), \
                      the input devices, the settings in use and whether it is running. Any \
                      setting given is checked, then used at once and saved for that device: \
                      driver and device (a device used before starts from its own settings), \
                      sample rate, buffer (frames, or null for the driver's), mix_buffer \
                      (the bigger buffer for the rest of the mix while live tracks play, or \
                      null), main outputs (left and right, from 1), the input device (or \
                      null), record_offset (frames, or null to use the driver's reported \
                      latency; measure_latency measures it), direct_monitoring, and the \
                      metronome's own output (from 1, or null for the main mix). rescan looks \
                      for devices again.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "driver": {"type": "string"},
                "device": {"type": "string"},
                "rate": {"type": "integer"},
                "buffer": {"type": ["integer", "null"]},
                "mix_buffer": {"type": ["integer", "null"]},
                "main_outputs": {"type": "array", "items": {"type": "integer"},
                                 "description": "Left and right, from 1."},
                "input": {"type": ["string", "null"]},
                "record_offset": {"type": ["integer", "null"]},
                "direct_monitoring": {"type": "boolean"},
                "click_output": {"type": ["integer", "null"]},
                "rescan": {"type": "boolean"}
            }})
        },
    },
    Tool {
        name: "measure_latency",
        description: "Measures the audio device's round trip with a loopback cable from \
                      main output left to an input (Settings → Latency), and saves it as the \
                      recording offset. The app's own audio stops while it runs, and it plays \
                      a few short chirps, so ask before starting. input is the input channel \
                      the cable goes into (from 1); start begins, cancel stops. With no \
                      arguments, how far it is and the last result.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "input": {"type": "integer"},
                "start": {"type": "boolean"},
                "cancel": {"type": "boolean"}
            }})
        },
    },
];

impl App {
    pub(crate) fn device_tool(&mut self, name: &str, args: &Value) -> Option<Done> {
        match name {
            "set_audio_device" => Some(self.set_audio_device(args)),
            "measure_latency" => Some(self.measure_latency(args)),
            _ => None,
        }
    }

    /// The settings the device window starts from.
    fn device_draft(&self) -> DeviceProfile {
        (self.draft.clone().or_else(|| self.profile.clone())).unwrap_or_else(|| DeviceProfile {
            host: self.hosts.first().map_or(String::new(), |h| h.name.clone()),
            device: String::new(),
            rate: 48_000,
            buffer: None,
            main: (0, 1),
            click: None,
            input: None,
            record_offset: None,
            measured_at: None,
            direct_monitoring: false,
            monitor: daw_audio_io::MonitorOutputs::default(),
            mix_buffer: None,
        })
    }

    fn set_audio_device(&mut self, args: &Value) -> Done {
        if args["rescan"].as_bool() == Some(true) {
            self.hosts = daw_audio_io::list();
            self.input_devices = crate::input_names(&crate::draft_host(&self.draft));
        }
        let setting = [
            "driver",
            "device",
            "rate",
            "buffer",
            "mix_buffer",
            "main_outputs",
            "input",
            "record_offset",
            "direct_monitoring",
            "click_output",
        ];
        if setting.iter().any(|k| args.get(*k).is_some()) {
            let p = self.checked_device(args)?;
            self.draft = Some(p);
            self.apply_draft();
        }
        Ok((self.device_value(), None))
    }

    /// The device window's settings with `args` put in, all checked.
    fn checked_device(&self, args: &Value) -> Result<DeviceProfile, String> {
        let mut p = self.device_draft();
        if let Some(h) = args["driver"].as_str() {
            if !self.hosts.iter().any(|x| x.name == h) {
                return Err(format!("There is no driver {h}."));
            }
            if p.host != h {
                p.host = h.to_string();
                p.device.clear();
            }
        }
        let host = self.hosts.iter().find(|h| h.name == p.host);
        if let Some(d) = args["device"].as_str() {
            if !host.is_some_and(|h| h.devices.iter().any(|x| x.name == d)) {
                return Err(format!("{} has no device {d}.", p.host));
            }
            if p.device != d {
                // Used before: start from its settings.
                p = match self.saved.for_device(&p.host, d) {
                    Some(saved) => saved.clone(),
                    None => DeviceProfile {
                        device: d.to_string(),
                        ..p
                    },
                };
            }
        }
        let dev = host.and_then(|h| h.devices.iter().find(|x| x.name == p.device));
        let Some(dev) = dev else {
            return Err("Choose a device first.".into());
        };
        if let Some(r) = args["rate"].as_u64() {
            let r = r as u32;
            if !dev.rates.contains(&r) {
                return Err(format!("{} runs at {:?} Hz, not {r}.", dev.name, dev.rates));
            }
            p.rate = r;
        }
        if let Some(b) = args.get("buffer") {
            p.buffer = match b.as_u64() {
                None if b.is_null() => None,
                None => return Err("buffer is a number of frames, or null.".into()),
                Some(b) => {
                    let b = b as u32;
                    let ok = (dev.buffer_range).is_none_or(|(lo, hi)| (lo..=hi).contains(&b));
                    if !BUFFERS.contains(&b) || !ok {
                        return Err(format!(
                            "{b} frames isn't a buffer size {} takes.",
                            dev.name
                        ));
                    }
                    Some(b)
                }
            };
        }
        if let Some(b) = args.get("mix_buffer") {
            p.mix_buffer = match b.as_u64() {
                None if b.is_null() => None,
                Some(b) if daw_engine::ahead::MIX_BLOCKS.contains(&(b as u32)) => Some(b as u32),
                _ => {
                    return Err(format!(
                        "mix_buffer is null or one of {:?}.",
                        daw_engine::ahead::MIX_BLOCKS
                    ));
                }
            };
        }
        let outs = (dev.output_channels as usize).max(1);
        let output = |v: &Value| -> Result<usize, String> {
            match v.as_u64() {
                Some(c) if (1..=outs as u64).contains(&c) => Ok(c as usize - 1),
                _ => Err(format!("{} has outputs 1 to {outs}.", dev.name)),
            }
        };
        if let Some(m) = args["main_outputs"].as_array() {
            let [l, r] = m.as_slice() else {
                return Err("main_outputs is the left and the right output.".into());
            };
            p.main = (output(l)?, output(r)?);
        }
        if let Some(c) = args.get("click_output") {
            p.click = if c.is_null() { None } else { Some(output(c)?) };
        }
        if let Some(i) = args.get("input") {
            p.input = match i.as_str() {
                None => None,
                Some(name) if self.input_devices.iter().any(|x| x == name) => {
                    Some(name.to_string())
                }
                Some(name) => return Err(format!("There is no input device {name}.")),
            };
        }
        if let Some(o) = args.get("record_offset") {
            p.record_offset = match o.as_u64() {
                None => None,
                Some(o) if o <= 48_000 => Some(o as u32),
                Some(_) => return Err("The offset is 0 to 48000 frames.".into()),
            };
            p.measured_at = None;
        }
        if let Some(d) = args["direct_monitoring"].as_bool() {
            p.direct_monitoring = d;
        }
        Ok(p)
    }

    fn device_value(&self) -> Value {
        let drivers: Vec<Value> = (self.hosts.iter())
            .map(|h| {
                let devices: Vec<Value> = (h.devices.iter())
                    .map(|d| {
                        json!({"name": d.name, "outputs": d.output_channels,
                               "inputs": d.input_channels, "rates": d.rates,
                               "buffers": d.buffer_range.map(|(lo, hi)| [lo, hi])})
                    })
                    .collect();
                json!({"driver": h.name, "devices": devices})
            })
            .collect();
        let using = self.profile.as_ref().map(|p| {
            json!({
                "driver": p.host, "device": p.device, "rate": p.rate, "buffer": p.buffer,
                "mix_buffer": p.mix_buffer,
                "main_outputs": [p.main.0 + 1, p.main.1 + 1],
                "input": p.input, "record_offset": p.record_offset,
                "measured_at": p.measured_at.map(|(rate, buffer)| json!({"rate": rate, "buffer": buffer})),
                "direct_monitoring": p.direct_monitoring,
                "click_output": p.click.map(|c| c + 1),
            })
        });
        let running = match &self.audio {
            Audio::Running(r) => json!({
                "running": !r.status.failed.load(Ordering::Relaxed),
                "rate": r.rate, "channels": r.channels,
                "input_channels": r.input_channels, "input_problem": r.input_error,
            }),
            Audio::Silent(_, why) => json!({"running": false, "problem": why}),
            Audio::Stopped => json!({"running": false}),
        };
        json!({"drivers": drivers, "input_devices": self.input_devices,
               "using": using, "audio": running})
    }

    fn measure_latency(&mut self, args: &Value) -> Done {
        if self.draft.is_none() {
            self.draft = self.profile.clone();
        }
        if let Some(d) = &self.draft {
            self.latency.inputs = crate::input_channels(&d.host, d.input.as_deref());
        }
        if let Some(c) = args["input"].as_u64() {
            let n = self.latency.inputs.max(1);
            if !(1..=n as u64).contains(&c) {
                return Err(format!("The input device has inputs 1 to {n}."));
            }
            if self.latency.busy() {
                return Err("It is measuring; cancel first.".into());
            }
            self.latency.set_channel(c as usize - 1);
        }
        if args["cancel"].as_bool() == Some(true) && self.latency.busy() {
            self.latency.cancel();
            self.restart_audio();
        }
        if args["start"].as_bool() == Some(true) {
            if self.recording {
                return Err("Stop recording first.".into());
            }
            if self.latency.busy() {
                return Err("It is measuring already.".into());
            }
            let target = self.measure_target();
            if let Err(e) = &target {
                return Err(e.clone());
            }
            self.start_measuring(target);
        }
        let last = self.latency.last().map(|r| match r {
            Ok(m) => json!({"frames": m.frames, "ms": m.ms(), "steady": m.stable(),
                            "upside_down": m.inverted()}),
            Err(e) => json!({"problem": e}),
        });
        Ok((
            json!({"measuring": self.latency.progress(), "input": self.latency.channel() + 1,
                   "inputs": self.latency.inputs, "last": last}),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use daw_audio_io::{DeviceInfo, HostInfo};
    use serde_json::json;

    use crate::agent::driver::Driver;

    fn with_a_device() -> Driver {
        let mut d = Driver::new();
        d.app().hosts = vec![HostInfo {
            name: "Test driver".into(),
            devices: vec![DeviceInfo {
                name: "Test interface".into(),
                output_channels: 8,
                input_channels: 8,
                default_rate: Some(48_000),
                rates: vec![44_100, 48_000, 96_000],
                buffer_range: Some((64, 1024)),
            }],
        }];
        d.app().input_devices = vec!["Test interface".into()];
        d
    }

    #[test]
    fn an_assistant_chooses_and_sets_up_the_audio_device() {
        let mut d = with_a_device();
        let r = d.call("set_audio_device", json!({})).unwrap();
        assert_eq!(
            r["drivers"][0]["devices"][0]["name"], "Test interface",
            "{r}"
        );
        // Checked before anything is used.
        for bad in [
            json!({"driver": "Nothing"}),
            json!({"driver": "Test driver", "device": "Nothing"}),
            json!({"driver": "Test driver", "device": "Test interface", "rate": 22_050}),
            json!({"driver": "Test driver", "device": "Test interface", "buffer": 32}),
            json!({"driver": "Test driver", "device": "Test interface", "main_outputs": [1, 9]}),
            json!({"driver": "Test driver", "device": "Test interface", "input": "Nothing"}),
        ] {
            assert!(d.call("set_audio_device", bad.clone()).is_err(), "{bad}");
        }
        let r = d
            .call(
                "set_audio_device",
                json!({"driver": "Test driver", "device": "Test interface", "rate": 96_000,
                       "buffer": 128, "mix_buffer": daw_engine::ahead::MIX_BLOCKS[0],
                       "main_outputs": [3, 4], "input": "Test interface",
                       "record_offset": 300, "direct_monitoring": true, "click_output": 5}),
            )
            .unwrap();
        let u = &r["using"];
        assert_eq!(
            (u["device"].clone(), u["rate"].clone(), u["buffer"].clone()),
            (json!("Test interface"), json!(96_000), json!(128)),
            "{r}"
        );
        assert_eq!(u["main_outputs"], json!([3, 4]));
        assert_eq!(u["click_output"], 5);
        assert_eq!(u["record_offset"], 300);
        assert_eq!(u["direct_monitoring"], true);
        assert_eq!(u["mix_buffer"], json!(daw_engine::ahead::MIX_BLOCKS[0]));
        // The fake device can't open: it plays silently and says why.
        assert_eq!(r["audio"]["running"], false);
        // Back to the driver's buffer and latency, the click on the main mix.
        let r = d
            .call(
                "set_audio_device",
                json!({"buffer": null, "record_offset": null, "click_output": null}),
            )
            .unwrap();
        let u = &r["using"];
        assert_eq!(
            (
                u["buffer"].clone(),
                u["record_offset"].clone(),
                u["click_output"].clone()
            ),
            (json!(null), json!(null), json!(null))
        );
        assert_eq!(u["rate"], 96_000, "the rest kept");
    }

    #[test]
    fn an_assistant_measures_the_round_trip() {
        let mut d = with_a_device();
        d.app().loopback_for_tests = Some(321);
        assert!(
            d.call("measure_latency", json!({"start": true})).is_err(),
            "no device yet"
        );
        d.call(
            "set_audio_device",
            json!({"driver": "Test driver", "device": "Test interface",
                   "input": "Test interface", "buffer": 256}),
        )
        .unwrap();
        assert!(d.call("measure_latency", json!({"input": 99})).is_err());
        let r = d
            .call("measure_latency", json!({"input": 1, "start": true}))
            .unwrap();
        assert!(r["measuring"].is_number(), "{r}");
        let mut r = r;
        for _ in 0..1000 {
            d.h.step();
            r = d.call("measure_latency", json!({})).unwrap();
            if r["measuring"].is_null() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let frames = r["last"]["frames"]
            .as_u64()
            .unwrap_or_else(|| panic!("{r}"));
        let u = d.call("set_audio_device", json!({})).unwrap();
        assert_eq!(u["using"]["record_offset"], frames, "saved as the offset");

        // Started and cancelled.
        d.call("measure_latency", json!({"start": true})).unwrap();
        let r = d.call("measure_latency", json!({"cancel": true})).unwrap();
        assert_eq!(r["measuring"], json!(null));
    }
}
