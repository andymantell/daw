// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 C, first candidate toolkit (egui): a timeline with hundreds of
//! clips and waveforms, smooth scrolling and zooming, plus the everyday UI
//! pieces a DAW needs (menus, text entry, drag and drop, shortcuts, screen
//! reader labels).
//!
//! Usage:
//!   ui-timeline [--tracks 32] [--clips-per-track 10]
//!   ui-timeline --bench 20      scrolls and zooms by itself for 20 s,
//!                               then saves ui-bench-report.txt and exits
//!   ui-timeline --panel         adds a native child window at the bottom,
//!                               where a plugin editor would go
//!   ui-timeline --plugin <bundle.vst3>
//!                               opens that plugin's editor in the panel
//!                               (Windows)
//!
//! Controls: mouse wheel scrolls, Ctrl+wheel (or +/-) zooms, drag a clip to
//! move it, double-click a clip to rename it, Space starts and stops the
//! playhead, F2 renames the selected clip, Delete removes it.

use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, FontId, Key, Mesh, Modifiers, Rect, Sense, Stroke, StrokeKind, Vec2,
    pos2, vec2,
};

const SAMPLE_RATE: f64 = 48_000.0;
/// Finest waveform summary: one min/max pair per this many samples.
const BASE_HOP: usize = 64;
const TRACK_HEIGHT: f32 = 64.0;
const HEADER_WIDTH: f32 = 150.0;
const RULER_HEIGHT: f32 = 24.0;

/// Waveform summary at several resolutions, each level halving the last.
struct Peaks {
    levels: Vec<Vec<(f32, f32)>>,
}

impl Peaks {
    /// Synthetic but realistic-looking audio: notes with attack/decay
    /// envelopes over a little noise.
    fn synth(seconds: f64, seed: u64) -> Peaks {
        let n = (seconds * SAMPLE_RATE / BASE_HOP as f64) as usize;
        let mut rng = seed | 1;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 40) as f32 / (1u64 << 24) as f32
        };
        let note_len = 40 + (seed % 60) as usize;
        let base: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let t = (i % note_len) as f32 / note_len as f32;
                let env = (1.0 - t).powf(2.0) * (0.4 + 0.6 * ((i / note_len) % 3) as f32 / 2.0);
                let a = (env * (0.8 + 0.2 * next())).min(1.0) + 0.03 * next();
                (-a, a)
            })
            .collect();
        let mut levels = vec![base];
        while levels.last().unwrap().len() > 1 {
            let prev = levels.last().unwrap();
            let next: Vec<_> = prev
                .chunks(2)
                .map(|c| {
                    c.iter()
                        .fold((0.0f32, 0.0f32), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)))
                })
                .collect();
            levels.push(next);
        }
        Peaks { levels }
    }
}

struct Clip {
    name: String,
    start: f64, // seconds
    len: f64,
    peaks: usize, // index into App::peaks
    color: Color32,
}

struct Track {
    name: String,
    clips: Vec<Clip>,
}

#[derive(Default)]
struct Bench {
    seconds: f64,
    started: Option<Instant>,
    cpu: Vec<f32>,
    /// Time spent in our own code building the frame (no rendering).
    layout: Vec<f32>,
    intervals: Vec<f32>,
    last: Option<Instant>,
}

struct App {
    tracks: Vec<Track>,
    peaks: Vec<Peaks>,
    /// Seconds at the left edge, and pixels per second.
    view_start: f64,
    px_per_sec: f64,
    scroll_y: f32,
    selected: Option<(usize, usize)>,
    renaming: Option<(usize, usize, String)>,
    dragging: Option<(usize, usize, f64)>,
    playing: Option<(Instant, f64)>,
    playhead: f64,
    show_stats: bool,
    frame_ms: Vec<f32>,
    bench: Option<Bench>,
    embed: Option<Embed>,
}

/// The native child window for a plugin editor (`--panel`, `--plugin`).
struct Embed {
    plugin: Option<std::path::PathBuf>,
    panel: Option<embed_panel::Panel>,
    status: String,
}

