// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings → Guided audio setup… (Phase 4 plan, D2): a step at a time,
//! picks the interface and its driver, how the player listens, a buffer
//! size (checked for dropouts), the input's level, and measures latency
//! with the existing tool, explaining each problem found in plain words.
//! It opens by itself the first time the app starts with no device
//! settings saved.
//!
//! Every step is a command (Next, Skip, Back, Cancel), so the guide can
//! be walked from the keyboard, the phone remote or an assistant, taking
//! the suggested choice each time.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use daw_audio_io::setup::{self, Candidate, Level, Problem};
use daw_audio_io::{self as audio, HostInfo};
use daw_engine::engine::Command;
use eframe::egui::{self, RichText};

use crate::commands::Cmd;
use crate::{App, Audio};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Step {
    #[default]
    Device,
    Monitoring,
    Buffer,
    Input,
    Latency,
    Done,
}

const STEPS: [Step; 6] = [
    Step::Device,
    Step::Monitoring,
    Step::Buffer,
    Step::Input,
    Step::Latency,
    Step::Done,
];

impl Step {
    fn index(self) -> usize {
        STEPS.iter().position(|&s| s == self).unwrap_or(0)
    }

    fn next(self) -> Step {
        STEPS[(self.index() + 1).min(STEPS.len() - 1)]
    }

    fn back(self) -> Step {
        STEPS[self.index().saturating_sub(1)]
    }

    fn title(self) -> &'static str {
        match self {
            Step::Device => "Your audio interface",
            Step::Monitoring => "How you listen while playing",
            Step::Buffer => "Buffer size",
            Step::Input => "Input level",
            Step::Latency => "Recording delay",
            Step::Done => "All set",
        }
    }
}

/// How long the buffer check listens for dropouts.
const CHECK: Duration = Duration::from_secs(3);
/// How long the input meter holds its highest peak.
const HOLD: Duration = Duration::from_millis(1500);
/// Buffer sizes offered, where the driver allows them.
const BUFFERS: [u32; 7] = [32, 64, 128, 256, 512, 1024, 2048];

#[derive(Default)]
pub(crate) struct AudioSetup {
    pub(crate) open: bool,
    pub(crate) step: Step,
    /// Devices found, best first, and the one picked.
    pub(crate) cands: Vec<Candidate>,
    pub(crate) pick: usize,
    /// Problems with the drivers found.
    drivers: Vec<Problem>,
    /// The player hears themselves through the app (with its effects),
    /// not the interface's direct monitoring.
    pub(crate) through_app: bool,
    pub(crate) buffer: Option<u32>,
    /// The dropout check: when it started, and the count then.
    checking: Option<(Instant, u64)>,
    /// Input channel listened to, from 0.
    pub(crate) channel: usize,
    /// The input tap was sent for this channel.
    tapped: Option<usize>,
    samples: Vec<f32>,
    peak: f32,
    peak_at: Option<Instant>,
    /// The level heard on the input step.
    pub(crate) heard: Option<Level>,
    /// Problems on the step showing.
    pub(crate) problems: Vec<Problem>,
    /// Written once the guide has been seen, so it opens by itself only
    /// the first time.
    seen: Option<PathBuf>,
}

impl AudioSetup {
    pub(crate) fn new(seen: Option<PathBuf>) -> AudioSetup {
        AudioSetup {
            through_app: true,
            seen,
            ..AudioSetup::default()
        }
    }

    /// Whether to open by itself: never seen, with no device settings.
    pub(crate) fn first_run(&self, saved_any: bool) -> bool {
        !saved_any && self.seen.as_ref().is_some_and(|p| !p.exists())
    }

