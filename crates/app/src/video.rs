// SPDX-License-Identifier: GPL-3.0-or-later
//! Video clips (roadmap 3.9): a video file's picture on the timeline,
//! as a filmstrip above the tracks, and the Video pane to watch it in
//! step with the playhead. Its sound comes in as an ordinary audio clip
//! on a track of its own (`import::soundtrack`), so speed, transpose and
//! looping apply, and moves with the picture.
//!
//! Frames are decoded by the system's own media framework (the
//! `daw-video` crate): small ones for the filmstrip on a thread per
//! file, as soon as the file comes in, and kept on disk for next time;
//! and the one at the playhead for the pane on a thread of its own.
//!
//! Scoring to picture (roadmap 3.10) adds, in the pane: the timecode
//! on the picture, stepping a frame at a time, the hit points (the one
//! on screen, the next one coming, and a strip to jump to them), and
//! lining the timecode up with the film; and a copy of the video with
//! the mix as its sound (see `App::export_video`).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use daw_engine::engine::Command;
use daw_engine::time::{Frames, TempoMap};
use daw_engine::timecode::{Clock, Rate};
use daw_project::HitPoint;
use daw_video::player::Player;
use daw_video::thumbs::Thumbs;
use daw_video::{Filmstrip, Frame, Info};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, pos2, vec2,
};

use crate::App;
use crate::commands::Cmd;
use crate::hitpoints;
use crate::timeline::{self, Grid};

/// The filmstrip's height on the timeline.
pub(crate) const BAND: f32 = 58.0;
/// How many filmstrip pictures are kept on the graphics card.
const TEXTURES: usize = 800;
/// How long a hit point's name stays on the picture once the playhead
/// reaches it, in seconds, so it can be seen while playing.
const HIT_SHOWN: f64 = 0.75;
/// The hit point strip's height at the foot of the pane.
const HIT_STRIP: f32 = 22.0;

/// Where the filmstrips are kept between runs.
fn filmstrip_dir() -> PathBuf {
    crate::cache_dir().with_file_name("filmstrips")
}

/// A video clip, as the app has it. Positions are in frames at the
/// app's rate.
pub(crate) struct VideoClip {
    /// Its id in the project document.
    pub(crate) key: String,
    /// The file in the project document: an asset's name once saved,
    /// the file's full path before.
    pub(crate) asset: String,
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) start: i64,
    pub(crate) offset: i64,
    pub(crate) len: i64,
    /// The audio clip holding its sound, by key.
    pub(crate) sound: Option<String>,
    pub(crate) thumbs: Arc<Thumbs>,
}

impl VideoClip {
    pub(crate) fn end(&self) -> i64 {
        self.start + self.len
    }

    pub(crate) fn value(&self) -> daw_project::VideoClip {
        daw_project::VideoClip {
            id: self.key.clone(),
            asset: self.asset.clone(),
            name: self.name.clone(),
            start: self.start,
            offset: self.offset,
            len: self.len,
            sound: self.sound.clone(),
        }
    }

    /// Where the film's first frame falls on the timeline, in frames at
    /// the app's rate.
    pub(crate) fn zero(&self) -> i64 {
        self.start - self.offset
    }

    /// The first sample of the film's frame `n`.
    pub(crate) fn frame_start(&self, info: &Info, n: u64, rate: u32) -> i64 {
        self.zero() + (info.frame_time(n) * rate as f64 - 1e-6).ceil() as i64
    }

    /// The film's frame at `at`, a position on the timeline.
    pub(crate) fn frame_at(&self, info: &Info, at: i64, rate: u32) -> u64 {
        info.frame_at((at - self.zero()) as f64 / rate as f64)
    }

    fn strip(&self, info: Info, rate: f64) -> Filmstrip {
        Filmstrip {
            info,
            start: self.start as f64 / rate,
            offset: self.offset as f64 / rate,
            len: self.len as f64 / rate,
        }
    }
}

/// A video file opening, to become a clip once its length is known.
struct Opening {
    path: PathBuf,
    thumbs: Arc<Thumbs>,
    sound: Option<String>,
    /// Where it starts on the timeline.
    at: i64,
}

/// What the filmstrip asks of the app after it is drawn.
#[derive(Default)]
pub(crate) struct BandOutcome {
    pub(crate) seek: Option<i64>,
    /// A clip was moved or removed: an edit.
    pub(crate) changed: bool,
    /// Move the audio clip with this key by this many frames, as its
    /// picture moved.
    pub(crate) moved: Option<(String, i64)>,
    /// A picture moved: where it was (its film's first frame, its start
    /// and end) and by how much, so its hit points go with it.
    pub(crate) picture: Option<(i64, i64, i64, i64)>,
}

#[derive(Default)]
pub(crate) struct Videos {
    pub(crate) clips: Vec<VideoClip>,
    opening: Vec<Opening>,
    /// Each file's pictures, shared by its clips.
    thumbs: HashMap<PathBuf, Arc<Thumbs>>,
    player: Option<Player>,
    /// Filmstrip pictures uploaded, by file and frame, oldest first.
    textures: HashMap<(PathBuf, u64), TextureHandle>,
    texture_order: VecDeque<(PathBuf, u64)>,
    /// The pane's picture, and the player's count it was made at.
    shown: Option<(u64, TextureHandle)>,
    /// The clip being dragged: its key, the frame the press was at and
    /// where it started.
    drag: Option<(String, i64, i64)>,
    /// The clip right-clicked.
    menu: Option<String>,
    pub(crate) selected: Option<String>,
    /// The pane leaves the timecode off the picture.
    pub(crate) hide_timecode: bool,
}

