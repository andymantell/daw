// SPDX-License-Identifier: GPL-3.0-or-later
//! The guided audio setup's choices and checks (Phase 4 plan, D2): which
//! interface and driver to use, the sample rate and buffer size to start
//! from, and what an input's level, the dropouts and the monitoring
//! delay say, each in plain words.
//!
//! Nothing here opens a device: [`suggest`] works on the lists
//! [`list`](crate::list) and [`input_devices`](crate::input_devices)
//! give, so it can be tested without one.

use crate::{DeviceInfo, DeviceProfile, HostInfo, MonitorOutputs};

/// One device the guide can offer, best first.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub host: String,
    pub device: DeviceInfo,
    /// The device to record from with it, if any.
    pub input: Option<String>,
    /// Why it is offered, in plain words.
    pub why: String,
}

/// The rate the guide starts from where the device has it: what video
/// and most interfaces use, and what the app's own sounds are made at.
pub const RATE: u32 = 48_000;

/// Delay from playing to hearing yourself, in ms, above which many
/// players feel they are behind. A rule of thumb, not a measurement.
pub const FEEL_MS: f64 = 10.0;

fn is_asio(host: &str) -> bool {
    host.eq_ignore_ascii_case("asio")
}

/// Names a system gives its own stand-ins for "whatever is the default",
/// which are no use for choosing an interface.
fn is_stand_in(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "default",
        "sysdefault",
        "sound mapper",
        "primary sound",
        "pulse",
        "pipewire",
    ]
    .iter()
    .any(|s| n.contains(s))
}

/// The part of a name in brackets, which shared drivers use for the
/// hardware: "Speakers (Interface)" and "Line In (Interface)".
fn bracketed(name: &str) -> Option<&str> {
    let a = name.find('(')?;
    let b = name.rfind(')').filter(|&b| b > a)?;
    Some(name[a + 1..b].trim()).filter(|s| !s.is_empty())
}

/// The input to record from with `device`: itself where it records,
/// otherwise the input with the same hardware name in brackets.
fn input_for(device: &DeviceInfo, inputs: &[String]) -> Option<String> {
    if inputs.contains(&device.name) {
        return Some(device.name.clone());
    }
    let hw = bracketed(&device.name)?;
    inputs.iter().find(|i| bracketed(i) == Some(hw)).cloned()
}

fn score(host: &str, d: &DeviceInfo, input: bool) -> i32 {
    let mut s = 0;
    if is_asio(host) {
        s += 100;
    }
    if input {
        s += 20;
    }
    if d.output_channels > 2 || d.input_channels > 2 {
        s += 10;
    }
    if is_stand_in(&d.name) {
        s -= 30;
    }
    s
}

/// Every device that can play, best first: an interface on its maker's
/// low-latency driver, then one that also records, then the rest.
/// `inputs` gives the recording devices on a driver.
pub fn candidates(hosts: &[HostInfo], inputs: impl Fn(&str) -> Vec<String>) -> Vec<Candidate> {
    let mut all: Vec<(i32, Candidate)> = Vec::new();
    for h in hosts {
        let ins = inputs(&h.name);
        for d in &h.devices {
            let input = input_for(d, &ins);
            let why = if is_asio(&h.name) {
                "Uses the interface maker's own driver, which has the least delay.".to_string()
            } else if input.is_some() {
                format!("Plays and records through the {} driver.", h.name)
            } else {
                format!("Plays through the {} driver, but can't record.", h.name)
            };
            all.push((
                score(&h.name, d, input.is_some()),
                Candidate {
                    host: h.name.clone(),
                    device: d.clone(),
                    input,
                    why,
                },
            ));
        }
    }
    // Stable: equal scores keep the drivers' own order.
    all.sort_by_key(|(s, _)| -s);
    all.into_iter().map(|(_, c)| c).collect()
}

/// The buffer size to start from: small enough to play through the app
/// without feeling late, or a safer size when the interface's own
/// monitoring is used, within what the driver allows.
pub fn buffer_for(range: Option<(u32, u32)>, through_app: bool) -> u32 {
    let want = if through_app { 128 } else { 256 };
    match range {
        Some((lo, hi)) => want.clamp(lo.max(1), hi.max(lo)),
        None => want,
    }
}

