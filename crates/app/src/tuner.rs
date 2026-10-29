// SPDX-License-Identifier: GPL-3.0-or-later
//! The chromatic tuner pane and the reference pitch (roadmap 1.4).

use daw_engine::engine::{Command, Controller};
use daw_engine::tuner::{Detector, Note, Smoother, window};
use eframe::egui::{self, Color32, RichText, Sense, Stroke, pos2, vec2};

pub struct Tuner {
    pub open: bool,
    /// Device input channel listened to.
    input: usize,
    /// The A above middle C, in Hz.
    pub a4: f32,
    buf: Vec<f32>,
    detector: Detector,
    smoother: Smoother,
    shown: Option<f32>,
}

/// In tune when this close, in cents.
const IN_TUNE: f32 = 3.0;

impl Tuner {
    pub fn new(rate: u32) -> Tuner {
        Tuner {
            open: false,
            input: 0,
            a4: 440.0,
            buf: Vec::new(),
            detector: Detector::new(rate),
            smoother: Smoother::default(),
            shown: None,
        }
    }

    /// What the engine should send the tuner now.
    pub fn command(&self) -> Command {
        Command::SetTunerInput(self.open.then_some(self.input))
    }

    /// Reads new input and updates the pitch shown. Call every frame.
    pub fn update(&mut self, ctl: &mut Controller, rate: u32) {
        if !self.open {
            return;
        }
        if self.detector.rate() != rate {
            self.detector = Detector::new(rate);
            self.buf.clear();
        }
        let w = window(rate);
        let before = self.buf.len();
        ctl.take_tuner_samples(&mut self.buf, w);
        if self.buf.len() == before && before == w {
            // No new audio (stopped device): keep the last reading.
            return;
        }
        let reading = (self.buf.len() >= w)
            .then(|| self.detector.detect(&self.buf))
            .flatten();
        self.shown = self.smoother.push(reading);
    }

    /// Opens or closes it. Returns the command to send if that changed.
    pub fn set_open(&mut self, open: bool) -> Option<Command> {
        if open == self.open {
            return None;
        }
        self.open = open;
        if !open {
            self.shown = None;
            self.buf.clear();
        }
        Some(self.command())
    }

    /// Draws the pane. Returns a command to send if the input changed.
    pub fn pane(&mut self, ui: &mut egui::Ui, inputs: usize) -> Option<Command> {
        let before = self.input;
        ui.horizontal(|ui| {
            ui.label("Input");
            if inputs == 0 {
                ui.label(RichText::new("none on this device").weak());
            } else {
                self.input = self.input.min(inputs - 1);
                egui::ComboBox::from_id_salt("tuner-input")
                    .width(70.0)
                    .selected_text(format!("In {}", self.input + 1))
                    .show_ui(ui, |ui| {
                        for c in 0..inputs {
                            ui.selectable_value(&mut self.input, c, format!("In {}", c + 1));
                        }
                    })
                    .response
                    .accessible_name("Tuner input");
            }
            ui.separator();
            ui.label("A =");
            ui.add(
                egui::DragValue::new(&mut self.a4)
                    .range(400.0..=480.0)
                    .speed(0.1)
                    .fixed_decimals(1)
                    .suffix(" Hz"),
            )
            .on_hover_text("Reference pitch");
        });
        ui.add_space(8.0);
        let note = self.shown.map(|hz| Note::from_hz(hz, self.a4));
        ui.vertical_centered(|ui| {
            let (text, color) = match note {
                Some(n) if n.cents.abs() <= IN_TUNE => (
                    format!("{}{}", n.name(), n.octave()),
                    Color32::from_rgb(80, 210, 110),
                ),
                Some(n) => (
                    format!("{}{}", n.name(), n.octave()),
                    ui.visuals().strong_text_color(),
                ),
                None => ("–".into(), ui.visuals().weak_text_color()),
            };
            ui.label(RichText::new(text).size(48.0).color(color));
        });
        needle(ui, note.map(|n| n.cents));
        ui.horizontal(|ui| match (self.shown, note) {
            (Some(hz), Some(n)) => {
                ui.label(format!("{hz:.2} Hz"));
                ui.label(RichText::new(format!("{:+.1} cents", n.cents)).weak());
            }
            _ => {
                ui.label(RichText::new("Play a single note").weak());
            }
        });
        (self.input != before).then(|| self.command())
    }
}

/// A meter from −50 to +50 cents with a needle.
fn needle(ui: &mut egui::Ui, cents: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, Color32::from_gray(30));
    let x = |c: f32| rect.left() + rect.width() * (c + 50.0) / 100.0;
    // The in-tune zone, and ticks every 10 cents.
    p.rect_filled(
        egui::Rect::from_x_y_ranges(x(-IN_TUNE)..=x(IN_TUNE), rect.y_range()),
        0.0,
        Color32::from_rgb(30, 70, 40),
    );
    for c in (-50..=50).step_by(10) {
        let h = if c == 0 { 14.0 } else { 7.0 };
        p.line_segment(
            [
                pos2(x(c as f32), rect.bottom()),
                pos2(x(c as f32), rect.bottom() - h),
            ],
            Stroke::new(1.0, Color32::from_gray(110)),
        );
    }
    if let Some(c) = cents {
        let color = if c.abs() <= IN_TUNE {
            Color32::from_rgb(80, 210, 110)
        } else {
            Color32::from_rgb(230, 200, 60)
        };
        let nx = x(c.clamp(-50.0, 50.0));
        p.line_segment(
            [pos2(nx, rect.top() + 2.0), pos2(nx, rect.bottom() - 2.0)],
            Stroke::new(3.0, color),
        );
    }
}
