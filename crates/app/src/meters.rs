// SPDX-License-Identifier: GPL-3.0-or-later
//! The meters window (roadmap 2.3): loudness, true peak, peak and RMS,
//! a spectrum and phase correlation for the master or one track; and the
//! peak and RMS meter on each mixer strip.

use daw_engine::engine::{Command, Controller};
use daw_engine::meter::{Analyser, Tap, bands};
use eframe::egui::{self, RichText, Sense, Stroke, pos2, vec2};

pub struct Meters {
    pub open: bool,
    /// The track measured, by key; `None` is the master.
    track: Option<String>,
    /// What the engine was last told to tap.
    sent: Option<Tap>,
    pub reset_on_play: bool,
    analyser: Analyser,
    buf: Vec<f32>,
    /// The spectrum drawn, per band, falling back slowly.
    shown: Vec<f32>,
    was_playing: bool,
    /// The engine's count of missed audio at the last reset.
    dropped_at_reset: u64,
    missed: bool,
}

/// Spectrum bands: a twelfth of an octave each, 20 Hz to 20 kHz.
const BANDS_PER_OCTAVE: f32 = 12.0;
const LOW_HZ: f32 = 20.0;
const HIGH_HZ: f32 = 20_000.0;
/// The spectrum's floor, in dB.
const FLOOR: f32 = -90.0;

fn edges() -> Vec<f32> {
    let n = (BANDS_PER_OCTAVE * (HIGH_HZ / LOW_HZ).log2()).ceil() as usize;
    (0..=n)
        .map(|i| LOW_HZ * 2f32.powf(i as f32 / BANDS_PER_OCTAVE))
        .collect()
}

impl Meters {
    pub fn new(rate: u32) -> Meters {
        Meters {
            open: false,
            track: None,
            sent: None,
            reset_on_play: true,
            analyser: Analyser::new(rate),
            buf: Vec::new(),
            shown: Vec::new(),
            was_playing: false,
            dropped_at_reset: 0,
            missed: false,
        }
    }

    /// Opens the window on a track (by key), or the master.
    pub fn show(&mut self, track: Option<String>) {
        self.open = true;
        if self.track != track {
            self.track = track;
            self.reset();
        }
    }

    /// The tap the engine should have now, given the tracks' keys in
    /// order. A deleted track falls back to the master.
    fn wanted(&mut self, keys: &[&str]) -> Option<Tap> {
        if !self.open {
            return None;
        }
        match &self.track {
            None => Some(Tap::Master),
            Some(k) => match keys.iter().position(|x| x == k) {
                Some(i) => Some(Tap::Track(i)),
                None => {
                    self.track = None;
                    self.reset();
                    Some(Tap::Master)
                }
            },
        }
    }

    /// What to send a new engine.
    pub fn command(&self) -> Command {
        Command::SetMeterTap(self.sent)
    }

    fn reset(&mut self) {
        self.analyser.reset();
        self.shown.clear();
        self.missed = false;
        self.dropped_at_reset = u64::MAX;
    }

    /// Reads the tapped audio and measures it. Call every frame.
    pub fn update(&mut self, ctl: &mut Controller, rate: u32, playing: bool) {
        if self.analyser.rate() != rate {
            self.analyser = Analyser::new(rate);
        }
        if self.reset_on_play && playing && !self.was_playing {
            self.reset();
        }
        self.was_playing = playing;
        self.buf.clear();
        ctl.take_meter_samples(&mut self.buf);
        let dropped = ctl.status().meter_dropped();
        if self.dropped_at_reset == u64::MAX {
            self.dropped_at_reset = dropped;
        }
        if !self.open {
            return;
        }
        self.missed |= dropped > self.dropped_at_reset;
        self.analyser.push(&self.buf);
    }