/// What the pane asks of the app.
#[derive(Default)]
pub(crate) struct PaneOutcome {
    pub(crate) seek: Option<i64>,
    pub(crate) run: Option<Cmd>,
}

/// The number of a decoded frame.
fn number(info: &Info, f: &Frame) -> u64 {
    info.frame_at(f.time + 0.25 / info.fps().max(1e-6))
}

impl Videos {
    fn thumbs_for(&mut self, path: &Path) -> Arc<Thumbs> {
        self.thumbs
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                // Tests leave the user's cache alone.
                Arc::new(if cfg!(test) {
                    Thumbs::start(path)
                } else {
                    Thumbs::start_cached(path, &filmstrip_dir())
                })
            })
            .clone()
    }

    /// The clip showing at `at`: the last added, where two overlap.
    pub(crate) fn clip_at(&self, at: i64) -> Option<&VideoClip> {
        self.clips
            .iter()
            .rev()
            .find(|c| at >= c.start && at < c.end())
    }

    /// The clip a command about "the video" means: the one selected,
    /// or else the one at `at`, or else the first.
    pub(crate) fn current(&self, at: i64) -> Option<&VideoClip> {
        (self.selected.as_ref())
            .and_then(|k| self.clips.iter().find(|c| &c.key == k))
            .or_else(|| self.clip_at(at))
            .or_else(|| self.clips.first())
    }

    /// Where `by` frames of the film on from the one at `at` starts, if
    /// a film is showing at `at` and its length is known.
    pub(crate) fn step(&self, at: i64, by: i64, rate: u32) -> Option<i64> {
        let c = self.clip_at(at)?;
        let info = c.thumbs.info()?;
        let n = c.frame_at(&info, at, rate) as i64;
        let first = c.frame_at(&info, c.start, rate) as i64;
        let last = c.frame_at(&info, c.end() - 1, rate) as i64;
        let to = (n + by).clamp(first, last.max(first));
        Some(c.frame_start(&info, to as u64, rate).max(c.start))
    }

    /// Starts bringing in `path`'s picture, to start at frame `at`.
    pub(crate) fn add(&mut self, path: &Path, at: i64) {
        let thumbs = self.thumbs_for(path);
        self.opening.push(Opening {
            path: path.to_path_buf(),
            thumbs,
            sound: None,
            at: at.max(0),
        });
    }

    /// The sound of `path` came in as clip `key`: it moves with the
    /// picture.
    pub(crate) fn sound_added(&mut self, path: &Path, key: &str) {
        if let Some(o) = self
            .opening
            .iter_mut()
            .find(|o| o.path == path && o.sound.is_none())
        {
            o.sound = Some(key.to_string());
        } else if let Some(c) =
            (self.clips.iter_mut()).find(|c| c.path == path && c.sound.is_none())
        {
            c.sound = Some(key.to_string());
        }
    }

    /// Whether `path` is a video coming in or here.
    pub(crate) fn has(&self, path: &Path) -> bool {
        self.opening.iter().any(|o| o.path == path) || self.clips.iter().any(|c| c.path == path)
    }

    /// Takes in files that have opened. Returns whether a clip was
    /// added (an edit), and a message for any that couldn't be.
    pub(crate) fn poll(&mut self, rate: u32) -> (bool, Option<String>) {
        let (mut added, mut message) = (false, None);
        let mut i = 0;
        while i < self.opening.len() {
            let o = &self.opening[i];
            if let Some(e) = o.thumbs.error() {
                let name = file_name(&o.path);
                message = Some(format!("{name}: {e}"));
                self.thumbs.remove(&o.path);
                self.opening.remove(i);
                continue;
            }
            let Some(info) = o.thumbs.info() else {
                i += 1;
                continue;
            };
            let o = self.opening.remove(i);
            self.clips.push(VideoClip {
                key: daw_project::new_id(),
                asset: o.path.to_string_lossy().into_owned(),
                name: file_name(&o.path),
                start: o.at,
                offset: 0,
                len: (info.seconds * rate as f64).round().max(1.0) as i64,
                sound: o.sound,
                path: o.path,
                thumbs: o.thumbs,
            });
            added = true;
        }
        (added, message)
    }

    /// Whether files are still opening.
    pub(crate) fn busy(&self) -> bool {
        !self.opening.is_empty()
    }

    /// Takes the project's clips, keeping what is known of those it has.
    /// `path_of` says where an asset's file is; `scale` turns the
    /// project's frames into the app's.
    pub(crate) fn apply(
        &mut self,
        clips: &[daw_project::VideoClip],
        path_of: &dyn Fn(&str) -> PathBuf,
        scale: &dyn Fn(i64) -> i64,
    ) {
        let mut had: HashMap<String, VideoClip> =
            self.clips.drain(..).map(|c| (c.key.clone(), c)).collect();
        for v in clips {
            let path = path_of(&v.asset);
            let thumbs = match had.remove(&v.id) {
                Some(c) if c.path == path => c.thumbs,
                _ => self.thumbs_for(&path),
            };
            self.clips.push(VideoClip {
                key: v.id.clone(),
                asset: v.asset.clone(),
                path,
                name: v.name.clone(),
                start: scale(v.start).max(0),
                offset: scale(v.offset).max(0),
                len: scale(v.len).max(1),
                sound: v.sound.clone(),
                thumbs,
            });
        }
        // Files no clip uses any more stop decoding.
        let used: Vec<PathBuf> = (self.clips.iter().map(|c| c.path.clone()))
            .chain(self.opening.iter().map(|o| o.path.clone()))
            .collect();
        self.thumbs.retain(|p, _| used.contains(p));
        self.textures.retain(|(p, _), _| used.contains(p));
        self.texture_order.retain(|(p, _)| used.contains(p));
        if self
            .selected
            .as_ref()
            .is_some_and(|k| !self.clips.iter().any(|c| &c.key == k))
        {
            self.selected = None;
        }
    }

    /// The asset moved into the project's folder as `name`, at `path`.
    pub(crate) fn saved_as(&mut self, old: &str, name: &str, path: &Path) {
        for c in self.clips.iter_mut().filter(|c| c.asset == old) {
            c.asset = name.to_string();
            c.path = path.to_path_buf();
        }
    }

    /// A filmstrip picture of `n`, or the nearest made so far.
    fn texture(
        &mut self,
        ctx: &egui::Context,
        c: usize,
        info: &Info,
        n: u64,
    ) -> Option<TextureHandle> {
        let path = self.clips[c].path.clone();
        if let Some(t) = self.textures.get(&(path.clone(), n)) {
            return Some(t.clone());
        }
        let (f, _) = self.clips[c].thumbs.get(n)?;
        let got = number(info, &f);
        let key = (path, got);
        if let Some(t) = self.textures.get(&key) {
            return Some(t.clone());
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [f.width as usize, f.height as usize],
            &f.rgba,
        );
        let t = ctx.load_texture(
            format!("filmstrip {} {got}", key.0.display()),
            image,
            TextureOptions::LINEAR,
        );
        self.textures.insert(key.clone(), t.clone());
        self.texture_order.push_back(key);
        while self.texture_order.len() > TEXTURES {
            if let Some(old) = self.texture_order.pop_front() {
                self.textures.remove(&old);
            }
        }
        Some(t)
    }

    /// Draws the filmstrip in `rect` (the band under the ruler, with
    /// the track names' column at its left) and handles the pointer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn band(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        header: f32,
        (scroll, pps): (f64, f32),
        rate: u32,
        playhead: i64,
        map: &TempoMap,
        snap: Option<Grid>,
    ) -> BandOutcome {
        let mut out = BandOutcome::default();
        let rate_f = rate as f64;
        let pps_f = pps as f64;
        let left = rect.left() + header;
        let lanes = Rect::from_min_max(pos2(left, rect.top()), rect.max);
        let x_of = |f: i64| left + ((f as f64 / rate_f - scroll) * pps_f) as f32;
        let frame_of = |x: f32| ((scroll + (x - left) as f64 / pps_f) * rate_f).round() as i64;
        let resp = ui.interact(
            lanes,
            ui.scope_id().with("video band"),
            Sense::click_and_drag(),
        );
        let clip_at = |videos: &Videos, x: f32| {
            let f = frame_of(x);
            videos
                .clips
                .iter()
                .rev()
                .find(|c| f >= c.start && f < c.end())
                .map(|c| c.key.clone())
        };

        // Pointer: pick, drag along, or click empty film to go there.
        if self.drag.is_none()
            && let Some(p) = resp.hover_pos()
            && clip_at(self, p.x).is_some()
        {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
            && let Some(key) = clip_at(self, p.x)
            && let Some(c) = self.clips.iter().find(|c| c.key == key)
        {
            self.drag = Some((key.clone(), frame_of(p.x), c.start));
            self.selected = Some(key);
        }
        if let Some((key, from, was)) = self.drag.clone()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(c) = self.clips.iter_mut().find(|c| c.key == key)
        {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            let mut to = (was + frame_of(p.x) - from).max(0);
            if let Some(grid) = snap
                && !ui.input(|i| i.modifiers.alt)
            {
                to = timeline::snap(map, to, grid.at_zoom(map, to, pps_f)).max(0);
            }
            c.start = to;
        }
        if resp.drag_stopped()
            && let Some((key, _, was)) = self.drag.take()
            && let Some(c) = self.clips.iter().find(|c| c.key == key)
            && c.start != was
        {
            out.changed = true;
            let by = c.start - was;
            out.picture = Some((c.zero() - by, was, c.end() - by, by));
            if let Some(s) = &c.sound {
                out.moved = Some((s.clone(), by));
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.selected = clip_at(self, p.x);
            out.seek = Some(frame_of(p.x).max(0));
        }
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.menu = clip_at(self, p.x);
            if self.menu.is_some() {
                self.selected = self.menu.clone();
            }
        }
        if self.menu.is_some() {
            resp.context_menu(|ui| {
                let Some(key) = self.menu.clone() else { return };
                let Some(i) = self.clips.iter().position(|c| c.key == key) else {
                    return;
                };
                if ui
                    .button("Move to the start of the song")
                    .on_hover_text("Its sound moves with it")
                    .clicked()
                {
                    let c = &mut self.clips[i];
                    if c.start != 0 {
                        if let Some(s) = &c.sound {
                            out.moved = Some((s.clone(), -c.start));
                        }
                        out.picture = Some((c.zero(), c.start, c.end(), -c.start));
                        c.start = 0;
                        out.changed = true;
                    }
                    ui.close();
                }
                if ui
                    .button("Remove the picture")
                    .on_hover_text("Its sound stays on its own track")
                    .clicked()
                {
                    self.clips.remove(i);
                    self.selected = None;
                    out.changed = true;
                    ui.close();
                }
            });
        }

        // Drawing.
        let look = crate::theme::look(ui.ctx());
        let tc = look.c().clone();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, tc.lane);
        let name_rect = Rect::from_min_max(rect.min, pos2(left, rect.bottom()));
        painter.rect_filled(name_rect, 0.0, tc.panel);
        painter.text(
            name_rect.left_top() + vec2(8.0, 6.0),
            Align2::LEFT_TOP,
            "Video",
            look.font(look.body),
            tc.text,
        );
        let opening = self.opening.len();
        let making = (self.clips.iter())
            .map(|c| c.thumbs.progress())
            .fold(1.0f32, f32::min);
        let status = if opening > 0 {
            "Opening…".to_string()
        } else if making < 1.0 {
            format!("Filmstrip {:.0}%", making * 100.0)
        } else {
            String::new()
        };
        if !status.is_empty() {
            painter.text(
                name_rect.left_bottom() + vec2(8.0, -6.0),
                Align2::LEFT_BOTTOM,
                status,
                FontId::proportional(look.small),
                tc.text_dim,
            );
        }
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(look.line, tc.line),
        );

        let strip = painter.with_clip_rect(lanes);
        let (from_s, to_s) = (scroll, scroll + lanes.width() as f64 / pps_f);
        let film_h = rect.height() - 6.0;
        let mut wanted: HashMap<PathBuf, Vec<u64>> = HashMap::new();
        let ctx = ui.ctx().clone();
        for ci in 0..self.clips.len() {
            let c = &self.clips[ci];
            let (x0, x1) = (x_of(c.start), x_of(c.end()));
            let body = Rect::from_x_y_ranges(x0..=x1, rect.top() + 3.0..=rect.bottom() - 3.0);
            strip.rect_filled(body, 3.0, Color32::from_gray(12));
            let Some(info) = c.thumbs.info() else {
                continue;
            };
            let tile = (film_h * info.width as f32 / info.height.max(1) as f32).max(8.0) as f64;
            let fs = c.strip(info, rate_f);
            let tiles = fs.tiles(from_s, to_s, pps_f, tile);
            let path = c.path.clone();
            wanted
                .entry(path)
                .or_default()
                .extend(tiles.iter().map(|t| t.frame));
            for t in &tiles {
                let (a, b) = (
                    left + ((t.from - scroll) * pps_f) as f32,
                    left + ((t.to - scroll) * pps_f) as f32,
                );
                let w = (b - a).min(tile as f32);
                let pic = Rect::from_min_size(pos2(a, body.top()), vec2(w, film_h));
                if let Some(tex) = self.texture(&ctx, ci, &info, t.frame) {
                    let u = (w as f64 / tile).min(1.0) as f32;
                    strip.image(
                        tex.id(),
                        pic,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(u, 1.0)),
                        Color32::WHITE,
                    );
                }
                // Each frame's start, once frames are wider than their
                // pictures.
                if b - a > tile as f32 + 1.0 {
                    strip.line_segment(
                        [pos2(a, body.top()), pos2(a, body.bottom())],
                        Stroke::new(1.0, Color32::from_gray(90)),
                    );
                }
            }
            let c = &self.clips[ci];
            let selected = self.selected.as_ref() == Some(&c.key);
            let (w, colour) = if selected {
                (2.0, tc.accent)
            } else {
                (look.line, tc.line)
            };
            strip.rect_stroke(body, 3.0, Stroke::new(w, colour), StrokeKind::Inside);
            strip.text(
                pos2(x0.max(left) + 5.0, body.top() + 3.0),
                Align2::LEFT_TOP,
                &c.name,
                FontId::proportional(look.small),
                Color32::WHITE,
            );
        }
        for (path, frames) in wanted {
            if let Some(t) = self.thumbs.get(&path) {
                t.only_want(&frames);
            }
        }
        let px = x_of(playhead);
        if px >= left && px <= rect.right() {
            strip.line_segment(
                [pos2(px, rect.top()), pos2(px, rect.bottom())],
                Stroke::new(1.5 * look.line, tc.playhead),
            );
        }
        if self.clips.iter().any(|c| c.thumbs.changed()) {
            ui.ctx().request_repaint();
        }
        if opening > 0 || making < 1.0 {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        out
    }

    /// The Video pane's picture: the frame at the playhead, with the
    /// timecode on it (by `clock`), the hit point it shows and the next
    /// one, and a strip of the hit points along its foot.
    pub(crate) fn pane(
        &mut self,
        ui: &mut egui::Ui,
        playhead: i64,
        rate: u32,
        clock: &Clock,
        hits: &[HitPoint],
    ) -> PaneOutcome {
        let mut out = PaneOutcome::default();
        let player = self.player.get_or_insert_with(Player::new);
        let clip = self
            .clips
            .iter()
            .rev()
            .find(|c| playhead >= c.start && playhead < c.end());
        let at = clip.map(|c| (c.offset + playhead - c.start) as f64 / rate as f64);
        player.show(clip.map(|c| c.path.as_path()), at.unwrap_or(0.0));
        let (frame, count) = player.frame();
        let look = crate::theme::look(ui.ctx());
        let tc = look.c().clone();
        let (all, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        // The hit point strip along the foot, under a film.
        let (rect, strip) = match clip {
            Some(_) if all.height() > 3.0 * HIT_STRIP => {
                let (top, foot) = all.split_top_bottom_at_y(all.bottom() - HIT_STRIP);
                (top, Some(foot))
            }
            _ => (all, None),
        };
        let painter = ui.painter_at(all);
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        let say = |text: &str| {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                look.font(look.body),
                tc.text_dim,
            );
        };
        let Some(c) = clip else {
            self.shown = None;
            if self.clips.is_empty() {
                say(if self.opening.is_empty() {
                    "No video yet. Drop a video file on the window to score or play along to it."
                } else {
                    "Opening the video…"
                });
            } else {
                say("No video at the playhead");
            }
            return out;
        };
        if let Some(r) = strip {
            out.seek = hit_strip(ui, &painter, r, c, playhead, hits, &look);
        }
        if let Some(e) = player.error(&c.path) {
            say(&e.to_string());
            return out;
        }
        let info = player.info();
        // A picture from another file is let go of.
        let Some(f) = frame.filter(|_| info.is_some()) else {
            say("Opening the video…");
            ui.ctx().request_repaint_after(Duration::from_millis(30));
            return out;
        };
        if self.shown.as_ref().is_none_or(|(n, _)| *n != count) {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [f.width as usize, f.height as usize],
                &f.rgba,
            );
            match &mut self.shown {
                Some((n, t)) => {
                    t.set(image, TextureOptions::LINEAR);
                    *n = count;
                }
                None => {
                    let t = ui
                        .ctx()
                        .load_texture("video frame", image, TextureOptions::LINEAR);
                    self.shown = Some((count, t));
                }
            }
        }
        let k = (rect.width() / f.width as f32).min(rect.height() / f.height as f32);
        let pic =
            Rect::from_center_size(rect.center(), vec2(f.width as f32 * k, f.height as f32 * k));
        if let Some((_, tex)) = &self.shown {
            painter.image(
                tex.id(),
                pic,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let (Some(info), Some(at)) = (info, at) else {
            return out;
        };
        let n = info.frame_at(at);
        // Not there yet: look again shortly.
        if number(&info, &f) != n {
            ui.ctx().request_repaint_after(Duration::from_millis(15));
        }
        // Labels sit on the picture, or on the pane where the picture
        // is too small for them.
        let on = pic.intersect(rect);
        let boxed = |text: String, font: FontId, pos: Pos2, anchor: Align2, fill: Color32| {
            let g = painter.layout_no_wrap(text, font, Color32::WHITE);
            let r = anchor.anchor_size(pos, g.size() + vec2(10.0, 4.0));
            painter.rect_filled(r, 3.0, fill);
            painter.galley(r.min + vec2(5.0, 2.0), g, Color32::WHITE);
        };
        let shade = Color32::from_black_alpha(170);
        let small = FontId::monospace(look.small);
        boxed(
            format!(
                "{}   frame {} of {}   {}",
                c.name,
                n + 1,
                info.frames(),
                clock_text(at)
            ),
            small.clone(),
            on.left_top() + vec2(6.0, 6.0),
            Align2::LEFT_TOP,
            shade,
        );
        if !self.hide_timecode {
            // Burnt-in style: large, at the foot of the picture.
            let size = (on.height() * 0.08).clamp(14.0, 40.0);
            boxed(
                clock.timecode(playhead, rate).to_string(),
                FontId::monospace(size),
                on.center_bottom() - vec2(0.0, 8.0),
                Align2::CENTER_BOTTOM,
                shade,
            );
        }
        // The hit point just reached, and the next one coming.
        let held = (HIT_SHOWN * rate as f64) as i64;
        let frame_len = clock.frame_start(1, rate) - clock.frame_start(0, rate);
        if let Some(h) =
            (hits.iter().rev()).find(|h| h.at <= playhead && playhead < h.at + held.max(frame_len))
        {
            boxed(
                format!("Hit point: {}", h.name),
                look.font(look.body),
                on.left_top() + vec2(6.0, 30.0),
                Align2::LEFT_TOP,
                tc.accent.gamma_multiply(0.85),
            );
        }
        if let Some(h) = hits.iter().find(|h| h.at > playhead) {
            let secs = (h.at - playhead) as f64 / rate as f64;
            boxed(
                format!("Next: {} in {secs:.2} s", h.name),
                small,
                on.right_top() + vec2(-6.0, 6.0),
                Align2::RIGHT_TOP,
                shade,
            );
        }
        out
    }
}