const PANEL_HEIGHT: f32 = 260.0;

impl App {
    fn new(tracks: usize, clips_per_track: usize, bench: Option<f64>) -> App {
        let mut peaks = Vec::new();
        let mut list = Vec::new();
        for t in 0..tracks {
            let mut clips = Vec::new();
            let mut at = (t % 4) as f64 * 1.5;
            for c in 0..clips_per_track {
                let len = 6.0 + ((t * 7 + c * 3) % 9) as f64;
                peaks.push(Peaks::synth(len, (t * 1000 + c) as u64 + 1));
                let hue = ((t * 37 + c * 11) % 360) as f32 / 360.0;
                clips.push(Clip {
                    name: format!("Take {}-{}", t + 1, c + 1),
                    start: at,
                    len,
                    peaks: peaks.len() - 1,
                    color: egui::ecolor::Hsva::new(hue, 0.45, 0.55, 1.0).into(),
                });
                at += len + 0.5 + (c % 3) as f64;
            }
            list.push(Track {
                name: format!("Track {}", t + 1),
                clips,
            });
        }
        App {
            tracks: list,
            peaks,
            view_start: 0.0,
            px_per_sec: 40.0,
            scroll_y: 0.0,
            selected: None,
            renaming: None,
            dragging: None,
            playing: None,
            playhead: 0.0,
            show_stats: true,
            frame_ms: Vec::new(),
            embed: None,
            bench: bench.map(|seconds| Bench {
                seconds,
                ..Default::default()
            }),
        }
    }

    fn clip_count(&self) -> usize {
        self.tracks.iter().map(|t| t.clips.len()).sum()
    }