    fn mark_seen(&self) {
        if let Some(p) = &self.seen {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, "seen\n");
        }
    }

    /// Takes the drivers and devices found; `current` is the driver and
    /// device in use, picked if it is among them.
    pub(crate) fn offer(
        &mut self,
        hosts: &[HostInfo],
        inputs: impl Fn(&str) -> Vec<String>,
        current: Option<(&str, &str)>,
    ) {
        self.cands = setup::candidates(hosts, inputs);
        self.drivers = setup::driver_problems(hosts, cfg!(windows));
        self.pick = current
            .and_then(|(h, d)| (self.cands.iter()).position(|c| c.host == h && c.device.name == d))
            .unwrap_or(0);
    }

    fn picked(&self) -> Option<&Candidate> {
        self.cands.get(self.pick)
    }

    /// The input tap the guide wants: the channel listened to on the
    /// input step.
    pub(crate) fn tap(&self) -> Option<usize> {
        (self.open && self.step == Step::Input).then_some(self.channel)
    }

    /// Reads the input tap and judges the level.
    fn listen(&mut self, ctl: &mut daw_engine::engine::Controller, rate: u32) {
        self.samples.clear();
        ctl.take_tuner_samples(&mut self.samples, rate as usize);
        let now_peak = self.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        self.hear(now_peak, Instant::now());
    }

    /// Takes the newest peak, holding the highest for a moment.
    pub(crate) fn hear(&mut self, peak: f32, now: Instant) {
        let old = self.peak_at.is_none_or(|t| now.duration_since(t) > HOLD);
        if old || peak >= self.peak {
            self.peak = peak;
            self.peak_at = Some(now);
        }
        self.heard = Some(Level::of(self.peak));
    }
}