/// The strip under the picture: the film's length, its hit points as
/// diamonds and the playhead. A click goes there, or to the hit point
/// clicked.
#[allow(clippy::too_many_arguments)]
fn hit_strip(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    r: Rect,
    c: &VideoClip,
    playhead: i64,
    hits: &[HitPoint],
    look: &crate::theme::Look,
) -> Option<i64> {
    let tc = look.c();
    painter.rect_filled(r, 0.0, tc.panel);
    let inner = r.shrink2(vec2(8.0, 0.0));
    let len = c.len.max(1) as f32;
    let x_of = |at: i64| inner.left() + (at - c.start) as f32 / len * inner.width();
    let at_of = |x: f32| c.start + (((x - inner.left()) / inner.width()) * len) as i64;
    let mid = r.center().y;
    painter.line_segment(
        [pos2(inner.left(), mid), pos2(inner.right(), mid)],
        Stroke::new(2.0 * look.line, tc.line),
    );
    let inside: Vec<&HitPoint> = (hits.iter())
        .filter(|h| h.at >= c.start && h.at < c.end())
        .collect();
    for h in &inside {
        painter.add(hitpoints::diamond(pos2(x_of(h.at), mid), 5.0, tc.accent));
    }
    let px = x_of(playhead);
    painter.line_segment(
        [pos2(px, r.top() + 3.0), pos2(px, r.bottom() - 3.0)],
        Stroke::new(1.5 * look.line, tc.playhead),
    );
    let resp = ui
        .interact(r, ui.scope_id().with("video hit strip"), Sense::click())
        .on_hover_text("The film's hit points: click one, or anywhere, to go there");
    let p = resp.interact_pointer_pos().filter(|_| resp.clicked())?;
    let near = (inside.iter())
        .map(|h| (h.at, (x_of(h.at) - p.x).abs()))
        .filter(|(_, d)| *d <= 7.0)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    Some(near.map_or_else(|| at_of(p.x).clamp(c.start, c.end() - 1), |(at, _)| at))
}

