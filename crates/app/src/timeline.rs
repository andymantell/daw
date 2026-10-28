// SPDX-License-Identifier: GPL-3.0-or-later
//! The timeline (roadmap 1.5): each track's clips on a lane, with their
//! waveforms. Select, move, trim, split and delete clips, snapped to
//! beats or bars; markers on bar lines name the song's sections, and a
//! section can be looped. Undo belongs to the project document; the
//! timeline only asks for it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use daw_engine::stream::StreamSource;
use daw_engine::time::{Frames, TempoMap, Ticks};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, pos2, vec2,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Part of a file placed on a track.
#[derive(Clone, Debug)]
pub struct Clip {
    /// Unique for the life of the process; selection follows it.
    pub id: u64,
    /// The clip's id in the project document.
    pub key: String,
    /// Its audio in the project document: an asset's name once saved,
    /// the file's full path before.
    pub asset: String,
    pub source: Arc<StreamSource>,
    /// The file as given: the original of a converted copy, or the take.
    /// Opened again if the device changes rate.
    pub path: PathBuf,
    /// What the file was called when it came in; the saved copy is named
    /// by its content.
    pub label: String,
    /// Where it plays on the timeline, in frames.
    pub start: i64,
    /// The first frame of the source that plays.
    pub offset: usize,
    /// How many frames play.
    pub len: usize,
    /// Which recorded take this is part of (from 0); `None` for a file.
    pub take: Option<usize>,
    /// Out of the mix: an older take under a newer one.
    pub muted: bool,
}

/// The shortest clip a trim or split leaves, in frames.
pub const MIN_LEN: usize = 64;

impl Clip {
    /// All of `source`, starting at `start`. Anything before the start of
    /// the timeline is trimmed off.
    pub fn new(source: Arc<StreamSource>, path: PathBuf, start: i64, take: Option<usize>) -> Clip {
        let mut c = Clip {
            id: next_id(),
            key: daw_project::new_id(),
            asset: path.to_string_lossy().into_owned(),
            label: path
                .file_stem()
                .map_or(String::new(), |s| s.to_string_lossy().into_owned()),
            len: source.frames,
            source,
            path,
            start,
            offset: 0,
            take,
            muted: false,
        };
        if c.start < 0 {
            let cut = ((-c.start) as usize).min(c.len);
            c.offset = cut;
            c.len -= cut;
            c.start = 0;
        }
        c
    }

    pub fn end(&self) -> i64 {
        self.start + self.len as i64
    }

    fn overlaps(&self, other: &Clip) -> bool {
        self.start < other.end() && other.start < self.end()
    }

    /// Moves the start edge to `to`, keeping the audio where it is: as
    /// far back as the source goes (and no earlier than the timeline's
    /// start), and up to [`MIN_LEN`] short of the end.
    pub fn trim_start(&mut self, to: i64) {
        let lo = (self.start - self.offset as i64).max(0);
        let hi = self.end() - MIN_LEN as i64;
        if hi < lo {
            return;
        }
        let d = to.clamp(lo, hi) - self.start;
        self.offset = (self.offset as i64 + d) as usize;
        self.len = (self.len as i64 - d) as usize;
        self.start += d;
    }

    /// Moves the end edge to `to`, as far as the source goes.
    pub fn trim_end(&mut self, to: i64) {
        let most = self.source.frames - self.offset;
        self.len = ((to - self.start).max(MIN_LEN as i64) as usize).min(most);
    }

    /// Cuts in two at timeline frame `at`; `None` if either part would be
    /// shorter than [`MIN_LEN`].
    pub fn split(&self, at: i64) -> Option<(Clip, Clip)> {
        if at - self.start < MIN_LEN as i64 || self.end() - at < MIN_LEN as i64 {
            return None;
        }
        let first = (at - self.start) as usize;
        let mut a = self.clone();
        a.len = first;
        let mut b = self.clone();
        b.id = next_id();
        b.key = daw_project::new_id();
        b.start = at;
        b.offset += first;
        b.len -= first;
        Some((a, b))
    }

    /// Swaps in the same file opened at another rate, scaling positions
    /// to match.
    pub fn reopened(&mut self, source: Arc<StreamSource>) {
        let k = source.rate as f64 / self.source.rate as f64;
        self.start = (self.start as f64 * k).round() as i64;
        self.offset = ((self.offset as f64 * k).round() as usize).min(source.frames);
        self.len = ((self.len as f64 * k).round() as usize).min(source.frames - self.offset);
        self.source = source;
    }

    pub fn name(&self) -> String {
        match self.take {
            Some(t) => format!("Take {}", t + 1),
            None => self.label.clone(),
        }
    }
}

/// Adds a newly recorded take to a track's clips, on top: older takes
/// under it are muted.
pub fn add_take(clips: &mut Vec<Clip>, mut c: Clip) {
    c.take = Some(take_count(clips));
    for o in clips.iter_mut() {
        if o.take.is_some() && o.overlaps(&c) {
            o.muted = true;
        }
    }
    clips.push(c);
}

/// How many takes have been recorded on the track.
pub fn take_count(clips: &[Clip]) -> usize {
    clips
        .iter()
        .filter_map(|c| c.take)
        .max()
        .map_or(0, |t| t + 1)
}