    fn zoom_at(&mut self, factor: f64, anchor_x: f32, area_left: f32) {
        let t = self.view_start + (anchor_x - area_left) as f64 / self.px_per_sec;
        self.px_per_sec = (self.px_per_sec * factor).clamp(0.5, 20_000.0);
        self.view_start = (t - (anchor_x - area_left) as f64 / self.px_per_sec).max(0.0);
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let has = self.selected.is_some();
                if ui
                    .add_enabled(has, egui::Button::new("Rename clip (F2)"))
                    .clicked()
                {
                    self.start_rename();
                    ui.close();
                }
                if ui
                    .add_enabled(has, egui::Button::new("Delete clip (Del)"))
                    .clicked()
                {
                    self.delete_selected();
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Zoom in (+)").clicked() {
                    self.px_per_sec *= 1.25;
                }
                if ui.button("Zoom out (-)").clicked() {
                    self.px_per_sec /= 1.25;
                }
                ui.checkbox(&mut self.show_stats, "Frame statistics");
            });
            ui.separator();
            let label = if self.playing.is_some() {
                "Stop"
            } else {
                "Play"
            };
            if ui.button(label).on_hover_text("Space").clicked() {
                self.toggle_play();
            }
            ui.label(format!("{:.2} s", self.playhead));
        });
    }

    fn toggle_play(&mut self) {
        self.playing = match self.playing {
            Some(_) => None,
            None => Some((Instant::now(), self.playhead)),
        };
    }

    fn start_rename(&mut self) {
        if let Some((t, c)) = self.selected {
            self.renaming = Some((t, c, self.tracks[t].clips[c].name.clone()));
        }
    }

    fn delete_selected(&mut self) {
        if let Some((t, c)) = self.selected.take() {
            self.tracks[t].clips.remove(c);
        }
    }

    fn shortcuts(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return; // Typing into a text field.
        }
        let (space, f2, del, plus, minus) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Space),
                i.consume_key(Modifiers::NONE, Key::F2),
                i.consume_key(Modifiers::NONE, Key::Delete),
                i.consume_key(Modifiers::NONE, Key::Plus)
                    || i.consume_key(Modifiers::NONE, Key::Equals),
                i.consume_key(Modifiers::NONE, Key::Minus),
            )
        });
        if space {
            self.toggle_play();
        }
        if f2 {
            self.start_rename();
        }
        if del {
            self.delete_selected();
        }
        if plus {
            self.px_per_sec *= 1.25;
        }
        if minus {
            self.px_per_sec /= 1.25;
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let full = resp.rect;
        let area = Rect::from_min_max(
            pos2(full.left() + HEADER_WIDTH, full.top() + RULER_HEIGHT),
            full.max,
        );
        let visuals = ui.visuals().clone();
        painter.rect_filled(full, 0.0, visuals.extreme_bg_color);

        // Scroll and zoom.
        if resp.hovered() {
            let (scroll, zoom, ctrl) =
                ui.input(|i| (i.smooth_scroll_delta(), i.zoom_delta(), i.modifiers.command));
            if zoom != 1.0 || ctrl {
                let x = ui
                    .input(|i| i.pointer.hover_pos())
                    .map_or(area.center().x, |p| p.x);
                self.zoom_at(zoom as f64, x, area.left());
            } else {
                self.view_start = (self.view_start - (scroll.x as f64) / self.px_per_sec).max(0.0);
                self.scroll_y -= scroll.y;
            }
        }
        let content_h = self.tracks.len() as f32 * TRACK_HEIGHT;
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_h - area.height()).max(0.0));

        let to_x = |t: f64, s: &App| area.left() + ((t - s.view_start) * s.px_per_sec) as f32;
        let view_end = self.view_start + area.width() as f64 / self.px_per_sec;

        // Ruler.
        let ruler = Rect::from_min_max(
            pos2(area.left(), full.top()),
            pos2(full.right(), area.top()),
        );
        painter.rect_filled(ruler, 0.0, visuals.faint_bg_color);
        let step = [0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0]
            .into_iter()
            .find(|s| s * self.px_per_sec >= 70.0)
            .unwrap_or(600.0);
        let mut t = (self.view_start / step).floor() * step;
        while t <= view_end {
            let x = to_x(t, self);
            painter.line_segment(
                [pos2(x, ruler.top() + 12.0), pos2(x, ruler.bottom())],
                visuals.widgets.noninteractive.fg_stroke,
            );
            painter.text(
                pos2(x + 3.0, ruler.top() + 2.0),
                Align2::LEFT_TOP,
                format!("{t:.1}s"),
                FontId::proportional(11.0),
                visuals.text_color(),
            );
            t += step;
        }

        let clip_painter = painter.with_clip_rect(area);
        let font = FontId::proportional(12.0);
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let mut hit: Option<(usize, usize)> = None;

        for (ti, track) in self.tracks.iter().enumerate() {
            let y = area.top() + ti as f32 * TRACK_HEIGHT - self.scroll_y;
            if y + TRACK_HEIGHT < area.top() || y > area.bottom() {
                continue;
            }
            let header =
                Rect::from_min_size(pos2(full.left(), y), vec2(HEADER_WIDTH, TRACK_HEIGHT));
            let hp = painter.with_clip_rect(Rect::from_min_max(
                pos2(full.left(), area.top()),
                pos2(area.left(), area.bottom()),
            ));
            hp.rect_filled(header.shrink(1.0), 2.0, visuals.widgets.inactive.bg_fill);
            hp.text(
                header.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                &track.name,
                font.clone(),
                visuals.text_color(),
            );
            clip_painter.line_segment(
                [
                    pos2(area.left(), y + TRACK_HEIGHT),
                    pos2(area.right(), y + TRACK_HEIGHT),
                ],
                Stroke::new(1.0, visuals.faint_bg_color),
            );

            for (ci, clip) in track.clips.iter().enumerate() {
                if clip.start > view_end || clip.start + clip.len < self.view_start {
                    continue;
                }
                let r = Rect::from_min_max(
                    pos2(to_x(clip.start, self), y + 2.0),
                    pos2(to_x(clip.start + clip.len, self), y + TRACK_HEIGHT - 2.0),
                );
                if pointer.is_some_and(|p| r.contains(p) && area.contains(p)) {
                    hit = Some((ti, ci));
                }
                let selected = self.selected == Some((ti, ci));
                clip_painter.rect_filled(r, 3.0, clip.color.gamma_multiply(0.55));
                draw_waveform(
                    &clip_painter,
                    &self.peaks[clip.peaks],
                    r.shrink2(vec2(0.0, 14.0)).translate(vec2(0.0, 6.0)),
                    self,
                    area,
                );
                clip_painter.rect_stroke(
                    r,
                    3.0,
                    if selected {
                        Stroke::new(2.0, Color32::WHITE)
                    } else {
                        Stroke::new(1.0, clip.color)
                    },
                    StrokeKind::Inside,
                );
                if r.width() > 40.0 {
                    clip_painter.text(
                        r.left_top() + vec2(4.0, 2.0),
                        Align2::LEFT_TOP,
                        &clip.name,
                        font.clone(),
                        Color32::WHITE,
                    );
                }
            }
        }

        // Selection, drag to move, double-click to rename.
        if resp.drag_started() || resp.clicked() {
            self.selected = hit;
        }
        if resp.drag_started()
            && let (Some((t, c)), Some(p)) = (hit, pointer)
        {
            let at = self.view_start + (p.x - area.left()) as f64 / self.px_per_sec;
            self.dragging = Some((t, c, at - self.tracks[t].clips[c].start));
        }
        if let (Some((t, c, grab)), Some(p)) = (self.dragging, pointer)
            && resp.dragged()
        {
            let at = self.view_start + (p.x - area.left()) as f64 / self.px_per_sec;
            // Snap to 0.25 s.
            self.tracks[t].clips[c].start = (((at - grab) * 4.0).round() / 4.0).max(0.0);
        }
        if resp.drag_stopped() {
            self.dragging = None;
        }
        if resp.double_clicked() && hit.is_some() {
            self.selected = hit;
            self.start_rename();
        }

        // Playhead.
        let x = to_x(self.playhead, self);
        clip_painter.line_segment(
            [pos2(x, area.top()), pos2(x, area.bottom())],
            Stroke::new(2.0, Color32::from_rgb(255, 80, 60)),
        );

        // Screen-reader summary of what is on the timeline.
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Canvas,
                true,
                format!(
                    "Timeline, {} tracks, {} clips, playhead {:.1} seconds{}",
                    self.tracks.len(),
                    self.clip_count(),
                    self.playhead,
                    self.selected
                        .map(|(t, c)| format!(", selected {}", self.tracks[t].clips[c].name))
                        .unwrap_or_default()
                ),
            )
        });
    }

    /// A bottom panel whose inside is a native child window. The window is
    /// created on the first frame and moved to follow the layout.
    fn embed_panel(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let Some(e) = &mut self.embed else {
            return;
        };
        let ppp = ui.ctx().pixels_per_point();
        egui::Panel::bottom("plugin")
            .exact_size(PANEL_HEIGHT)
            .show(ui, |ui| {
                ui.label(format!("Plugin editor: {}", e.status));
                let area = ui.available_rect_before_wrap();
                ui.allocate_rect(area, Sense::hover());
                let rect = embed_panel::Rect {
                    x: (area.left() * ppp).round() as i32,
                    y: (area.top() * ppp).round() as i32,
                    w: (area.width() * ppp).round() as i32,
                    h: (area.height() * ppp).round() as i32,
                };
                match &mut e.panel {
                    Some(p) => p.set_rect(rect),
                    None => match embed_panel::Panel::new(frame, rect) {
                        Ok(mut p) => {
                            e.status = match &e.plugin {
                                Some(b) => p.open_plugin(b).unwrap_or_else(|err| err),
                                None => p.status(),
                            };
                            e.panel = Some(p);
                        }
                        Err(err) => e.status = format!("no panel: {err}"),
                    },
                }
            });
    }

    fn rename_window(&mut self, ui: &mut egui::Ui) {
        let Some((t, c, mut text)) = self.renaming.take() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut done = false;
        egui::Window::new("Rename clip")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text("Clip name"));
                r.request_focus();
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    done = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if done {
            if let Some(clip) = self.tracks.get_mut(t).and_then(|tr| tr.clips.get_mut(c)) {
                clip.name = text;
            }
        } else if open && !cancel {
            self.renaming = Some((t, c, text));
        }
    }
}