impl App {
    /// Runs one of the guided setup's commands.
    pub(crate) fn audio_setup_command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::AudioSetup => {
                self.look_for_devices();
                let s = &mut self.audio_setup;
                s.open = true;
                s.problems.clear();
                s.checking = None;
                self.setup_go(Step::Device);
            }
            Cmd::AudioSetupNext => self.setup_next(),
            Cmd::AudioSetupSkip => {
                let next = self.audio_setup.step.next();
                if self.audio_setup.step == Step::Done {
                    self.setup_close();
                } else {
                    self.setup_go(next);
                }
            }
            Cmd::AudioSetupBack => {
                let back = self.audio_setup.step.back();
                self.setup_go(back);
            }
            Cmd::AudioSetupCancel => self.setup_close(),
            _ => {}
        }
    }

    /// Lists the drivers and devices again and offers them.
    fn look_for_devices(&mut self) {
        self.hosts = audio::list();
        let current = self
            .profile
            .as_ref()
            .map(|p| (p.host.clone(), p.device.clone()));
        let hosts = self.hosts.clone();
        self.audio_setup.offer(
            &hosts,
            crate::input_names,
            current.as_ref().map(|(h, d)| (h.as_str(), d.as_str())),
        );
    }

    fn setup_close(&mut self) {
        let s = &mut self.audio_setup;
        s.open = false;
        s.checking = None;
        s.mark_seen();
        self.send_input_tap();
    }

    /// Moves to `step`, starting or stopping the input meter.
    fn setup_go(&mut self, step: Step) {
        let s = &mut self.audio_setup;
        s.step = step;
        s.problems.clear();
        s.checking = None;
        if step == Step::Buffer && s.buffer.is_none() {
            let range = s.picked().and_then(|c| c.device.buffer_range);
            s.buffer = Some(setup::buffer_for(range, s.through_app));
        }
        if step == Step::Input {
            s.peak = 0.0;
            s.peak_at = None;
            s.heard = None;
        }
        self.send_input_tap();
    }

    /// Sends the engine's input tap to whichever wants it: the guide's
    /// level meter, or else the tuner.
    pub(crate) fn send_input_tap(&mut self) {
        let tap = self.audio_setup.tap();
        self.audio_setup.tapped = tap;
        let c = match tap {
            Some(ch) => Command::SetTunerInput(Some(ch)),
            None => self.tuner.command(),
        };
        self.send(c);
    }

    /// What the step's main button does.
    fn setup_next(&mut self) {
        match self.audio_setup.step {
            Step::Device => {
                let s = &self.audio_setup;
                let Some(c) = s.picked().cloned() else {
                    self.audio_setup.problems = vec![Problem::NoDevices];
                    return;
                };
                let before = self.saved.for_device(&c.host, &c.device.name);
                let p = setup::profile_for(&c, before, s.through_app);
                self.audio_setup.buffer = p.buffer;
                self.input_devices = crate::input_names(&c.host);
                self.draft = Some(p);
                self.apply_draft();
                if let Some(why) = self.device_trouble() {
                    self.audio_setup.problems = vec![why];
                } else {
                    self.setup_go(Step::Monitoring);
                }
            }
            Step::Monitoring => {
                let through = self.audio_setup.through_app;
                let range = self
                    .audio_setup
                    .picked()
                    .and_then(|c| c.device.buffer_range);
                self.audio_setup.buffer = Some(setup::buffer_for(range, through));
                if let Some(d) = &mut self.draft {
                    d.direct_monitoring = !through;
                }
                self.setup_go(Step::Buffer);
            }
            Step::Buffer => {
                if let Some(d) = &mut self.draft {
                    d.buffer = self.audio_setup.buffer;
                }
                self.apply_draft();
                if let Some(why) = self.device_trouble() {
                    self.audio_setup.problems = vec![why];
                    return;
                }
                let count = self.dropouts();
                let s = &mut self.audio_setup;
                s.problems.clear();
                s.checking = Some((Instant::now(), count));
            }
            Step::Input | Step::Latency => {
                let next = self.audio_setup.step.next();
                self.setup_go(next);
            }
            Step::Done => self.setup_close(),
        }
    }

    /// Why the device isn't running, if it isn't.
    fn device_trouble(&self) -> Option<Problem> {
        match &self.audio {
            Audio::Running(r) if r.status.failed.load(Ordering::Relaxed) => Some(Problem::Stopped),
            Audio::Running(_) => None,
            Audio::Silent(_, why) => Some(Problem::WontOpen(why.clone())),
            Audio::Stopped => Some(Problem::Stopped),
        }
    }

    /// Times the sound broke up: the callback ran late, or input came
    /// short.
    fn dropouts(&self) -> u64 {
        match &self.audio {
            Audio::Running(r) | Audio::Silent(r, _) => {
                r.status.overloads.load(Ordering::Relaxed)
                    + r.status.input_short.load(Ordering::Relaxed)
            }
            Audio::Stopped => 0,
        }
    }

    /// The delay from playing to hearing yourself through the app.
    fn monitoring_ms(&self) -> Option<f64> {
        let Audio::Running(r) = &self.audio else {
            return None;
        };
        let s = &r.status;
        let frames = s.last_frames.load(Ordering::Relaxed) as u32;
        let buffer = self.audio_setup.buffer.unwrap_or(frames.max(1));
        let reported = (
            s.output_latency_us.load(Ordering::Relaxed),
            s.input_latency_us.load(Ordering::Relaxed),
        );
        Some(setup::monitoring_ms(r.rate, buffer, reported))
    }

    /// What is still wrong, for the last step.
    pub(crate) fn setup_summary(&self) -> Vec<Problem> {
        let s = &self.audio_setup;
        let mut out = s.drivers.clone();
        if let Some(p) = self.device_trouble() {
            out.push(p);
        }
        if self.profile.as_ref().is_some_and(|p| p.input.is_none()) {
            out.push(Problem::NoInput);
        }
        if s.through_app
            && let Some(ms) = self.monitoring_ms()
            && ms > setup::FEEL_MS
        {
            out.push(Problem::LateMonitoring(ms));
        }
        if let Some(l) = s.heard.filter(|l| *l != Level::Good) {
            out.push(Problem::Level(l));
        }
        if let Some(Ok(m)) = self.latency.last() {
            if !m.stable() {
                out.push(Problem::UnsteadyLatency);
            }
            if m.inverted() {
                out.push(Problem::Inverted);
            }
        }
        out
    }

    /// Each frame: the dropout check, the input meter and the window.
    pub(crate) fn audio_setup_update(&mut self, ctx: &egui::Context) {
        if !self.audio_setup.open {
            return;
        }
        if let Some((from, count)) = self.audio_setup.checking
            && from.elapsed() >= CHECK
        {
            let broke = self.dropouts().saturating_sub(count);
            let trouble = self.device_trouble();
            let s = &mut self.audio_setup;
            s.checking = None;
            if let Some(why) = trouble {
                s.problems = vec![why];
            } else if broke > 0 {
                let range = s.picked().and_then(|c| c.device.buffer_range);
                s.problems = vec![Problem::Dropouts(broke)];
                if let Some(b) = setup::bigger_buffer(s.buffer, range) {
                    s.buffer = Some(b);
                }
            } else {
                self.setup_go(Step::Input);
            }
        }
        if self.audio_setup.step == Step::Input {
            if self.audio_setup.tapped != self.audio_setup.tap() {
                self.send_input_tap();
            }
            let rate = self.rate;
            self.audio_setup.listen(&mut self.ctl, rate);
        }
        let inputs = self.input_count();
        let monitoring = self.monitoring_ms();
        let summary = (self.audio_setup.step == Step::Done).then(|| self.setup_summary());
        let measured = self.draft.as_ref().and_then(|d| {
            let at = d.measured_at?;
            (at == (d.rate, d.buffer)).then(|| (d.record_offset.unwrap_or(0), d.rate))
        });
        let measuring = self.latency.busy();
        let profile = self.profile.clone();
        let mut cmd = None;
        let mut open = true;
        egui::Window::new("Guided audio setup")
            .open(&mut open)
            .resizable(false)
            .default_width(480.0)
            .show(ctx, |ui| {
                let s = &mut self.audio_setup;
                ui.label(
                    RichText::new(format!("Step {} of {}", s.step.index() + 1, STEPS.len())).weak(),
                );
                ui.heading(s.step.title());
                let view = View {
                    inputs,
                    monitoring,
                    measured,
                    measuring,
                    profile: profile.as_ref(),
                    summary: summary.as_deref(),
                };
                cmd = s.step_ui(ui, &view);
                for p in &s.problems {
                    ui.colored_label(ui.visuals().warn_fg_color, p.explain());
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        // ai: audio-setup-back
                        .add_enabled(s.step != Step::Device, egui::Button::new("Back"))
                        .clicked()
                    {
                        cmd = Some(Cmd::AudioSetupBack);
                    }
                    let next = match s.step {
                        Step::Device => "Use this",
                        Step::Buffer => "Try it",
                        Step::Done => "Finish",
                        _ => "Next",
                    };
                    if ui
                        // ai: audio-setup-next
                        .add_enabled(s.checking.is_none(), egui::Button::new(next))
                        .clicked()
                    {
                        cmd = Some(Cmd::AudioSetupNext);
                    }
                    if !matches!(s.step, Step::Done | Step::Monitoring)
                        && ui
                            // ai: audio-setup-skip
                            .button("Skip")
                            .on_hover_text("Go on without changing this")
                            .clicked()
                    {
                        cmd = Some(Cmd::AudioSetupSkip);
                    }
                    // ai: audio-setup-cancel
                    if ui.button("Cancel").clicked() {
                        cmd = Some(Cmd::AudioSetupCancel);
                    }
                });
            });
        if !open {
            cmd = Some(Cmd::AudioSetupCancel);
        }
        if self.audio_setup.checking.is_some() || self.audio_setup.step == Step::Input {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if let Some(c) = cmd {
            self.run(ctx, c);
        }
    }
}