/// The newest take that plays, if any does.
pub fn active_take(clips: &[Clip]) -> Option<usize> {
    clips
        .iter()
        .filter(|c| !c.muted)
        .filter_map(|c| c.take)
        .max()
}

/// Plays take `which` where it was recorded, muting other takes under it;
/// `None` mutes every take. Imported files are left alone.
pub fn choose_take(clips: &mut [Clip], which: Option<usize>) {
    let chosen: Vec<(i64, i64)> = clips
        .iter()
        .filter(|c| which.is_some() && c.take == which)
        .map(|c| (c.start, c.end()))
        .collect();
    for c in clips.iter_mut().filter(|c| c.take.is_some()) {
        if c.take == which {
            c.muted = false;
        } else if which.is_none() || chosen.iter().any(|&(s, e)| c.start < e && s < c.end()) {
            c.muted = true;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Snap {
    Off,
    #[default]
    Beat,
    Bar,
}

impl Snap {
    fn label(self) -> &'static str {
        match self {
            Snap::Off => "Off",
            Snap::Beat => "Beat",
            Snap::Bar => "Bar",
        }
    }
}

/// `frames` moved to the nearest beat or bar line.
pub fn snap(map: &TempoMap, frames: i64, snap: Snap) -> i64 {
    let t = map.ticks_at(Frames(frames));
    let bb = map.bar_beat(t);
    let bar = map.bar_start(bb.bar);
    let ticks = match snap {
        Snap::Off => return frames,
        Snap::Beat => {
            let beat = map.signature_at(t).beat_ticks();
            bar.0 + ((t.0 - bar.0) as f64 / beat as f64).round() as i64 * beat
        }
        Snap::Bar => {
            let next = map.bar_start(bb.bar + 1);
            if t.0 - bar.0 < next.0 - t.0 {
                bar.0
            } else {
                next.0
            }
        }
    };
    map.frames_at(Ticks(ticks)).0
}

/// A named point on a bar line: the start of a section, which runs to
/// the next marker.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: String,
    /// Counted from 1.
    pub bar: i64,
    pub name: String,
}

/// Adds a marker at `bar`, keeping them in order. False if there is one
/// there already.
pub fn add_marker(markers: &mut Vec<Marker>, bar: i64) -> bool {
    let bar = bar.max(1);
    match markers.binary_search_by_key(&bar, |m| m.bar) {
        Ok(_) => false,
        Err(i) => {
            markers.insert(
                i,
                Marker {
                    id: daw_project::new_id(),
                    bar,
                    name: section_name(markers.len()),
                },
            );
            true
        }
    }
}

/// "Section A", "Section B", … for new markers, until renamed.
fn section_name(n: usize) -> String {
    let letter = (b'A' + (n % 26) as u8) as char;
    format!("Section {letter}")
}

/// The bars marker `i`'s section covers, as a loop: from its bar to the
/// next marker's, or to `end_bar` after the last.
pub fn section(markers: &[Marker], i: usize, end_bar: i64) -> (i64, i64) {
    let from = markers[i].bar;
    let to = markers.get(i + 1).map_or(end_bar, |m| m.bar);
    (from, to.max(from + 1))
}

/// One track as the timeline sees it.
pub struct Lane<'a> {
    pub name: &'a str,
    pub clips: &'a mut Vec<Clip>,
}

/// What the timeline edits: borrowed from the app for one frame.
pub struct Song<'a> {
    pub lanes: Vec<Lane<'a>>,
    pub markers: &'a mut Vec<Marker>,
}

impl Song<'_> {
    fn find_mut(&mut self, id: u64) -> Option<&mut Clip> {
        self.lanes
            .iter_mut()
            .flat_map(|l| l.clips.iter_mut())
            .find(|c| c.id == id)
    }

    /// The end of the last clip, in frames.
    pub fn end(&self) -> i64 {
        self.lanes
            .iter()
            .flat_map(|l| l.clips.iter())
            .map(Clip::end)
            .max()
            .unwrap_or(0)
    }
}

/// Where things are, for one frame.
pub struct View<'a> {
    pub map: &'a TempoMap,
    pub rate: u32,
    pub playhead: i64,
    pub playing: bool,
    pub loop_bars: Option<(i64, i64)>,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// What the app should do after the timeline is drawn.
#[derive(Default)]
pub struct Outcome {
    /// Clips changed: rebuild the graph.
    pub changed: bool,
    pub seek: Option<i64>,
    /// Loop these bars.
    pub loop_bars: Option<(i64, i64)>,
    pub undo: bool,
    pub redo: bool,
}

/// Frames between waveform peaks at the finest level.
const BASE_HOP: usize = 256;

/// A file's peaks at successively halved resolutions.
struct Waveform {
    levels: Vec<Vec<(f32, f32)>>,
}

impl Waveform {
    fn new(base: Vec<(f32, f32)>) -> Waveform {
        let mut levels = vec![base];
        while levels.last().is_some_and(|l| l.len() > 1) {
            let next = levels[levels.len() - 1]
                .chunks(2)
                .map(|c| {
                    c.iter()
                        .fold((0.0f32, 0.0f32), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)))
                })
                .collect();
            levels.push(next);
        }
        Waveform { levels }
    }

    /// Lowest and highest sample in source frames `from..to`.
    fn range(&self, from: usize, to: usize) -> (f32, f32) {
        let span = to.saturating_sub(from).max(1);
        // The coarsest level still finer than the span.
        let mut l = 0;
        while l + 1 < self.levels.len() && BASE_HOP << (l + 1) <= span {
            l += 1;
        }
        let hop = BASE_HOP << l;
        let level = &self.levels[l];
        let (i0, i1) = (from / hop, to.div_ceil(hop).max(from / hop + 1));
        level[i0.min(level.len())..i1.min(level.len())]
            .iter()
            .fold((0.0, 0.0), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)))
    }
}