/// Draws min/max columns, one per pixel, from the closest summary level.
fn draw_waveform(painter: &egui::Painter, peaks: &Peaks, r: Rect, app: &App, area: Rect) {
    if r.height() < 4.0 {
        return;
    }
    let samples_per_px = SAMPLE_RATE / app.px_per_sec;
    let level = ((samples_per_px / BASE_HOP as f64).max(1.0).log2().floor() as usize)
        .min(peaks.levels.len() - 1);
    let data = &peaks.levels[level];
    let hop_secs = (BASE_HOP << level) as f64 / SAMPLE_RATE;
    let x0 = r.left().max(area.left()).floor();
    let x1 = r.right().min(area.right()).ceil();
    let mid = r.center().y;
    let half = r.height() / 2.0;
    let color = Color32::from_white_alpha(200);
    let mut mesh = Mesh::default();
    let mut x = x0;
    while x < x1 {
        let t0 = (x - r.left()) as f64 / app.px_per_sec;
        let t1 = t0 + 1.0 / app.px_per_sec;
        let i0 = (t0 / hop_secs) as usize;
        let i1 = ((t1 / hop_secs).ceil() as usize)
            .max(i0 + 1)
            .min(data.len());
        if i0 < data.len() {
            let (lo, hi) = data[i0..i1]
                .iter()
                .fold((0.0f32, 0.0f32), |(l, h), &(a, b)| (l.min(a), h.max(b)));
            mesh.add_colored_rect(
                Rect::from_min_max(
                    pos2(x, mid - hi * half),
                    pos2(x + 1.0, mid - lo * half + 1.0),
                ),
                color,
            );
        }
        x += 1.0;
    }
    painter.add(mesh);
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let now = Instant::now();
        if let Some(cpu) = frame.info().cpu_usage {
            self.frame_ms.push(cpu * 1000.0);
            if self.frame_ms.len() > 240 {
                self.frame_ms.remove(0);
            }
        }

        // Benchmark: scroll and zoom continuously, record frame costs.
        let mut bench_done = None;
        if let Some(b) = &mut self.bench {
            let started = *b.started.get_or_insert(now);
            let el = now.duration_since(started).as_secs_f64();
            if let Some(cpu) = frame.info().cpu_usage {
                b.cpu.push(cpu * 1000.0);
            }
            if let Some(last) = b.last.replace(now) {
                b.intervals
                    .push(now.duration_since(last).as_secs_f32() * 1000.0);
            }
            // Pan back and forth, zoom in and out, and scroll the tracks.
            self.px_per_sec = 40.0 * (2.0f64).powf(3.0 * (el * 0.4).sin());
            self.view_start = (60.0 * (1.0 - (el * 0.3).cos())).max(0.0);
            self.scroll_y = (self.tracks.len() as f32 * TRACK_HEIGHT * 0.5)
                * (1.0 - (el as f32 * 0.5).cos())
                / 2.0;
            if el >= b.seconds {
                bench_done = Some(());
            }
            ui.ctx().request_repaint();
        }

        if let Some((t0, from)) = self.playing {
            self.playhead = from + t0.elapsed().as_secs_f64();
            ui.ctx().request_repaint();
        }

        self.shortcuts(ui);
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        if self.show_stats {
            egui::Panel::bottom("stats").show(ui, |ui| {
                let mut v = self.frame_ms.clone();
                v.sort_by(f32::total_cmp);
                ui.label(format!(
                    "{} tracks, {} clips | frame CPU: median {:.2} ms, 99th pct {:.2} ms, worst {:.2} ms (budget 16.7 ms at 60 fps)",
                    self.tracks.len(),
                    self.clip_count(),
                    percentile(&v, 0.5),
                    percentile(&v, 0.99),
                    v.last().copied().unwrap_or(0.0)
                ));
            });
        }
        if self.embed.is_some() {
            self.embed_panel(ui, frame);
        }
        egui::CentralPanel::default().show(ui, |ui| self.timeline(ui));
        self.rename_window(ui);
        if let Some(b) = &mut self.bench {
            b.layout.push(now.elapsed().as_secs_f32() * 1000.0);
        }

        if bench_done.is_some() {
            let b = self.bench.take().unwrap();
            let report = bench_report(&b, self);
            print!("{report}");
            let _ = std::fs::write("ui-bench-report.txt", &report);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn bench_report(b: &Bench, app: &App) -> String {
    let mut cpu = b.cpu.clone();
    cpu.sort_by(f32::total_cmp);
    let mut iv = b.intervals.clone();
    iv.sort_by(f32::total_cmp);
    let mut lay = b.layout.clone();
    lay.sort_by(f32::total_cmp);
    let over = |v: &[f32], ms: f32| v.iter().filter(|&&x| x > ms).count();
    let fps = if iv.is_empty() {
        0.0
    } else {
        1000.0 / (iv.iter().sum::<f32>() / iv.len() as f32)
    };
    format!(
        "UI timeline benchmark (egui)\n\
         \x20 {} tracks, {} clips with waveforms, {:.0} s of automatic scroll and zoom\n\
         \x20 frames: {}\n\
         \x20 frame CPU (UI + rendering): median {:.2} ms, 95th {:.2} ms, 99th {:.2} ms, worst {:.2} ms\n\
         \x20 of which building the frame (our code, no drawing): median {:.2} ms, 99th {:.2} ms, worst {:.2} ms\n\
         \x20 frames over 16.7 ms of CPU: {}\n\
         \x20 time between frames: median {:.2} ms, 99th {:.2} ms, worst {:.2} ms ({:.0} fps average)\n\
         \x20 gaps over 25 ms (visible stutter at 60 Hz): {}\n\
         \x20 verdict: {}\n",
        app.tracks.len(),
        app.clip_count(),
        b.seconds,
        cpu.len(),
        percentile(&cpu, 0.5),
        percentile(&cpu, 0.95),
        percentile(&cpu, 0.99),
        cpu.last().copied().unwrap_or(0.0),
        percentile(&lay, 0.5),
        percentile(&lay, 0.99),
        lay.last().copied().unwrap_or(0.0),
        over(&cpu, 16.7),
        percentile(&iv, 0.5),
        percentile(&iv, 0.99),
        iv.last().copied().unwrap_or(0.0),
        fps,
        over(&iv, 25.0),
        if percentile(&cpu, 0.99) < 16.7 && fps >= 55.0 {
            "PASS (60 fps held)"
        } else {
            "FAIL (see numbers above)"
        }
    )
}

fn main() -> eframe::Result {
    let mut tracks = 32;
    let mut per = 10;
    let mut bench = None;
    let mut embed = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut num = || args.next().and_then(|v| v.parse::<f64>().ok());
        match a.as_str() {
            "--tracks" => tracks = num().unwrap_or(32.0) as usize,
            "--clips-per-track" => per = num().unwrap_or(10.0) as usize,
            "--bench" => bench = Some(num().unwrap_or(20.0)),
            "--panel" => embed = Some(None),
            "--plugin" => embed = Some(args.next().map(std::path::PathBuf::from)),
            _ => {
                eprintln!(
                    "usage: ui-timeline [--tracks N] [--clips-per-track N] [--bench SECONDS] [--panel] [--plugin BUNDLE]"
                );
                std::process::exit(2);
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Timeline UI spike")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ui-timeline",
        options,
        Box::new(move |_cc| {
            let mut app = App::new(tracks, per, bench);
            app.embed = embed.map(|plugin| Embed {
                plugin,
                panel: None,
                status: "starting".into(),
            });
            Ok(Box::new(app))
        }),
    )
}