    /// Draws the window. Returns a command to send when what is measured
    /// changes. `tracks` is each track's key and name, in order.
    pub fn window(&mut self, ctx: &egui::Context, tracks: &[(&str, &str)]) -> Option<Command> {
        let mut open = self.open;
        egui::Window::new("Meters")
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| self.contents(ui, tracks));
        self.open = open;
        let keys: Vec<&str> = tracks.iter().map(|t| t.0).collect();
        let want = self.wanted(&keys);
        (want != self.sent).then(|| {
            self.sent = want;
            self.command()
        })
    }

    fn contents(&mut self, ui: &mut egui::Ui, tracks: &[(&str, &str)]) {
        ui.horizontal(|ui| {
            ui.label("Measure");
            let name = |k: &Option<String>| match k {
                None => "Master".to_string(),
                Some(k) => tracks
                    .iter()
                    .find(|t| t.0 == k)
                    .map_or("Master".into(), |t| t.1.to_string()),
            };
            let mut chosen = self.track.clone();
            egui::ComboBox::from_id_salt("meters-source")
                .width(160.0)
                .selected_text(name(&self.track))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "Master");
                    for (k, n) in tracks {
                        ui.selectable_value(&mut chosen, Some(k.to_string()), *n);
                    }
                })
                .response
                .accessible_name("Measure");
            if chosen != self.track {
                self.show(chosen);
            }
            if ui
                .button("Reset")
                .on_hover_text("Start integrated loudness, range and the highest readings again")
                .clicked()
            {
                self.reset();
            }
            ui.checkbox(&mut self.reset_on_play, "Reset on play");
        });
        ui.add_space(6.0);

        let l = &self.analyser.loudness;
        let (tp_l, tp_r) = self.analyser.true_peak.max();
        let (sp_l, sp_r) = self.analyser.sample_peak();
        egui::Grid::new("meter-readings")
            .num_columns(4)
            .spacing(vec2(18.0, 4.0))
            .show(ui, |ui| {
                reading(ui, "Momentary", &lu(l.momentary()), "LUFS");
                reading(ui, "Short-term", &lu(l.short_term()), "LUFS");
                ui.end_row();
                reading(ui, "Integrated", &lu(l.integrated()), "LUFS");
                reading(ui, "Range", &format!("{:.1}", l.range()), "LU");
                ui.end_row();
                reading(ui, "True peak", &db(tp_l.max(tp_r)), "dBTP");
                reading(ui, "Sample peak", &db(sp_l.max(sp_r)), "dBFS");
                ui.end_row();
                reading(ui, "Highest momentary", &lu(l.max_momentary()), "LUFS");
                reading(ui, "Highest short-term", &lu(l.max_short_term()), "LUFS");
                ui.end_row();
            });
        if self.missed {
            ui.label(
                RichText::new(
                    "Some audio was missed while the app was busy, so the readings since \
                     the reset may be a little off.",
                )
                .weak(),
            );
        }
        ui.add_space(6.0);

        egui::Grid::new("meter-bars")
            .num_columns(2)
            .spacing(vec2(8.0, 6.0))
            .show(ui, |ui| {
                ui.label(RichText::new("Loudness").weak());
                loudness_bar(ui, l.momentary(), l.short_term(), l.integrated()).on_hover_text(
                    "Momentary (green) and short-term (blue), with integrated as the \
                     white line; −60 to 0 LUFS, ticks every 10",
                );
                ui.end_row();
                ui.label(RichText::new("Correlation").weak());
                correlation(ui, self.analyser.correlation.value()).on_hover_text(
                    "+1: left and right the same (mono). 0: unrelated (wide). \
                     Below 0: out of phase, parts will cancel in mono",
                );
                ui.end_row();
            });
        ui.add_space(6.0);

        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let rate = self.analyser.rate();
        let edges = edges();
        let now = bands(self.analyser.spectrum(), rate, &edges);
        if self.shown.len() != now.len() {
            self.shown = vec![FLOOR; now.len()];
        }
        // Rises at once, falls at 30 dB a second.
        for (s, n) in self.shown.iter_mut().zip(&now) {
            *s = n.max(*s - 30.0 * dt).max(FLOOR);
        }
        spectrum(ui, &edges, &self.shown, rate);
    }
}

fn lu(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.1}")
    } else {
        "–".into()
    }
}

fn db(v: f32) -> String {
    if v > 0.0 {
        format!("{:.1}", 20.0 * v.log10())
    } else {
        "–".into()
    }
}

