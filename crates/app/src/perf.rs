// SPDX-License-Identifier: GPL-3.0-or-later
//! The performance check (roadmap 1.9): the DSP load, and a way to load
//! the session up to Phase 1's bar, 16 stereo tracks with 8 plugin
//! instances, to see whether it plays without dropouts at a small buffer.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use daw_audio_io::StreamStatus;
use daw_plugins::scan::Available;
use eframe::egui::{self, RichText};

/// The bar: tracks, and plugin instances spread over them.
pub const TRACKS: usize = 16;
pub const PLUGINS: usize = 8;
/// Length of each generated test track.
const SECONDS: usize = 30;

/// What the window asks the app to do.
pub enum Action {
    /// Add these files as tracks.
    AddTracks(Vec<PathBuf>),
    /// Put this effect on the first [`PLUGINS`] tracks.
    AddPlugin(Available),
    /// Start counting afresh.
    Reset,
}

/// The app's state, for the window.
pub struct Now {
    /// Late buffers and disk underruns since the device started.
    pub overloads: u64,
    pub underruns: u64,
    /// Frames in the last buffer, and the rate.
    pub buffer: Option<(u64, u32)>,
    pub tracks: usize,
    pub rate: u32,
}

#[derive(Default)]
pub struct Perf {
    pub open: bool,
    /// Recent average load (1.0 is all of the time there is).
    load: f32,
    /// The heaviest callback since reset.
    peak: f32,
    /// Counters when last read or reset.
    seen: (u64, u64),
    overloads_from: u64,
    underruns_from: u64,
    generating: Option<mpsc::Receiver<Result<Vec<PathBuf>, String>>>,
    error: Option<String>,
}

impl Perf {
    /// Reads the device's counters. Call every frame.
    pub fn update(&mut self, status: &StreamStatus) {
        let busy = status.busy_ns.load(Ordering::Relaxed);
        let audio = status.audio_ns.load(Ordering::Relaxed);
        let (b0, a0) = self.seen;
        // Averaged over at least a quarter of a second of audio.
        if audio < a0 || busy < b0 {
            self.seen = (busy, audio);
        } else if audio - a0 >= 250_000_000 {
            self.load = (busy - b0) as f32 / (audio - a0) as f32;
            self.seen = (busy, audio);
        }
        self.peak = self.peak.max(status.take_peak_load());
    }

    /// The load to show beside the device name.
    pub fn load(&self) -> f32 {
        self.load
    }

    pub fn reset(&mut self, overloads: u64, underruns: u64) {
        self.peak = 0.0;
        self.overloads_from = overloads;
        self.underruns_from = underruns;
    }