/// The next buffer size up from `b`, for when the sound breaks up.
pub fn bigger_buffer(b: Option<u32>, range: Option<(u32, u32)>) -> Option<u32> {
    let next = b.map_or(512, |b| (b * 2).max(64));
    match range {
        Some((_, hi)) if next > hi => None,
        _ if next > 4096 => None,
        _ => Some(next),
    }
}

/// A profile to start from on `c`, keeping what was already measured or
/// chosen for that device in `before`.
pub fn profile_for(
    c: &Candidate,
    before: Option<&DeviceProfile>,
    through_app: bool,
) -> DeviceProfile {
    let d = &c.device;
    let rate = if d.rates.is_empty() || d.rates.contains(&RATE) {
        RATE
    } else {
        d.default_rate.unwrap_or(d.rates[0])
    };
    let mut p = before.cloned().unwrap_or(DeviceProfile {
        host: c.host.clone(),
        device: d.name.clone(),
        rate,
        buffer: None,
        main: (0, 1.min(d.output_channels.saturating_sub(1) as usize)),
        click: None,
        input: None,
        record_offset: None,
        measured_at: None,
        direct_monitoring: false,
        monitor: MonitorOutputs::default(),
    });
    if p.input.is_none() {
        p.input = c.input.clone();
    }
    p.direct_monitoring = !through_app;
    if p.buffer.is_none() {
        p.buffer = Some(buffer_for(d.buffer_range, through_app));
    }
    p
}

/// What the guide found wrong, each with what to do about it.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// No device can play.
    NoDevices,
    /// On Windows, only the shared system drivers were found.
    NoMakerDriver,
    /// The chosen device can't record.
    NoInput,
    /// The device would not open, with the driver's reason.
    WontOpen(String),
    /// The device stopped while running.
    Stopped,
    /// The sound broke up this many times while checking.
    Dropouts(u64),
    /// Playing through the app reaches the ears this many ms late.
    LateMonitoring(f64),
    /// The input level, when it isn't good.
    Level(Level),
    /// The recording offset changed between runs of the measurement.
    UnsteadyLatency,
    /// The loopback signal came back upside down.
    Inverted,
}

impl Problem {
    /// What it means and what to do, in plain words.
    pub fn explain(&self) -> String {
        match self {
            Problem::NoDevices => "No audio devices were found. Plug the interface in and switch it on, \
                install its driver if it came with one, then press Look again."
                .into(),
            Problem::NoMakerDriver => "Only the system's shared sound drivers were found. They work, but add \
                delay between playing and hearing. If your interface's maker offers an ASIO driver, \
                install it from their website, restart the app and run this again."
                .into(),
            Problem::NoInput => "This device can't record, so there is nothing to plug a guitar or \
                microphone into. Choose your interface instead, or a device with inputs."
                .into(),
            Problem::WontOpen(why) => format!(
                "The device wouldn't start ({why}). Another program may be using it: close other \
                 music and video programs, or set them to a different device, then try again. \
                 A sample rate or buffer size the driver doesn't offer can also stop it."
            ),
            Problem::Stopped => "The device stopped. Check its cable and power, then try again.".into(),
            Problem::Dropouts(n) => format!(
                "The sound broke up {n} time{} while checking: the computer couldn't fill the buffer in \
                 time. Try the next bigger buffer size.",
                if *n == 1 { "" } else { "s" }
            ),
            Problem::LateMonitoring(ms) => format!(
                "Through the app you hear yourself about {ms:.0} ms after you play. Above about {FEEL_MS:.0} ms \
                 many players feel behind. Use a smaller buffer, or listen through the interface's \
                 direct monitoring instead."
            ),
            Problem::Level(l) => l.advice().into(),
            Problem::UnsteadyLatency => "The delay measured was different each time the audio started, so \
                recordings can't be lined up exactly. Try another buffer size or sample rate and \
                measure again."
                .into(),
            Problem::Inverted => "The test came back upside down: the cable or the interface flips the \
                signal. Recordings still line up; it only matters when mixing a track with a copy of itself."
                .into(),
        }
    }
}