fn reading(ui: &mut egui::Ui, name: &str, value: &str, unit: &str) {
    ui.label(RichText::new(name).weak());
    ui.label(RichText::new(format!("{value} {unit}")).size(18.0).strong())
        .accessible_name(format!("{name} {value} {unit}"));
}

/// Momentary and short-term as bars, integrated as a line, −60 to 0 LUFS.
fn loudness_bar(ui: &mut egui::Ui, m: f64, s: f64, i: f64) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let tc = crate::theme::look(ui.ctx()).c().clone();
    let p = ui.painter();
    p.rect_filled(rect, 2.0, tc.meter_bg);
    let x = |v: f64| rect.left() + rect.width() * (((v + 60.0) / 60.0).clamp(0.0, 1.0) as f32);
    let h = rect.height() / 2.0;
    for (row, v, color) in [(0.0, m, tc.meter_low), (1.0, s, tc.meter_mid)] {
        if v.is_finite() {
            let top = rect.top() + row * h;
            p.rect_filled(
                egui::Rect::from_x_y_ranges(rect.left()..=x(v), top..=top + h - 1.0),
                1.0,
                color,
            );
        }
    }
    if i.is_finite() {
        p.line_segment(
            [pos2(x(i), rect.top()), pos2(x(i), rect.bottom())],
            Stroke::new(2.0, tc.text),
        );
    }
    for v in (-60..=0).step_by(10) {
        let xv = x(v as f64);
        p.line_segment(
            [pos2(xv, rect.bottom() - 4.0), pos2(xv, rect.bottom())],
            Stroke::new(1.0, tc.text_dim),
        );
    }
    resp
}

/// −1 to +1, with a needle; dimmed while silent.
fn correlation(ui: &mut egui::Ui, v: Option<f32>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let tc = crate::theme::look(ui.ctx()).c().clone();
    let p = ui.painter();
    p.rect_filled(rect, 2.0, tc.meter_bg);
    let inner = rect.shrink2(vec2(18.0, 0.0));
    let x = |v: f32| inner.left() + inner.width() * (v + 1.0) / 2.0;
    p.line_segment(
        [pos2(x(0.0), rect.top()), pos2(x(0.0), rect.bottom())],
        Stroke::new(1.0, tc.text_dim),
    );
    let (at, color) = match v {
        Some(v) if v < 0.0 => (v, tc.meter_high),
        Some(v) => (v, tc.good),
        None => (0.0, tc.line),
    };
    p.line_segment(
        [
            pos2(x(at), rect.top() + 1.0),
            pos2(x(at), rect.bottom() - 1.0),
        ],
        Stroke::new(3.0, color),
    );
    for (t, align, txt) in [
        (-1.0, egui::Align2::LEFT_CENTER, "−1"),
        (1.0, egui::Align2::RIGHT_CENTER, "+1"),
    ] {
        p.text(
            pos2(x(t) + 16.0 * t, rect.center().y),
            align,
            txt,
            egui::FontId::proportional(10.0),
            tc.text_dim,
        );
    }
    resp
}

