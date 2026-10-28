// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings → Audio device → Measure: the recording offset measured with
//! a loopback cable (roadmap 1.1 and 1.2), using
//! [`daw_audio_io::latency`]. The app's own audio stops while it runs,
//! since the measurement needs the device to itself.

use daw_audio_io::DeviceProfile;
use daw_audio_io::latency::{self, Measured, Measuring, Target};
use eframe::egui::{self, Color32, RichText};

#[derive(Default)]
pub struct Latency {
    pub open: bool,
    /// Input channel recorded from, from 0.
    channel: usize,
    /// Input channels on the chosen input device.
    pub inputs: usize,
    measuring: Option<Measuring>,
    last: Option<Result<Measured, String>>,
}

/// What the window asks the app to do.
pub enum Action {
    /// Stop the app's audio and measure.
    Start,
    /// Stop measuring and start the app's audio again.
    Cancel,
}

impl Latency {
    pub fn busy(&self) -> bool {
        self.measuring.is_some()
    }

    /// Starts measuring on `target`; the app's audio must be stopped.
    /// Returns false if it could not start (the reason is shown).
    pub fn begin(&mut self, target: Target) -> bool {
        self.last = None;
        match Measuring::start(target, self.channel, latency::RUNS) {
            Ok(m) => {
                self.measuring = Some(m);
                true
            }
            Err(e) => {
                self.last = Some(Err(e.to_string()));
                false
            }
        }
    }

    pub fn cancel(&mut self) {
        self.measuring = None;
    }

    /// Moves a measurement on; returns its result once, when it ends.
    /// The device is free again by then.
    pub fn poll(&mut self) -> Option<Result<Measured, String>> {
        let done = self.measuring.as_mut()?.poll()?;
        self.measuring = None;
        let done = done.map_err(|e| e.to_string());
        self.last = Some(done.clone());
        Some(done)
    }

    pub fn window(
        &mut self,
        ctx: &egui::Context,
        draft: &DeviceProfile,
        recording: bool,
    ) -> Option<Action> {
        let mut action = None;
        let mut open = self.open;
        egui::Window::new("Measure latency")
            .open(&mut open)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Connect a cable from main output {} (left) to an input on the interface, and choose that input. \
                     Turn speakers and headphones down first: the test plays a few short chirps. \
                     Turn the interface's direct monitoring off, or the chirps go round again.",
                    draft.main.0 + 1
                ));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Input");
                    let inputs = self.inputs.max(1);
                    self.channel = self.channel.min(inputs - 1);
                    ui.add_enabled_ui(!self.busy(), |ui| {
                        egui::ComboBox::from_id_salt("latency-input")
                            .selected_text(format!("Input {}", self.channel + 1))
                            .show_ui(ui, |ui| {
                                for c in 0..inputs {
                                    ui.selectable_value(&mut self.channel, c, format!("Input {}", c + 1));
                                }
                            })
                            .response
                            .accessible_name("Loopback input");
                    });
                });
                ui.label(
                    RichText::new(format!(
                        "Measures at {} Hz, {}, the settings in the Audio device window. Measure again after changing them.",
                        draft.rate,
                        draft.buffer.map_or("the driver's buffer size".into(), |b| format!("{b} samples"))
                    ))
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                match &self.measuring {
                    Some(m) => {
                        ui.add(egui::ProgressBar::new(m.progress()).text("Measuring…"));
                        if ui.button("Cancel").clicked() {
                            action = Some(Action::Cancel);
                        }
                    }
                    None => {
                        let can = !recording && draft.input.is_some();
                        let b = ui
                            .add_enabled(can, egui::Button::new("Start measuring"))
                            .on_disabled_hover_text(if recording {
                                "Stop recording first."
                            } else {
                                "Choose an input device first."
                            });
                        if b.clicked() {
                            action = Some(Action::Start);
                        }
                    }
                }
                match &self.last {
                    Some(Ok(m)) => {
                        ui.separator();
                        ui.label(format!(
                            "Round trip: {} frames ({:.2} ms). Saved as this device's recording offset.",
                            m.frames,
                            m.ms()
                        ));
                        if m.stable() {
                            ui.label("The same every time the audio started, so recordings line up exactly.");
                        } else {
                            ui.colored_label(
                                Color32::from_rgb(230, 180, 60),
                                format!(
                                    "It changed by {:.0} frames ({:.1} ms) between starts, so recordings may be off by up to that much. \
                                     This driver may not start the same way each time; try again, or another buffer size.",
                                    m.spread,
                                    m.spread * 1000.0 / m.rate as f64
                                ),
                            );
                        }
                        if m.inverted() {
                            ui.label(
                                RichText::new("The interface flips the signal's polarity on the way through; that does not affect timing.")
                                    .small()
                                    .weak(),
                            );
                        }
                    }
                    Some(Err(e)) => {
                        ui.separator();
                        ui.colored_label(Color32::LIGHT_RED, e);
                    }
                    None => {}
                }
            });
        if !open && self.busy() {
            action = Some(Action::Cancel);
        }
        self.open = open;
        action
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use daw_audio_io::SilentInput;
    use daw_audio_io::latency::Target;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use crate::{App, Audio};

    fn harness() -> Harness<'static, App> {
        Harness::builder()
            .with_size([1200.0, 800.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()))
    }

    /// A measurement through a simulated cable sets the draft's offset,
    /// applies it, and starts the app's audio again.
    #[test]
    fn measuring_saves_the_offset() {
        let mut h = harness();
        {
            let app = h.state_mut();
            app.draft = Some(daw_audio_io::DeviceProfile {
                host: "Test".into(),
                device: "Loopback".into(),
                rate: 48_000,
                buffer: Some(128),
                main: (0, 1),
                click: None,
                input: Some("Loopback".into()),
                record_offset: None,
                measured_at: None,
                direct_monitoring: false,
            });
            app.latency.open = true;
            app.latency.inputs = 2;
        }
        h.run();
        h.get_by_label("Start measuring");
        h.state_mut().start_measuring(Ok(Target::Silent {
            rate: 48_000,
            buffer: 128,
            input: SilentInput::Loopback { delay: 411 },
        }));
        assert!(matches!(h.state().audio, Audio::Stopped));
        for _ in 0..600 {
            h.step();
            if !h.state().latency.busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let app = h.state();
        assert!(!app.latency.busy(), "the measurement did not finish");
        let p = app
            .profile
            .as_ref()
            .expect("the measured profile is applied");
        assert_eq!(p.record_offset, Some(411));
        assert_eq!(p.measured_at, Some((48_000, Some(128))));
        assert!(!matches!(app.audio, Audio::Stopped), "audio started again");
        h.run();
        h.get_by_label_contains("Round trip: 411 frames");
    }
}
