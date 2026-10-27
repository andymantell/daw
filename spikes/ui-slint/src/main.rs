// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 C, second candidate toolkit (Slint): the same timeline as
//! `ui-timeline` (egui), so the two can be compared side by side.
//!
//! Usage:
//!   ui-timeline-slint [--tracks 32] [--clips-per-track 10]
//!   ui-timeline-slint --bench 20     scrolls and zooms by itself for 20 s,
//!                                    then saves ui-slint-bench-report.txt
//!   ui-timeline-slint --panel        adds a native child window at the
//!                                    bottom, where a plugin editor would go
//!   ui-timeline-slint --plugin <bundle.vst3>
//!                                    opens that plugin's editor there
//!                                    (Windows)
//!
//! Controls are the same as the egui version: mouse wheel scrolls,
//! Ctrl+wheel (or +/-) zooms, drag a clip to move it, double-click a clip to
//! rename it, Space starts and stops the playhead, F2 renames the selected
//! clip, Delete removes it.
//!
//! Unlike egui, Slint keeps a tree of elements between frames. Each frame
//! we work out which clips are visible and update the element list; each
//! clip's waveform is an image drawn here and cached until the zoom
//! changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{
    Color, ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString,
    Timer, TimerMode, VecModel,
};

slint::include_modules!();

const SAMPLE_RATE: f64 = 48_000.0;
/// Finest waveform summary: one min/max pair per this many samples.
const BASE_HOP: usize = 64;
const TRACK_HEIGHT: f32 = 64.0;
/// Waveform image height in pixels; the image is stretched to the clip.
const WAVE_H: usize = 40;

/// Waveform summary at several resolutions, each level halving the last.
/// The same synthetic audio as the egui version.
struct Peaks {
    levels: Vec<Vec<(f32, f32)>>,
}

impl Peaks {
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

    /// Draws `w` pixel columns starting `off` pixels into the clip, at
    /// `px_per_sec`, as white min/max bars on a transparent image.
    fn render(&self, off: i32, w: usize, px_per_sec: f64) -> Image {
        let samples_per_px = SAMPLE_RATE / px_per_sec;
        let level = ((samples_per_px / BASE_HOP as f64).max(1.0).log2().floor() as usize)
            .min(self.levels.len() - 1);
        let data = &self.levels[level];
        let hop_secs = (BASE_HOP << level) as f64 / SAMPLE_RATE;
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w.max(1) as u32, WAVE_H as u32);
        let px = buf.make_mut_slice();
        let half = WAVE_H as f32 / 2.0;
        for x in 0..w {
            let t0 = (off as f64 + x as f64) / px_per_sec;
            let t1 = t0 + 1.0 / px_per_sec;
            let i0 = (t0 / hop_secs) as usize;
            let i1 = ((t1 / hop_secs).ceil() as usize)
                .max(i0 + 1)
                .min(data.len());
            if i0 >= data.len() {
                break;
            }
            let (lo, hi) = data[i0..i1]
                .iter()
                .fold((0.0f32, 0.0f32), |(l, h), &(a, b)| (l.min(a), h.max(b)));
            let top = ((half - hi * half).floor().max(0.0)) as usize;
            let bottom = ((half - lo * half).ceil() as usize).min(WAVE_H - 1);
            for y in top..=bottom {
                px[y * w + x] = Rgba8Pixel::new(255, 255, 255, 200);
            }
        }
        Image::from_rgba8(buf)
    }
}

struct Clip {
    name: String,
    start: f64,
    len: f64,
    peaks: usize,
    color: Color,
}

struct Track {
    name: String,
    clips: Vec<Clip>,
}

#[derive(Default)]
struct Bench {
    seconds: f64,
    started: Option<Instant>,
    /// Rendering time per frame, measured around Slint's renderer.
    render: Vec<f32>,
    /// Time in our code working out the visible clips and waveforms.
    layout: Vec<f32>,
    intervals: Vec<f32>,
    last: Option<Instant>,
    /// Frames rendered in total, including any we did not time.
    frames: usize,
}