/// The frame rate in the timecode list matching a film's, keeping the
/// way of counting chosen where the film's rate allows it (29.97 with or
/// without drop-frame).
pub(crate) fn rate_for(info: &Info, now: Rate) -> Option<Rate> {
    let same = |r: Rate| {
        let (n, d) = r.fps();
        n * info.rate.1 as i64 == d * info.rate.0 as i64
    };
    if same(now) {
        return Some(now);
    }
    Rate::ALL.into_iter().find(|&r| same(r))
}

// ---- In the app ----

impl App {
    /// The Video pane: a row of buttons, then the picture.
    pub(crate) fn video_pane(&mut self, ui: &mut egui::Ui) {
        let playhead = self.ctl.status().position().0;
        let mut run = None;
        ui.horizontal(|ui| {
            for (cmd, label, name) in [
                (Cmd::PrevHitPoint, "⏮◆", "Previous hit point"),
                (Cmd::FrameBack, "◀", "Back a frame"),
                (Cmd::FrameForward, "▶", "On a frame"),
                (Cmd::NextHitPoint, "◆⏭", "Next hit point"),
            ] {
                if ui
                    .button(label)
                    .accessible_name(name)
                    .on_hover_text(self.commands.hover(cmd))
                    .clicked()
                {
                    run = Some(cmd);
                }
            }
            ui.separator();
            let mut on = !self.video.hide_timecode;
            if ui
                .checkbox(&mut on, "Timecode")
                .on_hover_text(self.commands.hover(Cmd::VideoTimecode))
                .changed()
            {
                run = Some(Cmd::VideoTimecode);
            }
            // Offered when the timecode doesn't yet start with the film.
            let film = self.video.current(playhead);
            let lined_up =
                film.and_then(|c| Some((c, c.thumbs.info()?)))
                    .is_none_or(|(c, info)| {
                        let clock = hitpoints::clock(&self.timecode);
                        // A film at a rate timecode doesn't count in is
                        // lined up once its start is.
                        let rate = rate_for(&info, clock.rate);
                        rate.is_none_or(|r| r == clock.rate) && clock.offset == c.zero()
                    });
            if !lined_up
                && ui
                    .button(self.commands.name(Cmd::TimecodeFromFilm).to_string())
                    .on_hover_text(self.commands.hover(Cmd::TimecodeFromFilm))
                    .clicked()
            {
                run = Some(Cmd::TimecodeFromFilm);
            }
        });
        let clock = hitpoints::clock(&self.timecode);
        let out = self
            .video
            .pane(ui, playhead, self.rate, &clock, &self.hit_points);
        if let Some(at) = out.seek {
            self.send(Command::Seek(Frames(at)));
        }
        if let Some(cmd) = run.or(out.run) {
            let ctx = ui.ctx().clone();
            self.run(&ctx, cmd);
        }
    }