/// Waveforms by streamed file, worked out in the background.
#[derive(Default)]
struct Waveforms {
    ready: HashMap<PathBuf, Arc<Waveform>>,
    pending: HashMap<PathBuf, mpsc::Receiver<Option<Waveform>>>,
}

impl Waveforms {
    fn get(&mut self, src: &Arc<StreamSource>) -> Option<Arc<Waveform>> {
        if let Some(w) = self.ready.get(&src.path) {
            return Some(w.clone());
        }
        if !self.pending.contains_key(&src.path) {
            let (tx, rx) = mpsc::channel();
            let s = src.clone();
            std::thread::spawn(move || {
                let _ = tx.send(s.peaks(BASE_HOP).ok().map(Waveform::new));
            });
            self.pending.insert(src.path.clone(), rx);
        }
        None
    }

    /// Takes in finished waveforms. True while any are still coming.
    fn poll(&mut self) -> bool {
        let mut done = Vec::new();
        for (path, rx) in &self.pending {
            match rx.try_recv() {
                Ok(w) => done.push((path.clone(), w)),
                Err(mpsc::TryRecvError::Disconnected) => done.push((path.clone(), None)),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for (path, w) in done {
            self.pending.remove(&path);
            // A file that can't be read draws as a plain box.
            let w = w.unwrap_or_else(|| Waveform::new(Vec::new()));
            self.ready.insert(path, Arc::new(w));
        }
        !self.pending.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Zone {
    Body,
    StartEdge,
    EndEdge,
}

#[derive(Clone, Copy)]
enum Hit {
    Ruler,
    Marker(usize),
    Clip { id: u64, zone: Zone },
    Lane,
}

struct Drag {
    zone: Zone,
    /// The clip held.
    anchor: u64,
    /// Timeline frame under the pointer when pressed.
    from: i64,
    /// The clips being dragged, as they were.
    originals: Vec<Clip>,
    moved: bool,
}

pub struct Timeline {
    /// Zoom.
    px_per_sec: f32,
    /// Seconds at the left edge of the lanes.
    scroll: f64,
    /// Points scrolled down the lanes.
    vscroll: f32,
    pub snap: Snap,
    pub selected: HashSet<u64>,
    drag: Option<Drag>,
    /// Scrubbing the playhead along the ruler.
    scrubbing: bool,
    waves: Waveforms,
    /// What was right-clicked, and where (timeline frame).
    menu: Option<(Hit, i64)>,
    /// A marker being renamed: its index and the text so far.
    rename: Option<(usize, String)>,
    /// The rename field has been given the keyboard.
    rename_focused: bool,
}

impl Default for Timeline {
    fn default() -> Timeline {
        Timeline {
            px_per_sec: 40.0,
            scroll: 0.0,
            vscroll: 0.0,
            snap: Snap::default(),
            selected: HashSet::new(),
            drag: None,
            scrubbing: false,
            waves: Waveforms::default(),
            menu: None,
            rename: None,
            rename_focused: false,
        }
    }
}

/// Track names to the left of the lanes.
const HEADER: f32 = 110.0;
const RULER: f32 = 26.0;
/// Edge grab width for trimming, in points.
const EDGE: f32 = 6.0;

const COLOURS: [Color32; 6] = [
    Color32::from_rgb(70, 130, 200),
    Color32::from_rgb(200, 120, 60),
    Color32::from_rgb(90, 170, 100),
    Color32::from_rgb(170, 90, 170),
    Color32::from_rgb(190, 170, 70),
    Color32::from_rgb(70, 170, 170),
];

impl Timeline {
    /// Splits the selected clips at `at`, or, with none selected, every
    /// playing clip there. True if any were split.
    pub fn split_at(&mut self, song: &mut Song, at: i64) -> bool {
        let mut any = false;
        let all = self.selected.is_empty();
        for lane in &mut song.lanes {
            let mut i = 0;
            while i < lane.clips.len() {
                let c = &lane.clips[i];
                let wanted = if all {
                    !c.muted
                } else {
                    self.selected.contains(&c.id)
                };
                if wanted && let Some((a, b)) = c.split(at) {
                    if self.selected.contains(&a.id) {
                        self.selected.insert(b.id);
                    }
                    lane.clips[i] = a;
                    lane.clips.insert(i + 1, b);
                    any = true;
                    i += 1;
                }
                i += 1;
            }
        }
        any
    }

    /// Removes the selected clips. True if any were.
    pub fn delete_selected(&mut self, song: &mut Song) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        let mut any = false;
        for lane in &mut song.lanes {
            let n = lane.clips.len();
            lane.clips.retain(|c| !self.selected.contains(&c.id));
            any |= lane.clips.len() != n;
        }
        self.selected.clear();
        any
    }

    pub fn add_marker(&mut self, song: &mut Song, bar: i64) {
        add_marker(song.markers, bar);
    }

    /// The bar the playhead is in.
    fn bar_at(map: &TempoMap, frames: i64) -> i64 {
        map.bar_beat(map.ticks_at(Frames(frames))).bar
    }

    /// The first bar after the song's end.
    fn end_bar(map: &TempoMap, song: &Song) -> i64 {
        Self::bar_at(map, song.end()) + 1
    }

    /// Keyboard shortcuts, when no text field has the keyboard.
    fn keys(&mut self, ctx: &egui::Context, song: &mut Song, view: &View, out: &mut Outcome) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        if ctx.input_mut(|i| {
            i.consume_key(shift_cmd, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y)
        }) {
            out.redo = true;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            out.undo = true;
        }
        if ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Delete)
                || i.consume_key(Modifiers::NONE, Key::Backspace)
        }) {
            out.changed |= self.delete_selected(song);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::S)) {
            out.changed |= self.split_at(song, view.playhead);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::M)) {
            self.add_marker(song, Self::bar_at(view.map, view.playhead));
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.selected.clear();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, song: &mut Song, view: &View, out: &mut Outcome) {
        ui.horizontal(|ui| {
            ui.label("Snap");
            egui::ComboBox::from_id_salt("snap")
                .width(60.0)
                .selected_text(self.snap.label())
                .show_ui(ui, |ui| {
                    for s in [Snap::Off, Snap::Beat, Snap::Bar] {
                        ui.selectable_value(&mut self.snap, s, s.label());
                    }
                })
                .response
                .accessible_name("Snap")
                .on_hover_text("Hold Alt while dragging to place freely");
            ui.separator();
            if ui
                .button("−")
                .accessible_name("Zoom out")
                .on_hover_text("Zoom out (Ctrl + wheel)")
                .clicked()
            {
                self.px_per_sec = (self.px_per_sec / 1.5).max(1.0);
            }
            if ui
                .button("+")
                .accessible_name("Zoom in")
                .on_hover_text("Zoom in (Ctrl + wheel)")
                .clicked()
            {
                self.px_per_sec = (self.px_per_sec * 1.5).min(4000.0);
            }
            if ui
                .button("Fit")
                .on_hover_text("Show the whole song")
                .clicked()
            {
                let secs = (song.end() as f64 / view.rate as f64).max(10.0);
                let w = (ui.available_width() + ui.min_rect().width() - HEADER).max(200.0);
                self.px_per_sec = (w as f64 / (secs * 1.05)) as f32;
                self.scroll = 0.0;
            }
            ui.separator();
            if ui
                .button("✂ Split")
                .on_hover_text("Split the selected clips, or all of them, at the playhead (S)")
                .clicked()
            {
                out.changed |= self.split_at(song, view.playhead);
            }
            if ui
                .add_enabled(!self.selected.is_empty(), egui::Button::new("🗑 Delete"))
                .on_hover_text("Delete the selected clips (Delete)")
                .clicked()
            {
                out.changed |= self.delete_selected(song);
            }
            if ui
                .button("⚑ Marker")
                .on_hover_text("Mark a section at the playhead's bar (M)")
                .clicked()
            {
                self.add_marker(song, Self::bar_at(view.map, view.playhead));
            }
            ui.separator();
            if ui
                .add_enabled(view.can_undo, egui::Button::new("↶"))
                .accessible_name("Undo")
                .on_hover_text("Undo (Ctrl+Z)")
                .clicked()
            {
                out.undo = true;
            }
            if ui
                .add_enabled(view.can_redo, egui::Button::new("↷"))
                .accessible_name("Redo")
                .on_hover_text("Redo (Ctrl+Shift+Z)")
                .clicked()
            {
                out.redo = true;
            }
        });
    }

    /// Draws the timeline and handles editing. Fills the space given.
    pub fn show(&mut self, ui: &mut egui::Ui, song: &mut Song, view: &View) -> Outcome {
        let mut out = Outcome::default();
        if self.waves.poll() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.keys(&ui.ctx().clone(), song, view, &mut out);
        self.toolbar(ui, song, view, &mut out);

        let size = vec2(
            ui.available_width(),
            ui.available_height().max(RULER + 40.0),
        );
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let lanes_n = song.lanes.len().max(1);
        let lane_h = ((rect.height() - RULER) / lanes_n as f32).clamp(36.0, 80.0);
        // Lanes that don't fit scroll up and down.
        let overflow = (lanes_n as f32 * lane_h - (rect.height() - RULER)).max(0.0);
        let left = rect.left() + HEADER;
        let rate = view.rate as f64;
        let pps = self.px_per_sec as f64;

        // Wheel scrolls, Ctrl + wheel zooms about the pointer.
        if let Some(p) = resp.hover_pos() {
            let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
            if zoom != 1.0 {
                let at = self.scroll + (p.x - left) as f64 / pps;
                self.px_per_sec = (self.px_per_sec * zoom).clamp(1.0, 4000.0);
                self.scroll = (at - (p.x - left) as f64 / self.px_per_sec as f64).max(0.0);
            } else if scroll != egui::Vec2::ZERO {
                // The wheel scrolls the lanes if they overflow, or else
                // along; Shift + wheel (or a sideways wheel) along.
                let mut along = scroll.x;
                if overflow > 0.0 {
                    self.vscroll -= scroll.y;
                } else {
                    along += scroll.y;
                }
                self.scroll = (self.scroll - along as f64 / pps).max(0.0);
            }
        }
        self.vscroll = self.vscroll.clamp(0.0, overflow);
        let vscroll = self.vscroll;
        let pps = self.px_per_sec as f64;
        let scroll = self.scroll;
        let x_of = move |f: i64| left + ((f as f64 / rate - scroll) * pps) as f32;
        let frame_of = move |x: f32| ((scroll + (x - left) as f64 / pps) * rate).round() as i64;
        let lane_top = |i: usize| rect.top() + RULER + i as f32 * lane_h - vscroll;

        // Keep the playhead in view while playing.
        if view.playing && self.drag.is_none() {
            let x = x_of(view.playhead);
            if x > rect.right() - 20.0 || x < left {
                self.scroll =
                    view.playhead as f64 / rate - 0.1 * (rect.width() - HEADER) as f64 / pps;
                self.scroll = self.scroll.max(0.0);
                ui.ctx().request_repaint();
            }
        }

        let hit = |pos: Pos2, song: &Song| -> Option<Hit> {
            if !rect.contains(pos) || pos.x < left {
                return None;
            }
            if pos.y < rect.top() + RULER {
                for (i, m) in song.markers.iter().enumerate() {
                    let x = x_of(view.map.frames_at(view.map.bar_start(m.bar)).0);
                    if (pos.x - x).abs() < 5.0
                        || (pos.x > x && pos.x < x + 70.0 && pos.y < rect.top() + 14.0)
                    {
                        return Some(Hit::Marker(i));
                    }
                }
                return Some(Hit::Ruler);
            }
            let lane = ((pos.y - rect.top() - RULER + vscroll) / lane_h) as usize;
            let clips = &song.lanes.get(lane)?.clips;
            // Playing clips over muted ones; later ones on top.
            let order = clips
                .iter()
                .rev()
                .filter(|c| !c.muted)
                .chain(clips.iter().rev().filter(|c| c.muted));
            for c in order {
                let (x0, x1) = (x_of(c.start), x_of(c.end()));
                if pos.x >= x0 && pos.x <= x1 {
                    let edge = EDGE.min((x1 - x0) / 3.0);
                    let zone = if pos.x - x0 < edge {
                        Zone::StartEdge
                    } else if x1 - pos.x < edge {
                        Zone::EndEdge
                    } else {
                        Zone::Body
                    };
                    return Some(Hit::Clip { id: c.id, zone });
                }
            }
            Some(Hit::Lane)
        };

        // Pointer: cursors, selection, dragging.
        let modifiers = ui.input(|i| i.modifiers);
        let grid = self.snap;
        let snap_to = |f: i64| {
            if modifiers.alt {
                f
            } else {
                snap(view.map, f, grid)
            }
        };
        if let Some(p) = resp.hover_pos()
            && self.drag.is_none()
        {
            match hit(p, song) {
                Some(Hit::Clip {
                    zone: Zone::StartEdge | Zone::EndEdge,
                    ..
                }) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                Some(Hit::Clip { .. }) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                _ => {}
            }
        }
        // Where the press began, not where the drag was noticed.
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            match hit(p, song) {
                Some(Hit::Clip { id, zone }) => {
                    if !self.selected.contains(&id) {
                        if !(modifiers.shift || modifiers.command) {
                            self.selected.clear();
                        }
                        self.selected.insert(id);
                    }
                    let held: Vec<Clip> = song
                        .lanes
                        .iter()
                        .flat_map(|l| l.clips.iter())
                        .filter(|c| {
                            if zone == Zone::Body {
                                self.selected.contains(&c.id)
                            } else {
                                c.id == id
                            }
                        })
                        .cloned()
                        .collect();
                    self.drag = Some(Drag {
                        zone,
                        anchor: id,
                        from: frame_of(p.x),
                        originals: held,
                        moved: false,
                    });
                }
                Some(Hit::Ruler | Hit::Lane) if p.y < rect.top() + RULER => self.scrubbing = true,
                _ => {}
            }
        }
        if self.scrubbing
            && let Some(p) = resp.interact_pointer_pos()
        {
            out.seek = Some(snap_to(frame_of(p.x.max(left))).max(0));
        }
        if let Some(d) = &mut self.drag
            && let Some(p) = resp.interact_pointer_pos()
        {
            let dx = frame_of(p.x) - d.from;
            if let Some(anchor) = d.originals.iter().find(|c| c.id == d.anchor).cloned() {
                match d.zone {
                    Zone::Body => {
                        // The held clip lands on the grid; the rest keep
                        // their distance from it.
                        let lowest = d.originals.iter().map(|c| c.start).min().unwrap_or(0);
                        let delta = (snap_to(anchor.start + dx) - anchor.start).max(-lowest);
                        for o in &d.originals {
                            if let Some(c) = song.find_mut(o.id) {
                                d.moved |= c.start != o.start + delta;
                                c.start = o.start + delta;
                            }
                        }
                    }
                    Zone::StartEdge => {
                        if let Some(c) = song.find_mut(anchor.id) {
                            let mut t = anchor.clone();
                            t.trim_start(snap_to(anchor.start + dx));
                            d.moved |= (c.start, c.len) != (t.start, t.len);
                            *c = t;
                        }
                    }
                    Zone::EndEdge => {
                        if let Some(c) = song.find_mut(anchor.id) {
                            let mut t = anchor.clone();
                            t.trim_end(snap_to(anchor.end() + dx));
                            d.moved |= c.len != t.len;
                            *c = t;
                        }
                    }
                }
            }
        }
        if resp.drag_stopped() {
            self.scrubbing = false;
            if let Some(d) = self.drag.take()
                && d.moved
            {
                out.changed = true;
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            match hit(p, song) {
                Some(Hit::Clip { id, .. }) => {
                    if modifiers.shift || modifiers.command {
                        if !self.selected.remove(&id) {
                            self.selected.insert(id);
                        }
                    } else {
                        self.selected.clear();
                        self.selected.insert(id);
                    }
                }
                Some(Hit::Marker(i)) => {
                    // To the start of its section.
                    let bar = view.map.bar_start(song.markers[i].bar);
                    out.seek = Some(view.map.frames_at(bar).0);
                }
                Some(Hit::Ruler | Hit::Lane) => {
                    self.selected.clear();
                    out.seek = Some(snap_to(frame_of(p.x)).max(0));
                }
                None => {}
            }
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(Hit::Marker(i)) = hit(p, song)
        {
            self.rename = Some((i, song.markers[i].name.clone()));
            self.rename_focused = false;
        }
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.menu = hit(p, song).map(|h| (h, frame_of(p.x).max(0)));
            if let Some((Hit::Clip { id, .. }, _)) = self.menu
                && !self.selected.contains(&id)
            {
                self.selected.clear();
                self.selected.insert(id);
            }
        }

        // Drawing.
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
        let ruler = Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(rect.right(), rect.top() + RULER),
        );
        painter.rect_filled(ruler, 0.0, visuals.faint_bg_color);

        // Loop range.
        if let Some((a, b)) = view.loop_bars {
            let fa = view.map.frames_at(view.map.bar_start(a)).0;
            let fb = view.map.frames_at(view.map.bar_start(b)).0;
            let r = Rect::from_x_y_ranges(
                x_of(fa).max(left)..=x_of(fb).max(left),
                rect.top() + 16.0..=rect.top() + RULER,
            );
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(80, 160, 240, 90));
        }

        // Bar lines, thinned out when zoomed out.
        let bar_px = {
            let b1 = view.map.frames_at(view.map.bar_start(1)).0;
            let b2 = view.map.frames_at(view.map.bar_start(2)).0;
            (x_of(b2) - x_of(b1)).max(0.1)
        };
        let every = [1, 2, 4, 8, 16, 32, 64]
            .into_iter()
            .find(|&n| bar_px * n as f32 >= 40.0)
            .unwrap_or(128);
        let first = Self::bar_at(view.map, frame_of(left).max(0));
        let mut bar = (first - 1) / every * every + 1;
        loop {
            let x = x_of(view.map.frames_at(view.map.bar_start(bar)).0);
            if x > rect.right() {
                break;
            }
            if x >= left {
                painter.line_segment(
                    [pos2(x, rect.top() + 12.0), pos2(x, rect.bottom())],
                    Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
                );
                painter.text(
                    pos2(x + 3.0, rect.top() + RULER - 2.0),
                    Align2::LEFT_BOTTOM,
                    bar.to_string(),
                    FontId::proportional(11.0),
                    visuals.weak_text_color(),
                );
            }
            bar += every;
        }

        // Lanes and clips, under the ruler.
        let lanes = painter.with_clip_rect(Rect::from_min_max(
            pos2(rect.left(), rect.top() + RULER),
            rect.max,
        ));
        for (i, lane) in song.lanes.iter().enumerate() {
            let top = lane_top(i);
            let name_rect = Rect::from_min_size(pos2(rect.left(), top), vec2(HEADER, lane_h));
            lanes.rect_filled(name_rect, 0.0, visuals.faint_bg_color);
            lanes.text(
                name_rect.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                lane.name,
                FontId::proportional(13.0),
                visuals.text_color(),
            );
            lanes.line_segment(
                [
                    pos2(rect.left(), top + lane_h),
                    pos2(rect.right(), top + lane_h),
                ],
                Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
            );
            let colour = COLOURS[i % COLOURS.len()];
            let order = lane
                .clips
                .iter()
                .filter(|c| c.muted)
                .chain(lane.clips.iter().filter(|c| !c.muted));
            for c in order {
                let (x0, x1) = (x_of(c.start), x_of(c.end()));
                if x1 < left || x0 > rect.right() {
                    continue;
                }
                let r = Rect::from_x_y_ranges(
                    x0.max(left)..=x1.min(rect.right()),
                    top + 2.0..=top + lane_h - 2.0,
                );
                let selected = self.selected.contains(&c.id);
                if c.muted {
                    lanes.rect_stroke(
                        r,
                        3.0,
                        Stroke::new(1.0, colour.gamma_multiply(0.5)),
                        StrokeKind::Inside,
                    );
                } else {
                    lanes.rect_filled(r, 3.0, colour.gamma_multiply(0.35));
                }
                if let Some(w) = self.waves.get(&c.source) {
                    let mid = r.center().y + 6.0;
                    let half = (r.height() - 16.0) / 2.0;
                    let wave = if c.muted {
                        colour.gamma_multiply(0.4)
                    } else {
                        colour
                    };
                    let mut x = r.left().floor();
                    while x < r.right() {
                        let f0 = frame_of(x) - c.start + c.offset as i64;
                        let f1 = frame_of(x + 1.0) - c.start + c.offset as i64;
                        let lo_src = c.offset as i64;
                        let hi_src = (c.offset + c.len) as i64;
                        let (a, b) = (
                            f0.clamp(lo_src, hi_src) as usize,
                            f1.clamp(lo_src, hi_src) as usize,
                        );
                        if b > a {
                            let (lo, hi) = w.range(a, b);
                            lanes.line_segment(
                                [
                                    pos2(x + 0.5, mid - hi.min(1.0) * half),
                                    pos2(x + 0.5, mid - lo.max(-1.0) * half - 0.5),
                                ],
                                Stroke::new(1.0, wave),
                            );
                        }
                        x += 1.0;
                    }
                }
                lanes.text(
                    r.left_top() + vec2(4.0, 2.0),
                    Align2::LEFT_TOP,
                    c.name(),
                    FontId::proportional(11.0),
                    if c.muted {
                        visuals.weak_text_color()
                    } else {
                        visuals.strong_text_color()
                    },
                );
                if selected {
                    lanes.rect_stroke(r, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
                }
            }
        }
        if song.lanes.is_empty() {
            painter.text(
                pos2(left + 10.0, rect.top() + RULER + 20.0),
                Align2::LEFT_CENTER,
                "No tracks yet",
                FontId::proportional(13.0),
                visuals.weak_text_color(),
            );
        }

        // Markers, over everything.
        for m in song.markers.iter() {
            let x = x_of(view.map.frames_at(view.map.bar_start(m.bar)).0);
            if x < left - 60.0 || x > rect.right() {
                continue;
            }
            let flag = Color32::from_rgb(240, 180, 60);
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0, flag.gamma_multiply(0.6)),
            );
            let g =
                painter.layout_no_wrap(m.name.clone(), FontId::proportional(11.0), Color32::BLACK);
            let label = Rect::from_min_size(pos2(x, rect.top()), g.size() + vec2(8.0, 2.0));
            painter.rect_filled(label, 2.0, flag);
            painter.galley(label.min + vec2(4.0, 1.0), g, Color32::BLACK);
        }

        // Playhead.
        let px = x_of(view.playhead);
        if px >= left && px <= rect.right() {
            painter.line_segment(
                [pos2(px, rect.top()), pos2(px, rect.bottom())],
                Stroke::new(1.5, Color32::from_rgb(240, 70, 60)),
            );
        }

        // Right-click menus.
        let end_bar = Self::end_bar(view.map, song);
        resp.context_menu(|ui| match self.menu {
            Some((Hit::Clip { .. }, at)) => {
                if ui.button("Split here").clicked() {
                    out.changed |= self.split_at(song, snap_to(at));
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    out.changed |= self.delete_selected(song);
                    ui.close();
                }
            }
            Some((Hit::Marker(i), _)) if i < song.markers.len() => {
                if ui.button("Loop this section").clicked() {
                    out.loop_bars = Some(section(song.markers, i, end_bar));
                    ui.close();
                }
                if ui.button("Rename…").clicked() {
                    self.rename = Some((i, song.markers[i].name.clone()));
                    self.rename_focused = false;
                    ui.close();
                }
                if ui.button("Delete marker").clicked() {
                    song.markers.remove(i);
                    ui.close();
                }
            }
            Some((_, at)) => {
                let bar = Self::bar_at(view.map, snap(view.map, at, Snap::Bar));
                if ui.button(format!("Add marker at bar {bar}")).clicked() {
                    self.add_marker(song, bar);
                    ui.close();
                }
            }
            None => {
                ui.close();
            }
        });

        // Renaming a marker.
        if let Some((i, mut text)) = self.rename.take() {
            let mut keep = i < song.markers.len();
            let mut done = false;
            egui::Window::new("Rename marker")
                .collapsible(false)
                .resizable(false)
                .default_pos(rect.left_top() + vec2(HEADER + 40.0, RULER + 10.0))
                .show(ui.ctx(), |ui| {
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    let r = ui.text_edit_singleline(&mut text);
                    if !self.rename_focused {
                        r.request_focus();
                        self.rename_focused = true;
                    }
                    ui.horizontal(|ui| {
                        done = ui.button("OK").clicked() || (enter && r.lost_focus());
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
            if keep && done {
                let name = text.trim().to_string();
                if !name.is_empty() && song.markers[i].name != name {
                    song.markers[i].name = name;
                }
            } else if keep {
                self.rename = Some((i, text));
            }
            if self.rename.is_none() {
                self.rename_focused = false;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::time::{Tempo, TimeSignature};

    const RATE: u32 = 48_000;

    fn source(frames: usize) -> Arc<StreamSource> {
        let dir = std::env::temp_dir().join(format!("daw-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // One file each: tests run in parallel.
        let path = dir.join(format!("{}.wav", next_id()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..frames {
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
        StreamSource::open(&path).unwrap()
    }

    fn clip(start: i64, frames: usize) -> Clip {
        let s = source(frames);
        let path = s.path.clone();
        Clip::new(s, path, start, None)
    }

    #[test]
    fn trimming_keeps_the_audio_in_place_and_within_the_file() {
        let mut c = clip(1000, 10_000);
        c.trim_start(3000);
        assert_eq!(
            (c.start, c.offset, c.len, c.end()),
            (3000, 2000, 8000, 11_000)
        );
        // Back past the file's start stops at it.
        c.trim_start(0);
        assert_eq!((c.start, c.offset, c.len), (1000, 0, 10_000));
        c.trim_end(5000);
        assert_eq!(c.end(), 5000);
        // Past the file's end stops at it.
        c.trim_end(50_000);
        assert_eq!(c.end(), 11_000);
        // Never shorter than MIN_LEN.
        c.trim_start(20_000);
        assert_eq!(c.len, MIN_LEN);
        assert_eq!(c.end(), 11_000);
    }

    #[test]
    fn a_take_before_the_start_is_trimmed() {
        let c = clip(-500, 2000);
        assert_eq!((c.start, c.offset, c.len), (0, 500, 1500));
    }

    #[test]
    fn splitting_makes_two_clips_that_play_the_same() {
        let c = clip(1000, 10_000);
        let (a, b) = c.split(4000).unwrap();
        assert_eq!((a.start, a.offset, a.len), (1000, 0, 3000));
        assert_eq!((b.start, b.offset, b.len), (4000, 3000, 7000));
        assert_eq!(a.id, c.id);
        assert_ne!(b.id, c.id);
        assert!(c.split(1010).is_none(), "too near the start");
        assert!(c.split(20_000).is_none(), "outside");
    }

    #[test]
    fn a_new_take_mutes_the_takes_under_it_and_takes_can_be_chosen() {
        let mut clips = vec![clip(0, 48_000)]; // an imported backing track
        add_take(&mut clips, clip(1000, 10_000));
        add_take(&mut clips, clip(5000, 10_000));
        add_take(&mut clips, clip(30_000, 1000)); // elsewhere: nothing under it
        assert_eq!(take_count(&clips), 3);
        let muted: Vec<bool> = clips.iter().map(|c| c.muted).collect();
        assert_eq!(muted, vec![false, true, false, false]);
        assert_eq!(active_take(&clips), Some(2));

        choose_take(&mut clips, Some(0));
        let muted: Vec<bool> = clips.iter().map(|c| c.muted).collect();
        assert_eq!(muted, vec![false, false, true, false]);

        choose_take(&mut clips, None);
        let muted: Vec<bool> = clips.iter().map(|c| c.muted).collect();
        assert_eq!(muted, vec![false, true, true, true], "the file still plays");
    }

    #[test]
    fn snapping_goes_to_the_nearest_beat_or_bar() {
        // 120 BPM in 4/4: a beat is 24,000 frames, a bar 96,000.
        let map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
        assert_eq!(snap(&map, 35_000, Snap::Beat), 24_000);
        assert_eq!(snap(&map, 37_000, Snap::Beat), 48_000);
        assert_eq!(snap(&map, 47_000, Snap::Bar), 0);
        assert_eq!(snap(&map, 49_000, Snap::Bar), 96_000);
        assert_eq!(snap(&map, 12_345, Snap::Off), 12_345);
    }

    #[test]
    fn markers_stay_in_order_and_sections_run_to_the_next() {
        let mut m = Vec::new();
        assert!(add_marker(&mut m, 9));
        assert!(add_marker(&mut m, 1));
        assert!(add_marker(&mut m, 5));
        assert!(!add_marker(&mut m, 5), "one per bar");
        let bars: Vec<i64> = m.iter().map(|m| m.bar).collect();
        assert_eq!(bars, vec![1, 5, 9]);
        assert_eq!(section(&m, 0, 20), (1, 5));
        assert_eq!(section(&m, 2, 20), (9, 20));
        assert_eq!(section(&m, 2, 3), (9, 10), "at least a bar");
    }

    #[test]
    fn splitting_at_the_playhead_takes_the_selection_or_everything() {
        let mut clips = vec![clip(0, 10_000), clip(20_000, 10_000)];
        let mut markers = Vec::new();
        let mut tl = Timeline::default();
        let mut song = Song {
            lanes: vec![Lane {
                name: "Guitar",
                clips: &mut clips,
            }],
            markers: &mut markers,
        };
        assert!(tl.split_at(&mut song, 5000));
        assert_eq!(song.lanes[0].clips.len(), 3);
        let keys: HashSet<&str> = song.lanes[0].clips.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys.len(), 3, "the new half has its own key");
        tl.selected.insert(song.lanes[0].clips[2].id);
        assert!(
            !tl.split_at(&mut song, 2000),
            "only the selected clip, which isn't there"
        );
        assert!(tl.delete_selected(&mut song));
        assert_eq!(song.lanes[0].clips.len(), 2);
        tl.add_marker(&mut song, 3);
        assert_eq!(song.markers.len(), 1);
    }

    #[test]
    fn waveform_ranges_cover_what_is_asked() {
        let base: Vec<(f32, f32)> = (0..64).map(|i| (-(i as f32), i as f32)).collect();
        let w = Waveform::new(base);
        assert_eq!(w.range(0, BASE_HOP), (0.0, 0.0));
        // Coarser levels may reach a little past the range, never short.
        let (lo, hi) = w.range(BASE_HOP * 10, BASE_HOP * 20);
        assert!(
            (-24.0..=-19.0).contains(&lo) && (19.0..=24.0).contains(&hi),
            "{lo} {hi}"
        );
        assert_eq!(w.range(0, BASE_HOP * 64), (-63.0, 63.0));
    }
}
