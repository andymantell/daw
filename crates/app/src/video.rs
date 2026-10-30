// SPDX-License-Identifier: GPL-3.0-or-later
//! Video clips (roadmap 3.9): a video file's picture on the timeline,
//! as a filmstrip above the tracks, and the Video pane to watch it in
//! step with the playhead. Its sound comes in as an ordinary audio clip
//! on a track of its own (`import::soundtrack`), so speed, transpose and
//! looping apply, and moves with the picture.
//!
//! Frames are decoded by the system's own media framework (the
//! `daw-video` crate): small ones for the filmstrip on a thread per
//! file, as soon as the file comes in, and the one at the playhead for
//! the pane on a thread of its own.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use daw_engine::time::TempoMap;
use daw_video::player::Player;
use daw_video::thumbs::Thumbs;
use daw_video::{Filmstrip, Frame, Info};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, pos2, vec2,
};

use crate::timeline::{self, Grid};

/// The filmstrip's height on the timeline.
pub(crate) const BAND: f32 = 58.0;
/// How many filmstrip pictures are kept on the graphics card.
const TEXTURES: usize = 800;

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
}

/// The number of a decoded frame.
fn number(info: &Info, f: &Frame) -> u64 {
    info.frame_at(f.time + 0.25 / info.fps().max(1e-6))
}

impl Videos {
    fn thumbs_for(&mut self, path: &Path) -> Arc<Thumbs> {
        self.thumbs
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Thumbs::start(path)))
            .clone()
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
            if let Some(s) = &c.sound {
                out.moved = Some((s.clone(), c.start - was));
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

    /// The Video pane: the frame at the playhead.
    pub(crate) fn pane(&mut self, ui: &mut egui::Ui, playhead: i64, rate: u32) {
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
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let painter = ui.painter_at(rect);
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
            return;
        };
        if let Some(e) = player.error(&c.path) {
            say(&e.to_string());
            return;
        }
        let info = player.info();
        // A picture from another file is let go of.
        let Some(f) = frame.filter(|_| info.is_some()) else {
            say("Opening the video…");
            ui.ctx().request_repaint_after(Duration::from_millis(30));
            return;
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
        if let Some((_, tex)) = &self.shown {
            let k = (rect.width() / f.width as f32).min(rect.height() / f.height as f32);
            let size = vec2(f.width as f32 * k, f.height as f32 * k);
            let pic = Rect::from_center_size(rect.center(), size);
            painter.image(
                tex.id(),
                pic,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if let (Some(info), Some(at)) = (info, at) {
            let n = info.frame_at(at);
            // Not there yet: look again shortly.
            if number(&info, &f) != n {
                ui.ctx().request_repaint_after(Duration::from_millis(15));
            }
            let text = format!(
                "{}   frame {} of {}   {}",
                c.name,
                n + 1,
                info.frames(),
                clock(at)
            );
            let galley =
                painter.layout_no_wrap(text, FontId::monospace(look.small), Color32::WHITE);
            let r = Rect::from_min_size(
                rect.left_bottom() + vec2(8.0, -galley.size().y - 12.0),
                galley.size() + vec2(8.0, 4.0),
            );
            painter.rect_filled(r, 3.0, Color32::from_black_alpha(170));
            painter.galley(r.min + vec2(4.0, 2.0), galley, Color32::WHITE);
        }
    }
}

/// Seconds as minutes, seconds and thousandths: 1:02.500.
fn clock(seconds: f64) -> String {
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
        assert_eq!(clock(62.5), "1:02.500");
        assert_eq!(clock(0.0), "0:00.000");
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
}