struct State {
    tracks: Vec<Track>,
    peaks: Vec<Peaks>,
    view_start: f64,
    px_per_sec: f64,
    scroll_y: f32,
    selected: Option<(usize, usize)>,
    renaming: Option<(usize, usize)>,
    /// Clip being dragged and where in it the pointer grabbed it (s).
    dragging: Option<(usize, usize, f64)>,
    playing: Option<(Instant, f64)>,
    playhead: f64,
    /// Waveform images by (clip peaks, zoom, offset into clip, width).
    waves: HashMap<(usize, u64, i32, usize), Image>,
    render_ms: Vec<f32>,
    render_start: Option<Instant>,
    bench: Option<Bench>,
}

fn hsv(h: f32, s: f32, v: f32) -> Color {
    let c = v * s;
    let hp = h * 6.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let u = |f: f32| ((f + m) * 255.0).round() as u8;
    Color::from_rgb_u8(u(r), u(g), u(b))
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

impl State {
    fn new(tracks: usize, clips_per_track: usize, bench: Option<f64>) -> State {
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
                    color: hsv(hue, 0.45, 0.75),
                });
                at += len + 0.5 + (c % 3) as f64;
            }
            list.push(Track {
                name: format!("Track {}", t + 1),
                clips,
            });
        }
        State {
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
            waves: HashMap::new(),
            render_ms: Vec::new(),
            render_start: None,
            bench: bench.map(|seconds| Bench {
                seconds,
                ..Default::default()
            }),
        }
    }

    fn clip_count(&self) -> usize {
        self.tracks.iter().map(|t| t.clips.len()).sum()
    }

    fn zoom_at(&mut self, factor: f64, anchor_x: f32) {
        let t = self.view_start + anchor_x as f64 / self.px_per_sec;
        self.px_per_sec = (self.px_per_sec * factor).clamp(0.5, 20_000.0);
        self.view_start = (t - anchor_x as f64 / self.px_per_sec).max(0.0);
    }

    fn time_at(&self, x: f32) -> f64 {
        self.view_start + x as f64 / self.px_per_sec
    }

    fn hit(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let ti = ((y + self.scroll_y) / TRACK_HEIGHT).floor();
        if ti < 0.0 {
            return None;
        }
        let ti = ti as usize;
        let t = self.time_at(x);
        let track = self.tracks.get(ti)?;
        track
            .clips
            .iter()
            .position(|c| t >= c.start && t < c.start + c.len)
            .map(|ci| (ti, ci))
    }

    fn delete_selected(&mut self) {
        if let Some((t, c)) = self.selected.take() {
            self.tracks[t].clips.remove(c);
        }
    }

    fn toggle_play(&mut self) {
        self.playing = match self.playing {
            Some(_) => None,
            None => Some((Instant::now(), self.playhead)),
        };
    }

    fn summary(&self) -> String {
        format!(
            "Timeline, {} tracks, {} clips, playhead {:.1} seconds{}",
            self.tracks.len(),
            self.clip_count(),
            self.playhead,
            self.selected
                .map(|(t, c)| format!(", selected {}", self.tracks[t].clips[c].name))
                .unwrap_or_default()
        )
    }
}

/// Makes `model` hold `rows`, changing only the rows that differ, so Slint
/// keeps the existing elements.
fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    let n = model.row_count();
    for (i, row) in rows.iter().enumerate() {
        if i < n {
            if model.row_data(i).as_ref() != Some(row) {
                model.set_row_data(i, row.clone());
            }
        } else {
            model.push(row.clone());
        }
    }
    for i in (rows.len()..n).rev() {
        model.remove(i);
    }
}

type Handler = Box<dyn Fn(&MainWindow, &mut State)>;

struct Models {
    clips: Rc<VecModel<ClipView>>,
    tracks: Rc<VecModel<TrackView>>,
    ticks: Rc<VecModel<Tick>>,
}