    /// Draws the window.
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        now: Now,
        effects: &[Available],
    ) -> Option<Action> {
        let Now {
            overloads,
            underruns,
            buffer,
            tracks,
            rate,
        } = now;
        let mut action = None;
        if let Some(rx) = &self.generating
            && let Ok(r) = rx.try_recv()
        {
            self.generating = None;
            match r {
                Ok(files) => action = Some(Action::AddTracks(files)),
                Err(e) => self.error = Some(e),
            }
        }
        let mut open = self.open;
        egui::Window::new("Performance check")
            .open(&mut open)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                egui::Grid::new("perf").num_columns(2).show(ui, |ui| {
                    ui.label("Buffer");
                    ui.label(match buffer {
                        Some((frames, rate)) => format!(
                            "{frames} frames ({:.1} ms)",
                            frames as f64 * 1000.0 / rate as f64
                        ),
                        None => "–".into(),
                    });
                    ui.end_row();
                    ui.label("DSP load");
                    ui.label(format!(
                        "{:.0}% now, {:.0}% peak",
                        self.load * 100.0,
                        self.peak * 100.0
                    ))
                    .on_hover_text(
                        "Time spent making each buffer of audio, as a share of how long \
                             the buffer lasts. Over 100% is a dropout.",
                    );
                    ui.end_row();
                    let over = overloads - self.overloads_from.min(overloads);
                    ui.label("Dropouts");
                    ui.label(if over == 0 {
                        RichText::new("none").color(crate::theme::look(ui.ctx()).c().good)
                    } else {
                        RichText::new(format!("{over} buffers late"))
                            .color(ui.visuals().error_fg_color)
                    });
                    ui.end_row();
                    let under = underruns - self.underruns_from.min(underruns);
                    ui.label("Disk");
                    ui.label(if under == 0 {
                        RichText::new("kept up").color(crate::theme::look(ui.ctx()).c().good)
                    } else {
                        RichText::new(format!("{under} frames missed"))
                            .color(ui.visuals().error_fg_color)
                    });
                    ui.end_row();
                });
                if ui.button("Reset").clicked() {
                    action = Some(Action::Reset);
                }
                ui.separator();
                ui.label(format!(
                    "Phase 1's bar: {TRACKS} stereo tracks and {PLUGINS} plugins play with no \
                     dropouts at 128 frames. Set the buffer in Settings → Audio device, then:"
                ));
                ui.horizontal(|ui| {
                    let busy = self.generating.is_some();
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(format!("Add {TRACKS} test tracks")),
                        )
                        .on_hover_text(format!("{SECONDS} s of generated audio each"))
                        .clicked()
                    {
                        self.generating = Some(generate(rate));
                        self.error = None;
                    }
                    if busy {
                        ui.spinner();
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(tracks >= PLUGINS, |ui| {
                        egui::ComboBox::from_id_salt("perf-plugin")
                            .selected_text(format!("Add an effect to {PLUGINS} tracks…"))
                            .width(260.0)
                            .show_ui(ui, |ui| {
                                for p in effects {
                                    if ui.selectable_label(false, &p.class.name).clicked() {
                                        action = Some(Action::AddPlugin(p.clone()));
                                    }
                                }
                                if effects.is_empty() {
                                    ui.label("No effects found. See Settings → Plugins.");
                                }
                            })
                            .response
                            .accessible_name("Effect for the test tracks");
                    });
                });
                ui.label(
                    RichText::new(
                        "Then loop and play for a few minutes while using the app. \
                         A typical EQ or compressor is a fair test.",
                    )
                    .weak(),
                );
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            });
        self.open = open;
        action
    }
}

/// Writes the test tracks in the background, at `rate`.
fn generate(rate: u32) -> mpsc::Receiver<Result<Vec<PathBuf>, String>> {
    let (tx, rx) = mpsc::channel();
    let dir = crate::cache_dir().with_file_name("test-audio");
    std::thread::spawn(move || {
        let r = std::fs::create_dir_all(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))
            .and_then(|()| {
                (0..TRACKS)
                    .map(|i| {
                        let path = dir.join(format!("Test {:02} {rate}.wav", i + 1));
                        if !path.exists() {
                            write_test_track(&path, i, rate)?;
                        }
                        Ok(path)
                    })
                    .collect()
            });
        let _ = tx.send(r);
    });
    rx
}

/// A stereo track of plucked-ish notes, different for each `n`.
fn write_test_track(path: &Path, n: usize, rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let err = |e: hound::Error| format!("{}: {e}", path.display());
    let tmp = path.with_extension("part");
    let mut w = hound::WavWriter::create(&tmp, spec).map_err(err)?;
    let r = rate as f32;
    // A note every half beat at 120 BPM, walking a pentatonic scale.
    let step = (rate / 4) as usize;
    let scale = [0, 3, 5, 7, 10, 12, 15];
    let base = 82.4 * 2f32.powf((n % 12) as f32 / 12.0);
    let mut noise = 0x9e37_79b9u32 ^ n as u32;
    for i in 0..rate as usize * SECONDS {
        let note = i / step;
        let t = (i % step) as f32 / r;
        let semis = scale[(note * (n + 2)) % scale.len()] as f32;
        let hz = base * 2f32.powf(semis / 12.0);
        let env = (-t * 6.0).exp();
        let x = (t * hz * std::f32::consts::TAU).sin()
            + 0.3 * (t * 2.0 * hz * std::f32::consts::TAU).sin();
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        let hiss = (noise as f32 / u32::MAX as f32 - 0.5) * 0.01;
        let v = 0.15 * env * x;
        // Spread across the stereo field by track.
        let pan = (n as f32 / (TRACKS - 1) as f32) * 0.8 + 0.1;
        for s in [v * (1.0 - pan) + hiss, v * pan + hiss] {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
                .map_err(err)?;
        }
    }
    w.finalize().map_err(err)?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}