/// How loud an input is, from its peaks over the last moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Nothing at all is coming in.
    Silent,
    /// Something, but far below where it should be.
    Quiet,
    Good,
    /// Close to the top: one loud note will clip.
    Hot,
    /// Hitting the top: recordings will crackle.
    Clipping,
}

impl Level {
    /// Peak below this, in dBFS: nothing playing.
    pub const SILENT_DB: f32 = -60.0;
    /// Peak below this: too quiet.
    pub const QUIET_DB: f32 = -24.0;
    /// Peak above this: too close to the top.
    pub const HOT_DB: f32 = -3.0;
    /// Peak at or above this: clipped.
    pub const CLIP_DB: f32 = -0.1;

    /// The level of a peak (a sample's size, 1.0 being the top).
    pub fn of(peak: f32) -> Level {
        let db = to_db(peak);
        if db >= Level::CLIP_DB {
            Level::Clipping
        } else if db > Level::HOT_DB {
            Level::Hot
        } else if db >= Level::QUIET_DB {
            Level::Good
        } else if db >= Level::SILENT_DB {
            Level::Quiet
        } else {
            Level::Silent
        }
    }

    pub fn advice(self) -> &'static str {
        match self {
            Level::Silent => {
                "Nothing is coming in on this input. Check the cable is in this input, the \
                interface is switched on, and its gain knob isn't all the way down. A condenser \
                microphone also needs the interface's 48 V (phantom power) switch on. For a guitar \
                plugged straight in, press the input's instrument (Hi-Z) switch if it has one."
            }
            Level::Quiet => {
                "This input is quiet. Turn its gain knob on the interface up while playing as \
                loud as you will, until the bar turns green and this says Good."
            }
            Level::Good => "Good: the loudest notes have room to spare.",
            Level::Hot => {
                "Nearly too loud: the loudest notes come close to the top. Turn the gain knob \
                down a little to leave room."
            }
            Level::Clipping => {
                "Too loud: the input is hitting the top, which crackles in recordings and \
                can't be fixed afterwards. Turn the gain knob down until the clip light stays off, or \
                switch off a pad or boost the input has on."
            }
        }
    }
}

/// A sample's size in dBFS.
pub fn to_db(peak: f32) -> f32 {
    20.0 * peak.max(1e-9).log10()
}

/// The delay from playing to hearing yourself through the app, in ms:
/// what the driver reports if it does, otherwise a buffer in and a
/// buffer out.
pub fn monitoring_ms(rate: u32, buffer: u32, reported_us: (u64, u64)) -> f64 {
    let reported = (reported_us.0 + reported_us.1) as f64 / 1000.0;
    if reported > 0.0 {
        reported
    } else {
        2.0 * buffer as f64 * 1000.0 / rate.max(1) as f64
    }
}