/// Works out what is visible and pushes it into the window.
fn refresh(ui: &MainWindow, s: &mut State, m: &Models) {
    let width = ui.get_area_width();
    let height = ui.get_area_height();
    let content_h = s.tracks.len() as f32 * TRACK_HEIGHT;
    s.scroll_y = s.scroll_y.clamp(0.0, (content_h - height).max(0.0));
    let view_end = s.time_at(width);
    let to_x = |t: f64| ((t - s.view_start) * s.px_per_sec) as f32;

    let step = [0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0]
        .into_iter()
        .find(|st| st * s.px_per_sec >= 70.0)
        .unwrap_or(600.0);
    let mut ticks = Vec::new();
    let mut t = (s.view_start / step).floor() * step;
    while t <= view_end {
        ticks.push(Tick {
            x: to_x(t),
            label: SharedString::from(format!("{t:.1}s")),
        });
        t += step;
    }

    let mut tracks = Vec::new();
    let mut clips = Vec::new();
    let zoom_key = s.px_per_sec.to_bits();
    if s.waves.len() > 4000 {
        s.waves.clear();
    }
    for (ti, track) in s.tracks.iter().enumerate() {
        let y = ti as f32 * TRACK_HEIGHT - s.scroll_y;
        if y + TRACK_HEIGHT < 0.0 || y > height {
            continue;
        }
        tracks.push(TrackView {
            y,
            name: SharedString::from(track.name.as_str()),
        });
        for (ci, clip) in track.clips.iter().enumerate() {
            if clip.start > view_end || clip.start + clip.len < s.view_start {
                continue;
            }
            let x = to_x(clip.start);
            let w = (clip.len * s.px_per_sec) as f32;
            // Only the visible part of the waveform is drawn: a clip can be
            // hundreds of thousands of pixels wide when zoomed in.
            let off = (-x).max(0.0).floor() as i32;
            let vis_w = ((x + w).min(width) - x.max(0.0)).ceil().max(1.0) as usize;
            let key = (clip.peaks, zoom_key, off, vis_w);
            let wave = s
                .waves
                .entry(key)
                .or_insert_with(|| s.peaks[clip.peaks].render(off, vis_w, s.px_per_sec))
                .clone();
            clips.push(ClipView {
                x,
                y: y + 2.0,
                w,
                h: TRACK_HEIGHT - 4.0,
                name: SharedString::from(clip.name.as_str()),
                color: clip.color,
                selected: s.selected == Some((ti, ci)),
                wave,
                wave_x: off as f32,
                wave_w: vis_w as f32,
            });
        }
    }
    sync(&m.clips, clips);
    sync(&m.tracks, tracks);
    sync(&m.ticks, ticks);

    ui.set_playhead_x(to_x(s.playhead));
    ui.set_time_text(SharedString::from(format!("{:.2} s", s.playhead)));
    ui.set_playing(s.playing.is_some());
    ui.set_has_selection(s.selected.is_some());
    ui.set_summary(SharedString::from(s.summary()));
    let mut v = s.render_ms.clone();
    v.sort_by(f32::total_cmp);
    ui.set_stats(SharedString::from(format!(
        "{} tracks, {} clips | render: median {:.2} ms, 99th pct {:.2} ms, worst {:.2} ms (budget 16.7 ms at 60 fps)",
        s.tracks.len(),
        s.clip_count(),
        percentile(&v, 0.5),
        percentile(&v, 0.99),
        v.last().copied().unwrap_or(0.0)
    )));
}

/// Creates the native child window over the panel area once the window is
/// up, then keeps it on top of that area as the layout changes.
fn follow_panel(
    ui: &MainWindow,
    panel: &mut Option<embed_panel::Panel>,
    plugin: Option<&std::path::Path>,
) {
    let scale = ui.window().scale_factor();
    let rect = embed_panel::Rect {
        x: (ui.get_panel_x() * scale).round() as i32,
        y: (ui.get_panel_y() * scale).round() as i32,
        w: (ui.get_panel_w() * scale).round() as i32,
        h: (ui.get_panel_h() * scale).round() as i32,
    };
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    match panel {
        Some(p) => p.set_rect(rect),
        None => match embed_panel::Panel::new(&ui.window().window_handle(), rect) {
            Ok(mut p) => {
                let status = match plugin {
                    Some(b) => p.open_plugin(b).unwrap_or_else(|e| e),
                    None => p.status(),
                };
                ui.set_panel_status(status.into());
                *panel = Some(p);
            }
            Err(e) => ui.set_panel_status(format!("no panel: {e}").into()),
        },
    }
}