/// What the window shows from the rest of the app.
struct View<'a> {
    inputs: usize,
    monitoring: Option<f64>,
    /// The recording offset measured for these settings, and the rate.
    measured: Option<(u32, u32)>,
    measuring: bool,
    profile: Option<&'a daw_audio_io::DeviceProfile>,
    summary: Option<&'a [Problem]>,
}

impl AudioSetup {
    /// The step's own part of the window. Gives a command a button
    /// asked for.
    fn step_ui(&mut self, ui: &mut egui::Ui, v: &View) -> Option<Cmd> {
        let mut cmd = None;
        match self.step {
            Step::Device => {
                ui.label(
                    "Choose the interface your guitar or microphone plugs into. The one at the top \
                     is the best match found.",
                );
                for p in &self.drivers {
                    ui.colored_label(ui.visuals().warn_fg_color, p.explain());
                }
                ui.add_space(4.0);
                for (i, c) in self.cands.iter().enumerate() {
                    let text = format!(
                        "{}  ({}, {} in, {} out)",
                        c.device.name, c.host, c.device.input_channels, c.device.output_channels
                    );
                    // ai: audio_setup.device
                    ui.radio_value(&mut self.pick, i, text);
                    ui.indent(("why", i), |ui| {
                        ui.label(RichText::new(&c.why).small().weak());
                    });
                }
                // ai: audio-setup
                if ui.button("Look again").clicked() {
                    cmd = Some(Cmd::AudioSetup);
                }
            }
            Step::Monitoring => {
                ui.label(
                    "When you play, you hear yourself one of two ways. Pick the one you'll use.",
                );
                ui.add_space(4.0);
                // ai: audio_setup.through_app
                ui.radio_value(&mut self.through_app, true, "Through the app");
                ui.indent("app", |ui| {
                    ui.label(
                        RichText::new(
                            "You hear the app's effects, such as the amp, as you play. Needs a small \
                             buffer so the sound isn't late. Turn the interface's direct monitor off, \
                             or you hear yourself twice.",
                        )
                        .small(),
                    );
                });
                // ai: audio_setup.through_app
                ui.radio_value(
                    &mut self.through_app,
                    false,
                    "Through the interface (direct monitoring)",
                );
                ui.indent("direct", |ui| {
                    ui.label(
                        RichText::new(
                            "The interface sends what you play straight to your headphones, with no \
                             delay at all, but without the app's effects. The app won't play your \
                             input back, so you don't hear it twice. Turn the interface's direct \
                             monitor switch or mix knob towards the input.",
                        )
                        .small(),
                    );
                });
            }
            Step::Buffer => {
                ui.label(
                    "Sound is worked on a small batch at a time. Smaller batches reach your ears \
                     sooner, but the computer must keep up or the sound breaks up into clicks. \
                     Press Try it: the app listens for a few seconds.",
                );
                ui.add_space(4.0);
                let range = self.picked().and_then(|c| c.device.buffer_range);
                let rate = v.profile.map_or(setup::RATE, |p| p.rate);
                let suggested = setup::buffer_for(range, self.through_app);
                let ok = |b: u32| range.is_none_or(|(lo, hi)| (lo..=hi).contains(&b));
                ui.add_enabled_ui(self.checking.is_none(), |ui| {
                    for b in BUFFERS.into_iter().filter(|&b| ok(b)) {
                        let mut text =
                            format!("{b} samples ({:.1} ms)", b as f64 * 1000.0 / rate as f64);
                        if b == suggested {
                            text += "  (suggested)";
                        }
                        // ai: audio_setup.buffer
                        ui.radio_value(&mut self.buffer, Some(b), text);
                    }
                });
                if let Some((from, _)) = self.checking {
                    let left = CHECK.saturating_sub(from.elapsed()).as_secs_f32();
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Listening for dropouts… {left:.0} s"));
                    });
                } else if self.through_app
                    && let Some(ms) = v.monitoring
                {
                    ui.label(format!(
                        "Right now you'd hear yourself about {ms:.1} ms after you play."
                    ));
                }
            }
            Step::Input => {
                if v.inputs == 0 {
                    ui.colored_label(ui.visuals().warn_fg_color, Problem::NoInput.explain());
                    return None;
                }
                ui.label(
                    "Plug in and choose the input. Then play as loud as you will when recording, \
                     and set the interface's gain knob by what this says.",
                );
                self.channel = self.channel.min(v.inputs - 1);
                ui.horizontal(|ui| {
                    let l = ui.label("Input");
                    egui::ComboBox::from_id_salt("setup-input")
                        .selected_text(format!("Input {}", self.channel + 1))
                        .show_ui(ui, |ui| {
                            for c in 0..v.inputs {
                                // ai: audio_setup.input
                                ui.selectable_value(
                                    &mut self.channel,
                                    c,
                                    format!("Input {}", c + 1),
                                );
                            }
                        })
                        .response
                        .labelled_by(l.id);
                });
                let db = setup::to_db(self.peak);
                // -60 dB to 0 dB across the bar.
                let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                let level = self.heard.unwrap_or(Level::Silent);
                let colour = match level {
                    Level::Good => egui::Color32::from_rgb(60, 170, 80),
                    Level::Hot | Level::Clipping => egui::Color32::from_rgb(210, 60, 50),
                    _ => ui.visuals().widgets.inactive.bg_fill,
                };
                ui.add(
                    egui::ProgressBar::new(fill)
                        .fill(colour)
                        .text(if self.peak > 0.0 {
                            format!("{db:.0} dB")
                        } else {
                            "nothing".into()
                        }),
                )
                .accessible_name("Input level");
                ui.label(level.advice());
            }
            Step::Latency => {
                ui.label(
                    "Recordings arrive a little late, as the sound goes out and back in through the \
                     interface. Measuring that once lines every recording up exactly. You need a \
                     short cable from an output of the interface to an input.",
                );
                ui.add_space(4.0);
                match v.measured {
                    Some((frames, rate)) => {
                        ui.label(format!(
                            "Measured for these settings: {frames} frames ({:.2} ms).",
                            frames as f64 * 1000.0 / rate as f64
                        ));
                    }
                    None if v.measuring => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Measuring…");
                        });
                    }
                    None => {
                        ui.label(
                            RichText::new(
                                "Not measured yet: recordings use the delay the driver reports, which \
                                 can be a few milliseconds out. You can measure later from Settings.",
                            )
                            .weak(),
                        );
                    }
                }
                // ai: measure-latency
                if ui.button("Measure latency…").clicked() {
                    cmd = Some(Cmd::MeasureLatency);
                }
            }
            Step::Done => {
                if let Some(p) = v.profile {
                    egui::Grid::new("setup-summary")
                        .num_columns(2)
                        .show(ui, |ui| {
                            let rows = [
                                ("Interface", p.device.clone()),
                                ("Driver", p.host.clone()),
                                ("Sample rate", format!("{} Hz", p.rate)),
                                (
                                    "Buffer",
                                    p.buffer.map_or("driver default".into(), |b| {
                                        format!("{b} samples")
                                    }),
                                ),
                                ("Records from", p.input.clone().unwrap_or("nothing".into())),
                                (
                                    "You listen",
                                    if p.direct_monitoring {
                                        "through the interface".into()
                                    } else {
                                        "through the app".into()
                                    },
                                ),
                                (
                                    "Recording delay",
                                    match p.record_offset {
                                        Some(o) if p.measured_at.is_some() => {
                                            format!("{o} frames, measured")
                                        }
                                        Some(o) => format!("{o} frames, typed in"),
                                        None => "as the driver reports".into(),
                                    },
                                ),
                            ];
                            for (k, val) in rows {
                                ui.label(k);
                                ui.label(val);
                                ui.end_row();
                            }
                        });
                } else {
                    ui.label("No device is set up yet.");
                }
                match v.summary {
                    Some([]) | None => {
                        ui.label("Nothing else needs fixing. These settings are saved for this interface.");
                    }
                    Some(ps) => {
                        ui.label("Still worth fixing:");
                        for p in ps {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!("• {}", p.explain()),
                            );
                        }
                    }
                }
                ui.label(
                    RichText::new("Run this again any time from Settings → Guided audio setup….")
                        .small()
                        .weak(),
                );
            }
        }
        cmd
    }
}