    /// Moves the playhead `by` frames: the film's frames where one is
    /// showing, or else the timecode's. Playback stops, as stepping is
    /// for looking closely.
    pub(crate) fn step_frame(&mut self, by: i64) {
        if self.ctl.status().playing() {
            self.send(Command::Stop);
        }
        let at = self.ctl.status().position().0;
        let to = self.video.step(at, by, self.rate).unwrap_or_else(|| {
            let c = hitpoints::clock(&self.timecode);
            c.frame_start(c.frame_at(at, self.rate) + by, self.rate)
        });
        self.send(Command::Seek(Frames(to.max(0))));
    }

    /// Goes to the next hit point after the playhead, or the one before
    /// it (`forward` false).
    pub(crate) fn jump_hit_point(&mut self, forward: bool) {
        let at = self.ctl.status().position().0;
        let to = if forward {
            self.hit_points.iter().map(|h| h.at).find(|&h| h > at)
        } else {
            self.hit_points.iter().rev().map(|h| h.at).find(|&h| h < at)
        };
        if let Some(to) = to {
            self.send(Command::Seek(Frames(to)));
        }
    }

    /// A picture moved by `by` from where its film's first frame was
    /// `zero` and it ran from `start` to `end`: the hit points on it go
    /// with it, and so does the timecode if it was lined up with it.
    pub(crate) fn picture_moved(&mut self, (zero, start, end, by): (i64, i64, i64, i64)) {
        if self.timecode.offset == zero {
            self.timecode.offset += by;
        }
        for h in self
            .hit_points
            .iter_mut()
            .filter(|h| h.at >= start && h.at < end)
        {
            h.at = (h.at + by).max(0);
        }
        self.hit_points.sort_by_key(|h| h.at);
    }