/// Bands on a log scale, 20 Hz to 20 kHz, from the floor to 0 dB.
fn spectrum(ui: &mut egui::Ui, edges: &[f32], db: &[f32], rate: u32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), Sense::hover());
    let tc = crate::theme::look(ui.ctx()).c().clone();
    let p = ui.painter();
    p.rect_filled(rect, 2.0, tc.meter_bg);
    let x = |hz: f32| rect.left() + rect.width() * (hz / LOW_HZ).log2() / (HIGH_HZ / LOW_HZ).log2();
    let y = |d: f32| rect.bottom() - rect.height() * ((d - FLOOR) / -FLOOR).clamp(0.0, 1.0);
    let grid = Stroke::new(1.0, tc.grid);
    let label = |p: &egui::Painter, at: egui::Pos2, t: &str| {
        p.text(
            at,
            egui::Align2::LEFT_TOP,
            t,
            egui::FontId::proportional(10.0),
            tc.text_dim,
        );
    };
    for (hz, t) in [
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1000.0, "1k"),
        (2000.0, "2k"),
        (5000.0, "5k"),
        (10_000.0, "10k"),
    ] {
        p.line_segment([pos2(x(hz), rect.top()), pos2(x(hz), rect.bottom())], grid);
        label(p, pos2(x(hz) + 2.0, rect.bottom() - 12.0), t);
    }
    for d in [-20.0, -40.0, -60.0, -80.0] {
        p.line_segment([pos2(rect.left(), y(d)), pos2(rect.right(), y(d))], grid);
        label(p, pos2(rect.left() + 2.0, y(d) + 1.0), &format!("{d}"));
    }
    let nyquist = rate as f32 / 2.0;
    let mut points = Vec::with_capacity(db.len());
    for (e, &d) in edges.windows(2).zip(db) {
        if e[0] >= nyquist {
            break;
        }
        points.push(pos2(x((e[0] * e[1]).sqrt()), y(d)));
    }
    if points.len() > 1 {
        for w in points.windows(2) {
            // Filled under the line, a strip at a time (shapes must be convex).
            p.add(egui::Shape::convex_polygon(
                vec![
                    w[0],
                    w[1],
                    pos2(w[1].x, rect.bottom()),
                    pos2(w[0].x, rect.bottom()),
                ],
                tc.accent.gamma_multiply(0.25),
                Stroke::NONE,
            ));
        }
        p.add(egui::Shape::line(points, Stroke::new(1.5, tc.accent)));
    }
}

/// A mixer strip's stereo meter, −60 dB to 0 dB: RMS solid, peak paler
/// beyond it. Click to open the meters window on it.
pub fn strip(ui: &mut egui::Ui, (l, r): (f32, f32), (rl, rr): (f32, f32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(140.0, 14.0), Sense::click());
    let look = crate::theme::look(ui.ctx());
    let tc = look.c();
    let corner = look.radius.min(3.0);
    let p = ui.painter();
    p.rect_filled(rect, corner, tc.meter_bg);
    let h = rect.height() / 2.0;
    for (i, (peak, rms)) in [(l, rl), (r, rr)].into_iter().enumerate() {
        let level = |v: f32| 20.0 * v.max(1e-6).log10();
        let frac = |v: f32| ((level(v) + 60.0) / 60.0).clamp(0.0, 1.0);
        let row = |f: f32| {
            egui::Rect::from_min_size(
                rect.min + vec2(0.0, i as f32 * h),
                vec2(rect.width() * f, h - 1.0),
            )
        };
        let color = |db: f32| {
            if db > -0.5 {
                tc.meter_high
            } else if db > -12.0 {
                tc.meter_mid
            } else {
                tc.meter_low
            }
        };
        p.rect_filled(
            row(frac(peak)),
            1.0,
            color(level(peak)).gamma_multiply(0.45),
        );
        p.rect_filled(row(frac(rms)), 1.0, color(level(peak)));
    }
    p.rect_stroke(
        rect,
        corner,
        Stroke::new(look.line, tc.line),
        egui::StrokeKind::Inside,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_cover_the_audible_range() {
        let e = edges();
        assert_eq!(e[0], LOW_HZ);
        assert!(*e.last().unwrap() >= HIGH_HZ);
        assert_eq!(e.len(), 121);
    }

    #[test]
    fn deleted_track_falls_back_to_the_master() {
        let mut m = Meters::new(48_000);
        assert_eq!(m.wanted(&["a", "b"]), None, "closed: nothing tapped");
        m.show(Some("b".into()));
        assert_eq!(m.wanted(&["a", "b"]), Some(Tap::Track(1)));
        assert_eq!(m.wanted(&["b"]), Some(Tap::Track(0)), "follows the track");
        assert_eq!(m.wanted(&["a"]), Some(Tap::Master));
        assert_eq!(m.track, None);
    }

    #[test]
    fn readings_show_dashes_for_silence() {
        assert_eq!(lu(f64::NEG_INFINITY), "–");
        assert_eq!(lu(-23.04), "-23.0");
        assert_eq!(db(0.0), "–");
        assert_eq!(db(0.5), "-6.0");
    }
}