fn bench_report(b: &Bench, s: &State) -> String {
    let mut ren = b.render.clone();
    ren.sort_by(f32::total_cmp);
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
    let total99 = percentile(&ren, 0.99) + percentile(&lay, 0.99);
    format!(
        "UI timeline benchmark (Slint)\n\
         \x20 {} tracks, {} clips with waveforms, {:.0} s of automatic scroll and zoom\n\
         \x20 frames: {}\n\
         \x20 rendering: median {:.2} ms, 95th {:.2} ms, 99th {:.2} ms, worst {:.2} ms\n\
         \x20 building the frame (our code: visible clips and waveform images): median {:.2} ms, 99th {:.2} ms, worst {:.2} ms\n\
         \x20 frames over 16.7 ms of rendering: {}\n\
         \x20 time between frames: median {:.2} ms, 99th {:.2} ms, worst {:.2} ms ({:.0} fps average)\n\
         \x20 gaps over 25 ms (visible stutter at 60 Hz): {}\n\
         \x20 verdict: {}\n",
        s.tracks.len(),
        s.clip_count(),
        b.seconds,
        b.frames,
        percentile(&ren, 0.5),
        percentile(&ren, 0.95),
        percentile(&ren, 0.99),
        ren.last().copied().unwrap_or(0.0),
        percentile(&lay, 0.5),
        percentile(&lay, 0.99),
        lay.last().copied().unwrap_or(0.0),
        over(&ren, 16.7),
        percentile(&iv, 0.5),
        percentile(&iv, 0.99),
        iv.last().copied().unwrap_or(0.0),
        fps,
        over(&iv, 25.0),
        if total99 < 16.7 && fps >= 55.0 {
            "PASS (60 fps held)"
        } else {
            "FAIL (see numbers above)"
        }
    )
}