    /// Sets the timecode to the film's frame rate, with its start
    /// timecode on the film's first frame. Hit points stay with the
    /// picture.
    pub(crate) fn timecode_from_film(&mut self) {
        if !self.may_edit_song() {
            return;
        }
        let at = self.ctl.status().position().0;
        let Some((zero, info)) =
            (self.video.current(at)).and_then(|c| Some((c.zero(), c.thumbs.info()?)))
        else {
            self.message = Some("There's no video to take the timecode from.".into());
            return;
        };
        let now_rate = hitpoints::clock(&self.timecode).rate;
        match rate_for(&info, now_rate) {
            Some(r) => self.timecode.rate = r.id().to_string(),
            None => {
                self.message = Some(format!(
                    "The film runs at {:.3} frames a second, which timecode doesn't count in; \
                     its start is lined up, at the rate already chosen.",
                    info.fps()
                ));
            }
        }
        self.timecode.offset = zero;
        self.timecode.shown = true;
        // The film hasn't moved, so neither do its hit points: they only
        // go on the start of their frame, which is now the film's.
        let c = hitpoints::clock(&self.timecode);
        for h in &mut self.hit_points {
            h.at = c.snap(h.at, self.rate).max(0);
        }
    }
}

/// Seconds as minutes, seconds and thousandths: 1:02.500.
fn clock_text(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round().max(0.0) as u64;
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

fn file_name(path: &Path) -> String {
    path.file_stem()
        .map_or("Video".into(), |s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_reads_minutes_seconds_and_thousandths() {
        assert_eq!(clock_text(62.5), "1:02.500");
        assert_eq!(clock_text(0.0), "0:00.000");
    }
}

#[cfg(all(test, any(target_os = "linux", windows)))]
mod in_the_app {
    use std::path::PathBuf;
    use std::time::Duration;

    use daw_project::ProjectFolder;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use crate::App;

    /// Two seconds at 12 frames a second, with a tone (see the video
    /// crate's tests). The system here decodes the Ogg one; Windows the
    /// MP4.
    fn film() -> PathBuf {
        let name = if cfg!(windows) {
            "counting.mp4"
        } else {
            "counting.ogv"
        };
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../video/tests/fixtures")
            .join(name)
    }

    fn until(h: &mut Harness<'_, App>, what: impl Fn(&App) -> bool) {
        for _ in 0..2000 {
            h.step();
            if what(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    #[test]
    fn a_video_file_brings_its_picture_and_its_sound_which_move_together() {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        let rate = h.state().rate as i64;
        h.state_mut().add_file(&film());
        until(&mut h, |a| {
            !a.video.clips.is_empty() && !a.tracks.is_empty()
        });
        h.run();
        let a = h.state();
        let v = &a.video.clips[0];
        assert_eq!(v.name, "counting");
        assert!((v.len - 2 * rate).abs() < rate / 10, "{}", v.len);
        let sound = &a.tracks[0].clips[0];
        assert_eq!(v.sound.as_deref(), Some(sound.key.as_str()));
        assert!(h.query_by_label("Video tab").is_some(), "the pane opened");

        // In the project, and read back the same.
        let ctx = h.ctx.clone();
        h.state_mut().sync_doc(&ctx, true);
        let p = h.state().project();
        assert_eq!(p.videos.len(), 1);
        h.state_mut().apply(p.clone());
        h.run();
        assert_eq!(h.state().project(), p);

        // Saving brings the file into the project's folder.
        let dir = std::env::temp_dir().join(format!("daw-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let song = dir.join("Film");
        h.state_mut().save_in(&ctx, &song).unwrap();
        let folder = ProjectFolder { dir: song.clone() };
        let saved = &h.state().video.clips[0];
        assert!(folder.has_asset(&saved.asset), "{}", saved.asset);
        assert_eq!(saved.path, folder.asset(&saved.asset));
        assert!(h.state().proj.doc.assets_in_use().contains(&saved.asset));

        // The pane shows the frame at the playhead.
        h.state_mut()
            .send(daw_engine::engine::Command::Seek(daw_engine::time::Frames(
                rate,
            )));
        until(&mut h, |a| {
            a.video
                .player
                .as_ref()
                .and_then(|p| p.frame().0)
                .is_some_and(|f| (f.time - 1.0).abs() < 0.01)
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scoring_to_picture_steps_frames_jumps_to_hit_points_and_exports_the_film() {
        use crate::commands::Cmd;

        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        let ctx = h.ctx.clone();
        let rate = h.state().rate as i64;
        // A film at bar 2 (two seconds in, at 120 a minute).
        h.state_mut().video.add(&film(), 2 * rate);
        until(&mut h, |a| {
            a.video
                .clips
                .first()
                .is_some_and(|c| c.thumbs.info().is_some())
        });
        let start = h.state().video.clips[0].start;
        assert_eq!(start, 2 * rate);
        let pos = |h: &Harness<'_, App>| h.state().ctl.status().position().0;
        let go = |h: &mut Harness<'_, App>, to: i64| {
            h.state_mut()
                .send(daw_engine::engine::Command::Seek(daw_engine::time::Frames(
                    to,
                )));
            until(h, |a| a.ctl.status().position().0 == to);
        };
        // 12 frames a second: a frame is a twelfth of a second.
        let frame = |n: i64| start + (n * rate).div_euclid(12) + i64::from((n * rate) % 12 != 0);
        go(&mut h, start + rate / 24);
        h.state_mut().run(&ctx, Cmd::FrameForward);
        until(&mut h, |a| a.ctl.status().position().0 == frame(1));
        h.state_mut().run(&ctx, Cmd::FrameForward);
        until(&mut h, |a| a.ctl.status().position().0 == frame(2));
        h.state_mut().run(&ctx, Cmd::FrameBack);
        until(&mut h, |a| a.ctl.status().position().0 == frame(1));
        // Held at the film's first frame.
        h.state_mut().run(&ctx, Cmd::FrameBack);
        h.state_mut().run(&ctx, Cmd::FrameBack);
        until(&mut h, |a| a.ctl.status().position().0 == start);

        // Timecode lined up with the film: its rate, and its start on
        // the film's first frame.
        h.state_mut().run(&ctx, Cmd::TimecodeFromFilm);
        let tc = h.state().timecode.clone();
        assert_eq!(tc.offset, start);
        assert!(tc.shown);
        // 12 fps isn't a timecode rate: the one chosen stays.
        assert_eq!(tc.rate, "25");

        // Hit points: jump between them.
        h.state_mut().add_hit_point(frame(6));
        h.state_mut().add_hit_point(frame(18));
        assert_eq!(h.state().hit_points.len(), 2);
        let hits: Vec<i64> = h.state().hit_points.iter().map(|x| x.at).collect();
        go(&mut h, start);
        h.state_mut().run(&ctx, Cmd::NextHitPoint);
        until(&mut h, |a| a.ctl.status().position().0 == hits[0]);
        h.state_mut().run(&ctx, Cmd::NextHitPoint);
        until(&mut h, |a| a.ctl.status().position().0 == hits[1]);
        h.state_mut().run(&ctx, Cmd::PrevHitPoint);
        until(&mut h, |a| a.ctl.status().position().0 == hits[0]);
        assert_eq!(pos(&h), hits[0]);

        // Moving the picture takes its hit points and timecode with it.
        h.state_mut()
            .picture_moved((start, start, start + 2 * rate, rate));
        assert_eq!(h.state().timecode.offset, start + rate);
        assert_eq!(h.state().hit_points[0].at, hits[0] + rate);
        h.state_mut()
            .picture_moved((start + rate, start + rate, start + 3 * rate, -rate));

        // The pane draws, with the timecode on the picture, which a
        // command hides.
        h.state_mut().run(&ctx, Cmd::Video);
        h.run();
        assert!(!h.state().video.hide_timecode);
        h.state_mut().run(&ctx, Cmd::VideoTimecode);
        assert!(h.state().video.hide_timecode);
        h.state_mut().run(&ctx, Cmd::VideoTimecode);

        // The film with the mix, into the project's Exports folder.
        let dir = std::env::temp_dir().join(format!("daw-scoring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        h.state_mut().save_in(&ctx, &dir.join("Score")).unwrap();
        h.run();
        h.state_mut().run(&ctx, Cmd::ExportVideo);
        assert!(h.state().export.busy(), "{:?}", h.state().message);
        until(&mut h, |a| a.export.finished().is_some());
        let (folder, lines) = h.state().export.finished().unwrap();
        assert_eq!(lines.len(), 1, "only the film: {lines:?}");
        let (name, error) = lines[0];
        assert_eq!(error, None, "{name}");
        let out = folder.join(name);
        assert!(name.starts_with("counting with the mix."), "{name}");
        let mut r = daw_video::Reader::open(&out, (64, 48)).unwrap();
        assert_eq!(r.info().frames(), 24);
        assert!(r.next_frame().unwrap().is_some());
        // Nothing left behind but the film.
        let files: Vec<_> = std::fs::read_dir(folder).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