/// Problems with the drivers found, before any device is chosen.
pub fn driver_problems(hosts: &[HostInfo], windows: bool) -> Vec<Problem> {
    if hosts.iter().all(|h| h.devices.is_empty()) {
        return vec![Problem::NoDevices];
    }
    let maker = hosts
        .iter()
        .any(|h| is_asio(&h.name) && !h.devices.is_empty());
    if windows && !maker {
        return vec![Problem::NoMakerDriver];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, outs: u16, ins: u16) -> DeviceInfo {
        DeviceInfo {
            name: name.into(),
            output_channels: outs,
            input_channels: ins,
            default_rate: Some(44_100),
            rates: vec![44_100, 48_000, 96_000],
            buffer_range: Some((32, 2048)),
        }
    }

    fn host(name: &str, devices: Vec<DeviceInfo>) -> HostInfo {
        HostInfo {
            name: name.into(),
            devices,
        }
    }

    #[test]
    fn the_makers_driver_is_offered_first_then_a_device_that_records() {
        let hosts = vec![
            host(
                "WASAPI",
                vec![
                    dev("Speakers (Realtek Audio)", 2, 0),
                    dev("Speakers (Interface 18)", 10, 0),
                ],
            ),
            host("ASIO", vec![dev("Interface ASIO Driver", 10, 18)]),
        ];
        let c = candidates(&hosts, |h| match h {
            "WASAPI" => vec!["Line (Interface 18)".into()],
            _ => vec!["Interface ASIO Driver".into()],
        });
        assert_eq!(c[0].host, "ASIO");
        assert_eq!(c[0].input.as_deref(), Some("Interface ASIO Driver"));
        assert_eq!(c[1].device.name, "Speakers (Interface 18)");
        assert_eq!(c[1].input.as_deref(), Some("Line (Interface 18)"));
        assert_eq!(c[2].input, None);
        assert!(c[2].why.contains("can't record"));
        assert!(driver_problems(&hosts, true).is_empty());
    }

    #[test]
    fn brackets_inside_the_hardware_name_are_kept() {
        assert_eq!(
            bracketed("Speakers (Maker(R) Audio)"),
            Some("Maker(R) Audio")
        );
        let d = dev("Speakers (Maker(R) Audio)", 2, 0);
        let other = vec!["Microphone (Other(R) Device)".to_string()];
        assert_eq!(input_for(&d, &other), None);
    }

    #[test]
    fn stand_ins_for_the_default_come_after_real_devices() {
        let hosts = vec![host(
            "ALSA",
            vec![dev("default", 2, 2), dev("USB Interface", 2, 2)],
        )];
        let c = candidates(&hosts, |_| vec!["default".into(), "USB Interface".into()]);
        assert_eq!(c[0].device.name, "USB Interface");
    }

    #[test]
    fn windows_without_a_makers_driver_is_explained() {
        let hosts = vec![host("WASAPI", vec![dev("Speakers", 2, 0)])];
        assert_eq!(driver_problems(&hosts, true), vec![Problem::NoMakerDriver]);
        assert!(driver_problems(&hosts, false).is_empty());
        let none = vec![host("WASAPI", vec![])];
        assert_eq!(driver_problems(&none, true), vec![Problem::NoDevices]);
    }

    #[test]
    fn the_profile_starts_at_48k_with_a_buffer_for_how_you_listen() {
        let c = &candidates(&[host("ASIO", vec![dev("Iface", 10, 18)])], |_| {
            vec!["Iface".into()]
        })[0];
        let p = profile_for(c, None, true);
        assert_eq!(
            (p.rate, p.buffer, p.direct_monitoring),
            (48_000, Some(128), false)
        );
        assert_eq!(p.input.as_deref(), Some("Iface"));
        let p = profile_for(c, None, false);
        assert_eq!((p.buffer, p.direct_monitoring), (Some(256), true));
        // What was measured before is kept.
        let mut before = p.clone();
        before.record_offset = Some(321);
        before.buffer = Some(512);
        let p = profile_for(c, Some(&before), true);
        assert_eq!((p.record_offset, p.buffer), (Some(321), Some(512)));
    }

    #[test]
    fn buffers_stay_in_the_drivers_range() {
        assert_eq!(buffer_for(Some((256, 1024)), true), 256);
        assert_eq!(buffer_for(Some((16, 64)), false), 64);
        assert_eq!(buffer_for(None, true), 128);
        assert_eq!(bigger_buffer(Some(128), Some((32, 2048))), Some(256));
        assert_eq!(bigger_buffer(Some(2048), Some((32, 2048))), None);
        assert_eq!(bigger_buffer(None, None), Some(512));
    }

    #[test]
    fn levels_are_judged_from_the_peak() {
        assert_eq!(Level::of(0.0), Level::Silent);
        assert_eq!(Level::of(0.01), Level::Quiet); // -40 dB
        assert_eq!(Level::of(0.25), Level::Good); // -12 dB
        assert_eq!(Level::of(0.8), Level::Hot); // -1.9 dB
        assert_eq!(Level::of(1.0), Level::Clipping);
        assert!(Level::Silent.advice().contains("48 V"));
    }

    #[test]
    fn monitoring_delay_uses_the_drivers_figures_when_it_gives_them() {
        assert!((monitoring_ms(48_000, 128, (0, 0)) - 5.333).abs() < 0.01);
        assert_eq!(monitoring_ms(48_000, 128, (6_000, 5_000)), 11.0);
        assert!(Problem::LateMonitoring(14.0).explain().contains("14 ms"));
    }
}