fn main() -> Result<(), slint::PlatformError> {
    let mut tracks = 32;
    let mut per = 10;
    let mut bench = None;
    let mut embed: Option<Option<std::path::PathBuf>> = None;
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
                    "usage: ui-timeline-slint [--tracks N] [--clips-per-track N] [--bench SECONDS] [--panel] [--plugin BUNDLE]"
                );
                std::process::exit(2);
            }
        }
    }

    let ui = MainWindow::new()?;
    let state = Rc::new(RefCell::new(State::new(tracks, per, bench)));
    let models = Rc::new(Models {
        clips: Rc::new(VecModel::default()),
        tracks: Rc::new(VecModel::default()),
        ticks: Rc::new(VecModel::default()),
    });
    ui.set_clips(ModelRc::from(models.clips.clone()));
    ui.set_tracks(ModelRc::from(models.tracks.clone()));
    ui.set_ticks(ModelRc::from(models.ticks.clone()));
    ui.set_track_height(TRACK_HEIGHT);
    ui.set_show_panel(embed.is_some());
    ui.set_panel_status("starting".into());
    let panel: Rc<RefCell<Option<embed_panel::Panel>>> = Rc::new(RefCell::new(None));

    // Every callback changes the state, then refreshes the window.
    let wire = |f: Handler| {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        move || {
            let ui = w.unwrap();
            let mut st = s.borrow_mut();
            f(&ui, &mut st);
            refresh(&ui, &mut st, &m);
        }
    };
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    ui.on_toggle_play(wire(Box::new(|_, s| s.toggle_play())));
    ui.on_delete(wire(Box::new(|_, s| s.delete_selected())));
    ui.on_rename(wire(Box::new(|ui, s| {
        if let Some((t, c)) = s.selected {
            s.renaming = Some((t, c));
            ui.set_rename_text(SharedString::from(s.tracks[t].clips[c].name.as_str()));
            ui.set_renaming(true);
        }
    })));
    ui.on_rename_cancel(wire(Box::new(|ui, s| {
        s.renaming = None;
        ui.set_renaming(false);
    })));
    {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        ui.on_rename_done(move |text| {
            let ui = w.unwrap();
            let mut st = s.borrow_mut();
            if let Some((t, c)) = st.renaming.take()
                && let Some(clip) = st.tracks.get_mut(t).and_then(|tr| tr.clips.get_mut(c))
            {
                clip.name = text.to_string();
            }
            ui.set_renaming(false);
            refresh(&ui, &mut st, &m);
        });
    }
    {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        ui.on_zoom(move |f| {
            let ui = w.unwrap();
            let mut st = s.borrow_mut();
            st.zoom_at(f as f64, ui.get_area_width() / 2.0);
            refresh(&ui, &mut st, &m);
        });
    }
    {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        ui.on_scrolled(move |dx, dy, ctrl, x| {
            let ui = w.unwrap();
            let mut st = s.borrow_mut();
            if ctrl {
                st.zoom_at(if dy > 0.0 { 1.1 } else { 1.0 / 1.1 }, x);
            } else {
                st.view_start = (st.view_start - dx as f64 / st.px_per_sec).max(0.0);
                st.scroll_y -= dy;
            }
            refresh(&ui, &mut st, &m);
        });
    }
    {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        ui.on_press(move |x, y| {
            let ui = w.unwrap();
            let mut st = s.borrow_mut();
            st.selected = st.hit(x, y);
            st.dragging = st
                .selected
                .map(|(t, c)| (t, c, st.time_at(x) - st.tracks[t].clips[c].start));
            refresh(&ui, &mut st, &m);
        });
    }
    {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        ui.on_drag(move |x, _y| {
            let ui = w.unwrap();
            let mut st = s.borrow_mut();
            if let Some((t, c, grab)) = st.dragging {
                let at = st.time_at(x);
                // Snap to 0.25 s, as in the egui version.
                st.tracks[t].clips[c].start = (((at - grab) * 4.0).round() / 4.0).max(0.0);
                refresh(&ui, &mut st, &m);
            }
        });
    }
    ui.on_release(wire(Box::new(|_, s| s.dragging = None)));
    {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        ui.on_double_click(move |x, y| {
            let ui = w.unwrap();
            let mut st = s.borrow_mut();
            if let Some((t, c)) = st.hit(x, y) {
                st.selected = Some((t, c));
                st.renaming = Some((t, c));
                ui.set_rename_text(SharedString::from(st.tracks[t].clips[c].name.as_str()));
                ui.set_renaming(true);
            }
            refresh(&ui, &mut st, &m);
        });
    }

    // Render timing, around Slint's own renderer.
    {
        let s = state.clone();
        ui.window()
            .set_rendering_notifier(move |rs, _| {
                let Ok(mut st) = s.try_borrow_mut() else {
                    return;
                };
                match rs {
                    slint::RenderingState::BeforeRendering => {
                        st.render_start = Some(Instant::now());
                    }
                    slint::RenderingState::AfterRendering => {
                        let now = Instant::now();
                        if let Some(t0) = st.render_start.take() {
                            let ms = now.duration_since(t0).as_secs_f32() * 1000.0;
                            st.render_ms.push(ms);
                            if st.render_ms.len() > 240 {
                                st.render_ms.remove(0);
                            }
                            if let Some(b) = &mut st.bench
                                && b.started.is_some()
                            {
                                b.frames += 1;
                                b.render.push(ms);
                                if let Some(last) = b.last.replace(now) {
                                    b.intervals
                                        .push(now.duration_since(last).as_secs_f32() * 1000.0);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            })
            .ok();
    }

    // Playhead, benchmark motion and the first layout all run on a timer
    // that ticks at the display rate.
    let timer = Timer::default();
    {
        let (w, s, m) = (ui.as_weak(), state.clone(), models.clone());
        let panel = panel.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(8), move || {
            let ui = w.unwrap();
            if let Some(plugin) = &embed {
                follow_panel(&ui, &mut panel.borrow_mut(), plugin.as_deref());
            }
            let mut st = s.borrow_mut();
            let now = Instant::now();
            let mut done = false;
            let tracks_h = st.tracks.len() as f32 * TRACK_HEIGHT;
            if let Some(b) = &mut st.bench {
                let started = *b.started.get_or_insert(now);
                let el = now.duration_since(started).as_secs_f64();
                done = el >= b.seconds;
                // The same motion as the egui benchmark.
                st.px_per_sec = 40.0 * (2.0f64).powf(3.0 * (el * 0.4).sin());
                st.view_start = (60.0 * (1.0 - (el * 0.3).cos())).max(0.0);
                st.scroll_y = (tracks_h * 0.5) * (1.0 - (el as f32 * 0.5).cos()) / 2.0;
            }
            if let Some((t0, from)) = st.playing {
                st.playhead = from + t0.elapsed().as_secs_f64();
            }
            refresh(&ui, &mut st, &m);
            if let Some(b) = &mut st.bench {
                b.layout.push(now.elapsed().as_secs_f32() * 1000.0);
            }
            if done {
                let b = st.bench.take().unwrap();
                let report = bench_report(&b, &st);
                print!("{report}");
                let _ = std::fs::write("ui-slint-bench-report.txt", &report);
                let _ = slint::quit_event_loop();
            }
        });
    }

    ui.run()
}