pub(crate) const TOOLS: &[crate::agent::Tool] = &[crate::agent::Tool {
    name: "audio_setup",
    description: "The guided audio setup (command audio-setup opens it; audio-setup-next, \
                  -skip, -back and -cancel move through it): its step, the interfaces found \
                  (best first) and the one picked, whether the player listens through the app \
                  or the interface, the buffer size, the input listened to and how loud it \
                  is, and any problems with what to do. device picks an interface (its \
                  number in the list, from 0), through_app how they listen, buffer the size \
                  to try, input the input to listen to (from 1). Only what is given changes.",
    access: crate::agent::Access::Edit,
    schema: || {
        serde_json::json!({"type": "object", "properties": {
            "device": {"type": "integer"},
            "through_app": {"type": "boolean"},
            "buffer": {"type": "integer"},
            "input": {"type": "integer"}
        }})
    },
}];

impl App {
    pub(crate) fn setup_tool(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<Result<(serde_json::Value, Option<String>), String>> {
        (name == "audio_setup").then(|| self.setup_tool_run(args).map(|v| (v, None)))
    }

    fn setup_tool_run(&mut self, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        use serde_json::json;
        let inputs = self.input_count();
        let s = &mut self.audio_setup;
        let range = |s: &AudioSetup| s.picked().and_then(|c| c.device.buffer_range);
        // Check everything before changing anything.
        if let Some(d) = args["device"].as_u64()
            && d as usize >= s.cands.len()
        {
            return Err(format!("{} interfaces were found, from 0.", s.cands.len()));
        }
        if let Some(b) = args["buffer"].as_u64() {
            let b = b as u32;
            let ok = range(s).is_none_or(|(lo, hi)| (lo..=hi).contains(&b));
            if !BUFFERS.contains(&b) || !ok {
                return Err(format!(
                    "{b} samples isn't a buffer size the interface takes."
                ));
            }
        }
        if let Some(c) = args["input"].as_u64()
            && !(1..=inputs as u64).contains(&c)
        {
            return Err(format!("The interface has inputs 1 to {inputs}."));
        }
        if let Some(d) = args["device"].as_u64() {
            s.pick = d as usize;
        }
        if let Some(t) = args["through_app"].as_bool() {
            s.through_app = t;
        }
        if let Some(b) = args["buffer"].as_u64() {
            s.buffer = Some(b as u32);
        }
        if let Some(c) = args["input"].as_u64() {
            s.channel = c as usize - 1;
        }
        let r = range(s);
        let devices: Vec<serde_json::Value> = (s.cands.iter())
            .map(|c| {
                json!({"name": c.device.name, "driver": c.host, "inputs": c.device.input_channels,
                       "outputs": c.device.output_channels, "why": c.why})
            })
            .collect();
        let buffers: Vec<u32> = (BUFFERS.into_iter())
            .filter(|&b| r.is_none_or(|(lo, hi)| (lo..=hi).contains(&b)))
            .collect();
        Ok(json!({
            "open": s.open,
            "step": s.step.index() + 1,
            "of": STEPS.len(),
            "title": s.step.title(),
            "devices": devices,
            "device": s.pick,
            "through_app": s.through_app,
            "buffer": s.buffer,
            "buffers": buffers,
            "suggested_buffer": setup::buffer_for(r, s.through_app),
            "checking_buffer": s.checking.is_some(),
            "input": s.channel + 1,
            "inputs": inputs,
            "level": s.heard.map(|l| format!("{l:?}").to_lowercase()),
            "advice": s.heard.map(Level::advice),
            "problems": s.problems.iter().map(Problem::explain).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_audio_io::DeviceInfo;

    fn hosts() -> Vec<HostInfo> {
        vec![
            HostInfo {
                name: "WASAPI".into(),
                devices: vec![DeviceInfo {
                    name: "Speakers (Laptop)".into(),
                    output_channels: 2,
                    input_channels: 0,
                    default_rate: Some(48_000),
                    rates: vec![48_000],
                    buffer_range: None,
                }],
            },
            HostInfo {
                name: "ASIO".into(),
                devices: vec![DeviceInfo {
                    name: "Interface ASIO".into(),
                    output_channels: 10,
                    input_channels: 18,
                    default_rate: Some(44_100),
                    rates: vec![44_100, 48_000],
                    buffer_range: Some((32, 1024)),
                }],
            },
        ]
    }

    #[test]
    fn the_steps_go_in_order_and_stop_at_the_ends() {
        assert_eq!(Step::Device.back(), Step::Device);
        assert_eq!(Step::Device.next(), Step::Monitoring);
        assert_eq!(Step::Done.next(), Step::Done);
    }

    #[test]
    fn the_interface_in_use_is_picked_if_found() {
        let mut s = AudioSetup::new(None);
        s.offer(&hosts(), |_| vec!["Interface ASIO".into()], None);
        assert_eq!(s.picked().unwrap().host, "ASIO");
        s.offer(
            &hosts(),
            |_| Vec::new(),
            Some(("WASAPI", "Speakers (Laptop)")),
        );
        assert_eq!(s.picked().unwrap().host, "WASAPI");
    }

    #[test]
    fn the_meter_holds_its_peak_for_a_moment() {
        let mut s = AudioSetup::new(None);
        let t = Instant::now();
        s.hear(0.9, t);
        s.hear(0.01, t + Duration::from_millis(500));
        assert_eq!(s.heard, Some(Level::Hot));
        s.hear(0.2, t + HOLD + Duration::from_millis(1));
        assert_eq!(s.heard, Some(Level::Good));
    }

    #[test]
    fn it_opens_by_itself_only_the_first_time() {
        let path =
            std::env::temp_dir().join(format!("daw-audio-setup-seen-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let s = AudioSetup::new(Some(path.clone()));
        assert!(s.first_run(false));
        assert!(!s.first_run(true));
        s.mark_seen();
        assert!(!s.first_run(false));
        let _ = std::fs::remove_file(&path);
        assert!(!AudioSetup::new(None).first_run(false));
    }

    /// End to end, as an assistant would: every step through the
    /// command table, taking what each offers.
    #[test]
    fn an_assistant_walks_the_guide_by_commands() {
        let mut d = crate::agent::driver::Driver::new();
        d.command("audio-setup");
        assert!(d.app().audio_setup.open);
        assert_eq!(d.app().audio_setup.step, Step::Device);
        for (id, then) in [
            ("audio-setup-skip", Step::Monitoring),
            ("audio-setup-next", Step::Buffer),
            ("audio-setup-skip", Step::Input),
            ("audio-setup-back", Step::Buffer),
            ("audio-setup-skip", Step::Input),
            ("audio-setup-next", Step::Latency),
            ("audio-setup-skip", Step::Done),
        ] {
            d.command(id);
            assert_eq!(d.app().audio_setup.step, then, "after {id}");
        }
        d.command("measure-latency");
        assert!(d.app().latency.open);
        d.command("audio-setup-next");
        assert!(!d.app().audio_setup.open);
        // With the guide closed its steps are refused.
        assert!(
            d.call("run_command", serde_json::json!({"id": "audio-setup-next"}))
                .is_err()
        );
    }

    mod ui {
        use super::*;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        #[test]
        fn the_guide_is_walked_through_its_commands() {
            let mut h = Harness::builder()
                .with_size([1200.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let ctx = h.ctx.clone();
            // The steps can't run with the guide closed.
            h.state_mut().run(&ctx, Cmd::AudioSetupNext);
            assert!(!h.state().audio_setup.open);
            h.state_mut().run(&ctx, Cmd::AudioSetup);
            h.state_mut()
                .audio_setup
                .offer(&hosts(), |_| vec!["Interface ASIO".into()], None);
            h.run();
            assert!(h.query_by_label("Your audio interface").is_some());
            assert!(h.query_by_label_contains("Interface ASIO").is_some());
            // No real interface here: it won't open, and says why.
            h.state_mut().run(&ctx, Cmd::AudioSetupNext);
            h.run();
            let app = h.state();
            assert_eq!(app.audio_setup.step, Step::Device);
            assert!(matches!(
                app.audio_setup.problems[..],
                [Problem::WontOpen(_)]
            ));
            let d = app.draft.as_ref().unwrap();
            assert_eq!(
                (d.host.as_str(), d.rate, d.buffer),
                ("ASIO", 48_000, Some(128))
            );
            assert!(h.query_by_label_contains("wouldn't start").is_some());
            h.state_mut().run(&ctx, Cmd::AudioSetupSkip);
            h.run();
            assert!(h.query_by_label("How you listen while playing").is_some());
            h.state_mut().audio_setup.through_app = false;
            h.state_mut().run(&ctx, Cmd::AudioSetupNext);
            h.run();
            let app = h.state();
            assert_eq!(app.audio_setup.step, Step::Buffer);
            assert_eq!(app.audio_setup.buffer, Some(256));
            assert!(app.draft.as_ref().unwrap().direct_monitoring);
            assert!(h.query_by_label_contains("256 samples").is_some());
            h.state_mut().run(&ctx, Cmd::AudioSetupSkip);
            h.run();
            assert_eq!(h.state().audio_setup.step, Step::Input);
            assert_eq!(h.state().audio_setup.tap(), Some(0));
            h.state_mut().run(&ctx, Cmd::AudioSetupNext);
            h.run();
            assert!(h.query_by_label("Recording delay").is_some());
            h.state_mut().run(&ctx, Cmd::MeasureLatency);
            assert!(h.state().latency.open);
            h.state_mut().run(&ctx, Cmd::AudioSetupNext);
            h.run();
            assert!(h.query_by_label("All set").is_some());
            assert!(h.query_by_label_contains("Still worth fixing").is_some());
            h.state_mut().run(&ctx, Cmd::AudioSetupBack);
            assert_eq!(h.state().audio_setup.step, Step::Latency);
            h.state_mut().run(&ctx, Cmd::AudioSetupSkip);
            h.state_mut().run(&ctx, Cmd::AudioSetupNext);
            h.run();
            assert!(!h.state().audio_setup.open);
            assert_eq!(h.state().audio_setup.tap(), None);
        }
    }

    #[test]
    fn an_assistant_steps_through_the_guided_setup() {
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        d.command("audio-setup");
        // As if these were found.
        let hosts = hosts();
        d.app()
            .audio_setup
            .offer(&hosts, |_| vec!["Interface ASIO".into()], None);
        let r = d.call("audio_setup", json!({})).unwrap();
        assert_eq!(
            (r["open"].clone(), r["step"].clone()),
            (json!(true), json!(1)),
            "{r}"
        );
        assert_eq!(r["devices"][0]["name"], "Interface ASIO");
        assert!(d.call("audio_setup", json!({"device": 9})).is_err());
        let r = d.call("audio_setup", json!({"device": 1})).unwrap();
        assert_eq!(r["device"], 1);
        d.call("audio_setup", json!({"device": 0})).unwrap();
        assert!(
            d.call("audio_setup", json!({"buffer": 2048})).is_err(),
            "over its range"
        );
        let r = d
            .call("audio_setup", json!({"through_app": false, "buffer": 512}))
            .unwrap();
        assert_eq!(
            (r["through_app"].clone(), r["buffer"].clone()),
            (json!(false), json!(512))
        );
        assert!(!r["buffers"].as_array().unwrap().contains(&json!(2048)));
        assert!(d.call("audio_setup", json!({"input": 99})).is_err());
        // The test interface can't open: the step says so.
        d.command("audio-setup-next");
        let r = d.call("audio_setup", json!({})).unwrap();
        assert!(!r["problems"].as_array().unwrap().is_empty(), "{r}");
        d.command("audio-setup-cancel");
        assert_eq!(d.call("audio_setup", json!({})).unwrap()["open"], false);
    }
}
