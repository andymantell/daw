// SPDX-License-Identifier: GPL-3.0-or-later
//! Freezing, rendering in place, bouncing and pasting as rendered audio
//! (roadmap 3.6).
//!
//! - **Freeze** a track: it plays its frozen audio (see `freeze.rs`),
//!   with its effects switched off, until it is unfrozen. Its clips and
//!   effects stay as they were.
//! - **Render in place** a track, or a bus or folder with everything
//!   routed into it: the audio, through the effects and before the
//!   fader, becomes a new track beside it with the same volume, pan and
//!   routing, and the original is muted.
//! - **Bounce** the selected clips: the same, for just those clips, which
//!   are muted where they are.
//! - **Paste as rendered audio**: copied clips, through their own tracks'
//!   effects, pasted onto another track as one new clip.
//!
//! Renders run on a thread of their own with their own copies of any
//! plugins, the way an export does, and write files that wait with the
//! unsaved takes until the project is saved. Nothing is changed until a
//! render is done, and then in one edit, so undo takes it all back.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use daw_engine::automation::Target as AutoTarget;
use daw_engine::export::{Written, write_wav};
use daw_engine::graph::{Mix, Output, Session, TrackSpec};
use daw_engine::stream::StreamSource;
use daw_engine::time::Frames;
use daw_plugins::Plugin;
use daw_project::TrackKind;
use eframe::egui;

use crate::timeline::{Clip, next_id};
use crate::{App, Track, clip_spec};

/// Where rendered files go until the project is saved.
fn rendered_dir() -> PathBuf {
    crate::takes_dir().join("Rendered")
}

/// What a finished render becomes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Place {
    /// A new track beside the track (or bus) with this key. With no
    /// clips named, the whole track was rendered, and it is muted; else
    /// just those clips (by key) were, and they are muted.
    Track {
        from: String,
        clips: Option<Vec<String>>,
    },
    /// A clip on the track with this key, starting at this frame.
    Clip { lane: String, at: i64 },
}

/// One track to render, and the clips to play on it instead of its own.
pub(crate) struct Target {
    pub index: usize,
    pub clips: Option<Vec<Clip>>,
}

struct Job {
    place: Place,
    /// The new track's or clip's name.
    name: String,
    path: PathBuf,
    /// The frame the file starts at, and how much of it is lead-in
    /// before the first clip, room for effects' latency.
    from: i64,
    lead: usize,
    /// What the tracks it was made from were, so a render that no
    /// longer fits isn't put in.
    print: String,
    thread: Option<JoinHandle<Result<Written, String>>>,
    done: Arc<AtomicU64>,
    total: u64,
    cancel: Arc<AtomicBool>,
    /// The plugins' UI sides, kept until rendering ends.
    _plugins: Vec<Plugin>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Renders {
    jobs: Vec<Job>,
    /// Clips copied, with their tracks' keys.
    pub copied: Vec<(String, Clip)>,
}

impl Renders {
    pub fn busy(&self) -> usize {
        self.jobs.len()
    }

    /// How far the renders under way have got, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let (done, total) = (self.jobs.iter()).fold((0, 0), |(d, t), j| {
            (d + j.done.load(Ordering::Relaxed), t + j.total)
        });
        done as f32 / total.max(1) as f32
    }

    pub fn cancel(&mut self) {
        self.jobs.clear();
    }
}

/// The tracks a render of `targets` hears, by index: those routed into
/// a target (through any buses), those sending into one of those, and
/// the sidechains their effects listen to.
struct Involved {
    target: Vec<bool>,
    member: Vec<bool>,
    sender: Vec<bool>,
    key: Vec<bool>,
}

impl Involved {
    fn new(app: &App, s: &Session, targets: &[Target]) -> Involved {
        let n = app.tracks.len();
        let mut target = vec![false; n];
        for t in targets {
            target[t.index] = true;
        }
        let member: Vec<bool> = (0..n)
            .map(|i| {
                let mut at = i;
                // A route is never longer than the tracks.
                for _ in 0..n {
                    match s.tracks[at].output {
                        Output::Track(j) if j < n => at = j,
                        _ => return false,
                    }
                    if target[at] {
                        return !target[i];
                    }
                }
                false
            })
            .collect();
        let inside = |i: usize| target[i] || member[i];
        let sender: Vec<bool> = (0..n)
            .map(|i| !inside(i) && s.tracks[i].sends.iter().any(|x| x.to < n && inside(x.to)))
            .collect();
        let mut key = vec![false; n];
        for i in (0..n).filter(|&i| inside(i) || sender[i]) {
            for k in s.tracks[i].inserts.iter().filter_map(|x| x.sidechain) {
                if k < n {
                    key[k] = true;
                }
            }
        }
        Involved {
            target,
            member,
            sender,
            key,
        }
    }

    fn inside(&self, i: usize) -> bool {
        self.target[i] || self.member[i]
    }

    fn heard(&self, i: usize) -> bool {
        self.inside(i) || self.sender[i] || self.key[i]
    }
}

impl App {
    /// The session a render of `targets` plays: each target before its
    /// fader, straight to the master, with what it hears; everything else
    /// silent. Practice mute, solo and live input are left out. The
    /// tracks keep their places, and one more, silent, takes the audio
    /// of tracks only there for their sends or sidechains.
    fn render_session(&self, targets: &[Target]) -> (Session, Involved) {
        let mut s = self.session();
        let who = Involved::new(self, &s, targets);
        let sink = s.tracks.len();
        for (i, spec) in s.tracks.iter_mut().enumerate() {
            spec.input = None;
            spec.arm = false;
            spec.monitor = false;
            spec.cues = Default::default();
            spec.mix.mute = self.tracks[i].mix.mute;
            spec.mix.solo = false;
            // Lanes play as drawn, even one being written now.
            for l in &mut spec.automation {
                l.live = false;
            }
            if who.target[i] {
                spec.mix = Mix::default();
                spec.output = Output::Master;
                spec.sends.clear();
                spec.group = None;
                // Before the fader: its volume, pan and send lanes go to
                // the new track instead. Effects' lanes are rendered.
                spec.automation
                    .retain(|l| matches!(l.target, AutoTarget::Insert { .. }));
            } else if who.heard(i) {
                // Sends kept keep their lanes, renumbered.
                let kept: Vec<bool> = spec.sends.iter().map(|x| who.inside(x.to)).collect();
                spec.sends.retain(|x| who.inside(x.to));
                spec.automation.retain_mut(|l| match l.target {
                    AutoTarget::Send(k) if !kept.get(k).copied().unwrap_or(false) => false,
                    AutoTarget::Send(k) => {
                        l.target = AutoTarget::Send(kept[..k].iter().filter(|&&x| x).count());
                        true
                    }
                    _ => true,
                });
                if !who.member[i] {
                    spec.output = Output::Track(sink);
                }
            } else {
                spec.clips.clear();
                spec.inserts.clear();
                spec.sends.clear();
                spec.automation.clear();
                spec.output = Output::Track(sink);
            }
        }
        for t in targets {
            if let Some(clips) = &t.clips {
                s.tracks[t.index].clips = clips
                    .iter()
                    .filter(|c| !c.muted && c.source.rate == self.rate)
                    .map(clip_spec)
                    .collect();
            }
        }
        let mut quiet = TrackSpec::new(Vec::new());
        quiet.mix.mute = true;
        s.tracks.push(quiet);
        (s, who)
    }

    /// Starts rendering `targets`, to become `place`, called `name`.
    pub(crate) fn start_render(
        &mut self,
        targets: Vec<Target>,
        place: Place,
        name: String,
    ) -> Result<(), String> {
        for t in &targets {
            let track = &self.tracks[t.index];
            if t.clips.is_some() && self.frozen_playing(track).is_some() {
                return Err(if track.freeze {
                    format!("{} is frozen: unfreeze it first.", track.name)
                } else {
                    format!(
                        "{}'s effects aren't all installed here, so its clips can't be \
                         rendered through them.",
                        track.name
                    )
                });
            }
        }
        let (s, who) = self.render_session(&targets);
        let heard: Vec<usize> = (0..self.tracks.len()).filter(|&i| who.heard(i)).collect();
        // Where the audio asked for starts: the first clip heard.
        let mut first = None::<i64>;
        let mut converting = false;
        for &i in &heard {
            let t = &self.tracks[i];
            let own = targets
                .iter()
                .find(|x| x.index == i)
                .and_then(|x| x.clips.as_ref());
            for c in own.unwrap_or(&t.clips).iter().filter(|c| !c.muted) {
                converting |= c.source.rate != self.rate;
                first = Some(first.map_or(c.start, |f| f.min(c.start)));
            }
        }
        if converting {
            return Err(
                "Some of its audio is still being converted: try again in a moment.".into(),
            );
        }
        let Some(first) = first else {
            return Err("There is nothing to render there.".into());
        };
        let mut plugins = Vec::new();
        let mut fx = Vec::new();
        for (i, spec) in s.tracks.iter().enumerate().take(self.tracks.len()) {
            if spec.inserts.is_empty() {
                continue;
            }
            for slot in &self.tracks[i].fx {
                let (p, proc) = self.effects.fresh(slot, self.rate)?;
                plugins.extend(p);
                fx.push((slot.id, proc));
            }
        }
        let effects = s.tracks.iter().any(|t| t.inserts.iter().any(|x| !x.bypass));
        let graph = s
            .compile_with(daw_audio_io::MAX_BLOCK, &mut fx)
            .map_err(|e| e.to_string())?;
        let rate = self.rate as i64;
        // A second before, so effects' latency loses nothing at the
        // start, and room after for them to ring out.
        let (from, tail) = if effects {
            (
                (first - rate).max(0),
                daw_project::FROZEN_TAIL_SECONDS * rate,
            )
        } else {
            (first, 0)
        };
        let end = graph.end().0.max(first) + tail;
        let frames = (end - from).max(0) as usize;
        if frames == 0 {
            return Err("There is nothing to render there.".into());
        }
        let dir = rendered_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{}.wav", daw_project::new_id()));
        let done = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (d, c, p) = (done.clone(), cancel.clone(), path.clone());
        let thread = std::thread::spawn(move || write_wav(graph, Frames(from), frames, &p, &d, &c));
        let print = self.render_print(&heard);
        self.renders.jobs.push(Job {
            place,
            name,
            path,
            from,
            lead: (first - from) as usize,
            print,
            thread: Some(thread),
            done,
            total: frames as u64,
            cancel,
            _plugins: plugins,
        });
        Ok(())
    }

    /// What the tracks a render hears play now, to tell if they change.
    fn render_print(&self, heard: &[usize]) -> String {
        (heard.iter())
            .map(|&i| {
                let t = &self.tracks[i];
                // Its clips: lanes move effect settings as the song plays.
                let clips = self.track_value(t).freeze_shape(self.rate).clips;
                let clips: Vec<String> = (clips.iter())
                    .map(|c| format!("{}/{}/{}/{}/{}", c.asset, c.start, c.offset, c.len, c.sound))
                    .collect();
                format!("{}:{}", t.key, clips.join(";"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Takes in finished renders, once a frame. Each becomes one edit.
    pub(crate) fn poll_renders(&mut self, ctx: &egui::Context) {
        if self.renders.busy() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        while let Some(k) = (self.renders.jobs.iter())
            .position(|j| j.thread.as_ref().is_none_or(|t| t.is_finished()))
        {
            let mut job = self.renders.jobs.remove(k);
            let result = (job.thread.take()).map_or(Err("the render stopped".into()), |t| {
                t.join()
                    .unwrap_or_else(|_| Err("the render stopped".into()))
            });
            match self.place_render(&job, result) {
                Ok(()) => {
                    self.rebuild();
                    self.sync_doc(ctx, true);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&job.path);
                    if e != "cancelled" {
                        self.message = Some(format!("Couldn't render {}: {e}", job.name));
                    }
                }
            }
        }
    }

    fn place_render(&mut self, job: &Job, result: Result<Written, String>) -> Result<(), String> {
        let written = result?;
        let heard: Vec<usize> = (job.print.lines())
            .filter_map(|p| p.split(':').next())
            .filter_map(|key| self.track_index(key))
            .collect();
        if self.render_print(&heard) != job.print {
            return Err("it changed while it was being rendered: try again".into());
        }
        let source = StreamSource::open(&job.path)?;
        let mut clip = Clip::new(source, job.path.clone(), job.from, None);
        clip.offset = job.lead.min(clip.len);
        clip.len -= clip.offset;
        clip.start = job.from + clip.offset as i64;
        clip.label = job.name.clone();
        let id = clip.id;
        match &job.place {
            Place::Track { from, clips } => {
                let i = self.track_index(from).ok_or("its track is gone")?;
                let src = &self.tracks[i];
                let mut t = Track::new(job.name.clone());
                t.mix = Mix {
                    mute: false,
                    solo: false,
                    ..src.mix
                };
                t.parent = src.parent.clone();
                t.output = src.output.clone();
                t.sends = src.sends.clone();
                t.group = src.group.clone();
                // The fader's lanes, which the render left out.
                t.automation = (src.automation.iter())
                    .filter(|l| {
                        matches!(
                            l.target(),
                            daw_project::LaneTarget::Gain
                                | daw_project::LaneTarget::Pan
                                | daw_project::LaneTarget::Send(_)
                        )
                    })
                    .cloned()
                    .collect();
                t.clips.push(clip);
                let at = self.after_family(i);
                match clips {
                    None => self.tracks[i].mix.mute = true,
                    Some(keys) => {
                        for c in &mut self.tracks[i].clips {
                            if keys.contains(&c.key) {
                                c.muted = true;
                            }
                        }
                    }
                }
                self.tracks.insert(at, t);
            }
            Place::Clip { lane, .. } => {
                let i = self
                    .track_index(lane)
                    .ok_or("the track to paste onto is gone")?;
                self.tracks[i].clips.push(clip);
            }
        }
        self.timeline.selected = HashSet::from([id]);
        if written.clipped() {
            self.message = Some(format!(
                "{} went {:.1} dB over full scale and was clipped: turn its effects down and \
                 render again",
                job.name,
                20.0 * written.peak.log10()
            ));
        }
        Ok(())
    }

    /// Where a track beside track `i` goes: after it and, for a folder,
    /// the tracks in it.
    fn after_family(&self, i: usize) -> usize {
        let key = &self.tracks[i].key;
        let inside = |mut t: usize| {
            for _ in 0..self.tracks.len() {
                match self.tracks[t]
                    .parent
                    .as_ref()
                    .and_then(|p| self.track_index(p))
                {
                    Some(p) if &self.tracks[p].key == key => return true,
                    Some(p) => t = p,
                    None => return false,
                }
            }
            false
        };
        let mut at = i + 1;
        while at < self.tracks.len() && inside(at) {
            at += 1;
        }
        at
    }

    /// Renders track `i` (a bus or folder with what goes into it) to a
    /// new track beside it, and mutes it.
    pub(crate) fn render_in_place(&mut self, i: usize) {
        let Some(t) = self.tracks.get(i) else { return };
        let place = Place::Track {
            from: t.key.clone(),
            clips: None,
        };
        let name = format!("{} (rendered)", t.name);
        let r = self.start_render(
            vec![Target {
                index: i,
                clips: None,
            }],
            place,
            name,
        );
        if let Err(e) = r {
            self.message = Some(e);
        }
    }

    /// Renders the selected clips, track by track, to new tracks beside
    /// theirs, and mutes them.
    pub(crate) fn bounce_selection(&mut self) {
        let mut any = false;
        for i in 0..self.tracks.len() {
            let t = &self.tracks[i];
            let clips: Vec<Clip> = (t.clips.iter())
                .filter(|c| self.timeline.selected.contains(&c.id) && !c.muted)
                .cloned()
                .collect();
            if clips.is_empty() {
                continue;
            }
            any = true;
            let place = Place::Track {
                from: t.key.clone(),
                clips: Some(clips.iter().map(|c| c.key.clone()).collect()),
            };
            let name = format!("{} (bounced)", t.name);
            let target = Target {
                index: i,
                clips: Some(clips),
            };
            if let Err(e) = self.start_render(vec![target], place, name) {
                self.message = Some(e);
            }
        }
        if !any {
            self.message = Some("Select the clips to bounce first.".into());
        }
    }

    /// Copies the selected clips.
    pub(crate) fn copy_clips(&mut self) {
        let copied: Vec<(String, Clip)> = (self.tracks.iter())
            .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
            .filter(|(_, c)| self.timeline.selected.contains(&c.id))
            .map(|(t, c)| (t.key.clone(), c.clone()))
            .collect();
        if !copied.is_empty() {
            self.renders.copied = copied;
        }
    }

    /// Pastes the copied clips at `at`, keeping their spacing: onto
    /// track `lane`, or each onto its own track.
    pub(crate) fn paste_clips(&mut self, lane: Option<usize>, at: i64) -> bool {
        let Some(first) = self.renders.copied.iter().map(|(_, c)| c.start).min() else {
            return false;
        };
        let mut selected = HashSet::new();
        for (key, c) in &self.renders.copied {
            let Some(i) = lane.or_else(|| self.track_index(key)) else {
                continue;
            };
            let Some(t) = self
                .tracks
                .get_mut(i)
                .filter(|t| t.kind == TrackKind::Audio)
            else {
                continue;
            };
            let mut c = Clip {
                id: next_id(),
                key: daw_project::new_id(),
                label: c.name(),
                take: None,
                muted: false,
                comp: false,
                ..c.clone()
            };
            c.start = (at + c.start - first).max(0);
            selected.insert(c.id);
            t.clips.push(c);
        }
        let any = !selected.is_empty();
        if any {
            self.timeline.selected = selected;
        }
        any
    }

    /// Renders the copied clips through their own tracks' effects, and
    /// pastes the audio onto track `lane` at `at`.
    pub(crate) fn paste_rendered(&mut self, lane: usize, at: i64) {
        let Some(t) = self.tracks.get(lane).filter(|t| t.kind == TrackKind::Audio) else {
            return;
        };
        let lane_key = t.key.clone();
        let mut targets: Vec<Target> = Vec::new();
        let mut names = Vec::new();
        for (key, c) in &self.renders.copied {
            let Some(i) = self.track_index(key) else {
                continue;
            };
            match targets.iter_mut().find(|t| t.index == i) {
                Some(t) => t.clips.get_or_insert_default().push(c.clone()),
                None => {
                    names.push(c.name());
                    targets.push(Target {
                        index: i,
                        clips: Some(vec![c.clone()]),
                    });
                }
            }
        }
        if targets.is_empty() {
            self.message = Some("Copy some clips first; their tracks must still be here.".into());
            return;
        }
        // The render starts at the first clip; pasted, that lands at `at`.
        let first = (self.renders.copied.iter())
            .map(|(_, c)| c.start)
            .min()
            .unwrap_or(0);
        let place = Place::Clip { lane: lane_key, at };
        let name = format!("{} (rendered)", names.join(" + "));
        let before = self.renders.jobs.len();
        if let Err(e) = self.start_render(targets, place, name) {
            self.message = Some(e);
        }
        // Moved from where it was copied to where it is pasted.
        if let Some(job) = self.renders.jobs.get_mut(before) {
            let shift = at - first;
            job.from += shift;
            if job.from < 0 {
                // Before the song's start: lose some of the lead-in.
                let cut = (-job.from) as usize;
                job.lead = job.lead.saturating_sub(cut);
                job.from = 0;
            }
        }
    }

    /// Freezes track `i`, or unfreezes it.
    pub(crate) fn toggle_freeze(&mut self, ctx: &egui::Context, i: usize) {
        let Some(t) = self.tracks.get_mut(i) else {
            return;
        };
        t.freeze = !t.freeze;
        let (on, name) = (t.freeze, t.name.clone());
        self.rebuild();
        self.sync_doc(ctx, true);
        if !on {
            return;
        }
        if self.proj.folder.is_none() {
            self.message = Some(format!(
                "{name} freezes once the project is saved: frozen audio is kept in its folder."
            ));
            return;
        }
        self.write_plugin_states();
        self.freeze_tracks();
    }
}

/// Whether a track has anything to freeze: an effect switched on.
pub(crate) fn can_freeze(t: &Track) -> bool {
    t.fx.iter().any(|s| !s.bypass)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use daw_engine::engine::render_with;
    use daw_engine::time::Frames;
    use egui_kittest::Harness;

    use super::*;
    use crate::App;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-render-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    fn until(h: &mut Harness<'_, App>, what: impl Fn(&App) -> bool) {
        for _ in 0..1000 {
            h.step();
            if what(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    /// Half a second of a tone, mono, at `rate`, called `name`.
    fn tone(dir: &Path, name: &str, rate: u32, hz: f32) -> PathBuf {
        let path = dir.join(format!("{name}.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate / 2 {
            let x = (i as f32 * hz * std::f32::consts::TAU / rate as f32).sin();
            w.write_sample((x * 16_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    /// What the app plays, from the start, rendered offline in blocks
    /// as long as a render's: lanes are read once a block.
    fn render(app: &mut App, frames: usize) -> Vec<f32> {
        let s = app.session();
        let mut keep = Vec::new();
        let mut fx = Vec::new();
        for slot in app.tracks.iter().flat_map(|t| &t.fx) {
            let (p, proc) = app.effects.fresh(slot, app.rate).unwrap();
            keep.push(p);
            fx.push((slot.id, proc));
        }
        let g = s.compile_with(daw_audio_io::MAX_BLOCK, &mut fx).unwrap();
        let block = daw_engine::export::LANE_BLOCK;
        let mut out = Vec::new();
        render_with(g, Frames(0), frames, block, |l, r| {
            for (a, b) in l.iter().zip(r) {
                out.extend([*a, *b]);
            }
            true
        });
        out
    }

    fn worst(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
    }

    fn loudest(a: &[f32]) -> f32 {
        a.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    /// An app with two tone tracks, the second starting at a second.
    fn two_tracks(dir: &Path) -> Harness<'static, App> {
        let mut h = harness();
        let rate = h.state().rate;
        h.state_mut().add_file(&tone(dir, "Guitar", rate, 220.0));
        h.state_mut().add_file(&tone(dir, "Bass", rate, 110.0));
        until(&mut h, |a| a.tracks.len() == 2);
        h.state_mut().tracks[1].clips[0].start = rate as i64;
        h.state_mut().rebuild();
        h.run();
        h
    }

    /// A built-in delay, on track `i`: it changes the sound, and rings
    /// on after the clips.
    fn add_effect(h: &mut Harness<'_, App>, i: usize) {
        let app = h.state_mut();
        let slot = app
            .effects
            .create_builtin(daw_effects::Kind::Delay, app.rate);
        app.tracks[i].fx.push(slot);
        app.rebuild();
        h.run();
    }

    fn wait(h: &mut Harness<'_, App>) {
        until(h, |a| a.renders.busy() == 0);
        h.run();
    }

    #[test]
    fn rendering_in_place_sounds_the_same_and_undo_takes_it_back() {
        let dir = scratch("in-place");
        let mut h = two_tracks(&dir);
        add_effect(&mut h, 0);
        let app = h.state_mut();
        app.tracks[0].mix.gain_db = -6.0;
        app.tracks[0].mix.pan = -0.5;
        let rate = app.rate as usize;
        let frames = 5 * rate / 2;
        let before = render(app, frames);
        assert!(loudest(&before) > 0.1);

        app.render_in_place(0);
        assert_eq!(app.renders.busy(), 1);
        wait(&mut h);
        let app = h.state_mut();
        assert_eq!(app.message, None);
        assert_eq!(app.tracks.len(), 3);
        let t = &app.tracks[1];
        assert_eq!(t.name, "Guitar (rendered)");
        assert_eq!((t.mix.gain_db, t.mix.pan), (-6.0, -0.5));
        assert!(t.fx.is_empty());
        assert_eq!(t.clips.len(), 1);
        assert_eq!(t.clips[0].start, 0, "where the guitar starts");
        assert!(app.tracks[0].mix.mute, "the original is kept, muted");
        assert_eq!(app.tracks[0].fx.len(), 1);
        let after = render(app, frames);
        assert!(worst(&before, &after) < 1e-4, "{}", worst(&before, &after));

        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        let app = h.state();
        assert_eq!(app.tracks.len(), 2);
        assert!(!app.tracks[0].mix.mute);
        h.state_mut().redo(&ctx);
        h.run();
        assert_eq!(h.state().tracks.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renders_follow_automation() {
        let dir = scratch("automation");
        let mut h = two_tracks(&dir);
        add_effect(&mut h, 0);
        let app = h.state_mut();
        let rate = app.rate as i64;
        let effect = app.tracks[0].fx[0].key.clone();
        let line = |param: String, a: f64, b: f64| daw_project::Lane {
            param,
            points: vec![
                daw_project::AutoPoint { at: 0, value: a },
                daw_project::AutoPoint {
                    at: rate / 2,
                    value: b,
                },
            ],
        };
        // The volume fades down and the delay's mix comes up.
        app.tracks[0].automation = vec![
            line(daw_project::Lane::GAIN.into(), 0.0, -24.0),
            line(format!("{}{effect}:mix", daw_project::Lane::FX), 0.0, 100.0),
        ];
        app.rebuild();
        h.run();
        let app = h.state_mut();
        let frames = 5 * rate as usize / 2;
        let before = render(app, frames);
        app.tracks[0].automation.clear();
        app.rebuild();
        let flat = render(app, frames);
        assert!(worst(&before, &flat) > 0.05, "the lanes are heard");
        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        assert_eq!(h.state().tracks[0].automation.len(), 2);

        h.state_mut().render_in_place(0);
        wait(&mut h);
        let app = h.state_mut();
        assert_eq!(app.message, None);
        let params: Vec<&str> = (app.tracks[1].automation.iter())
            .map(|l| l.param.as_str())
            .collect();
        assert_eq!(
            params,
            [daw_project::Lane::GAIN],
            "the volume lane moves over"
        );
        let after = render(app, frames);
        assert!(worst(&before, &after) < 1e-4, "{}", worst(&before, &after));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_renders_with_everything_in_it() {
        let dir = scratch("folder");
        let mut h = two_tracks(&dir);
        let app = h.state_mut();
        let mut folder = Track::new("Band".into());
        folder.kind = TrackKind::Folder;
        folder.mix.gain_db = -3.0;
        let key = folder.key.clone();
        app.tracks.insert(0, folder);
        app.tracks[1].parent = Some(key.clone());
        app.tracks[2].parent = Some(key.clone());
        app.tracks[2].mix.gain_db = -12.0;
        app.rebuild();
        h.run();
        add_effect(&mut h, 0);
        let app = h.state_mut();
        let frames = 7 * app.rate as usize / 2;
        let before = render(app, frames);

        app.render_in_place(0);
        wait(&mut h);
        let app = h.state_mut();
        assert_eq!(app.message, None);
        assert_eq!(app.tracks.len(), 4);
        // After the folder and its tracks, outside it.
        let t = &app.tracks[3];
        assert_eq!(t.name, "Band (rendered)");
        assert_eq!(t.parent, None);
        assert_eq!(t.mix.gain_db, -3.0);
        assert!(app.tracks[0].mix.mute);
        let after = render(app, frames);
        assert!(worst(&before, &after) < 1e-4, "{}", worst(&before, &after));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bouncing_mutes_just_the_clips_bounced() {
        let dir = scratch("bounce");
        let mut h = two_tracks(&dir);
        let rate = h.state().rate as i64;
        let app = h.state_mut();
        // The guitar in two halves; the second is bounced.
        let at = rate / 4;
        app.timeline.selected.clear();
        app.with_song(|tl, song| tl.split_at(song, at));
        let second = app.tracks[0].clips[1].id;
        app.timeline.selected = HashSet::from([second]);
        app.rebuild();
        h.run();
        add_effect(&mut h, 0);
        let app = h.state_mut();
        let frames = 5 * rate as usize / 2;
        let before = render(app, frames);
        app.bounce_selection();
        wait(&mut h);
        let app = h.state_mut();
        assert_eq!(app.message, None);
        assert_eq!(app.tracks.len(), 3);
        let g = &app.tracks[0];
        assert!(!g.mix.mute);
        assert!(!g.clips[0].muted && g.clips[1].muted);
        let t = &app.tracks[1];
        assert_eq!(t.name, "Guitar (bounced)");
        assert_eq!(t.clips[0].start, at);
        let after = render(app, frames);
        assert!(worst(&before, &after) < 1e-4, "{}", worst(&before, &after));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copied_clips_paste_as_themselves_or_as_rendered_audio() {
        let dir = scratch("paste");
        let mut h = two_tracks(&dir);
        add_effect(&mut h, 0);
        let rate = h.state().rate as i64;
        let app = h.state_mut();
        let id = app.tracks[0].clips[0].id;
        app.timeline.selected = HashSet::from([id]);
        app.copy_clips();
        assert_eq!(app.renders.copied.len(), 1);

        // As itself, on its own track, at the playhead.
        assert!(app.paste_clips(None, 3 * rate));
        let c = &app.tracks[0].clips[1];
        assert_eq!(c.start, 3 * rate);
        assert_ne!(c.key, app.tracks[0].clips[0].key);
        assert_eq!(app.timeline.selected, HashSet::from([c.id]));
        app.tracks[0].clips.pop();

        // Rendered through the guitar's effect, onto the bass.
        app.tracks[1].mix.mute = true;
        let guitar = render(app, 2 * rate as usize);
        app.tracks[1].mix.mute = false;
        app.paste_rendered(1, 4 * rate);
        wait(&mut h);
        let app = h.state_mut();
        assert_eq!(app.message, None);
        let c = app.tracks[1].clips.last().unwrap();
        assert_eq!(c.start, 4 * rate);
        assert_eq!(c.label, "Guitar (rendered)");
        // Heard alone, it sounds as the guitar did through its effect.
        for t in &mut app.tracks {
            t.mix.mute = true;
        }
        app.tracks[1].mix.mute = false;
        app.tracks[1].clips.retain(|c| c.start == 4 * rate);
        app.tracks[1].clips[0].start = 0;
        let pasted = render(app, 2 * rate as usize);
        assert!(
            worst(&guitar, &pasted) < 1e-4,
            "{}",
            worst(&guitar, &pasted)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_render_whose_clips_change_meanwhile_is_left_out() {
        let dir = scratch("changed");
        let mut h = two_tracks(&dir);
        let app = h.state_mut();
        app.render_in_place(0);
        app.tracks[0].clips[0].start += 100;
        wait(&mut h);
        let app = h.state();
        assert_eq!(app.tracks.len(), 2);
        assert!(app.message.as_ref().unwrap().contains("try again"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frozen_track_plays_its_frozen_audio_with_its_effects_off() {
        let dir = scratch("freeze");
        let mut h = two_tracks(&dir);
        add_effect(&mut h, 0);
        let rate = h.state().rate as usize;
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &dir.join("Song")).unwrap();
        h.run();
        let before = render(h.state_mut(), 5 * rate / 2);
        assert!(can_freeze(&h.state().tracks[0]));
        h.state_mut().toggle_freeze(&ctx, 0);
        until(&mut h, |a| !a.freezer.busy());
        h.run();
        let app = h.state_mut();
        assert!(app.tracks[0].freeze);
        assert!(app.tracks[0].frozen.is_some());
        assert!(app.frozen_playing(&app.tracks[0]).is_some());
        let s = app.session();
        assert!(
            s.tracks[0].inserts.iter().all(|x| x.bypass),
            "its effects are off"
        );
        let after = render(app, 5 * rate / 2);
        assert!(worst(&before, &after) < 1e-4, "{}", worst(&before, &after));

        // Kept in the project.
        h.state_mut().save(&ctx).unwrap();
        let (_, bytes) = daw_project::ProjectFolder::open(&dir.join("Song")).unwrap();
        let p = daw_project::ProjectDoc::from_bytes(&bytes, 1)
            .unwrap()
            .get();
        assert!(p.tracks[0].freeze);

        // Undo unfreezes, and so does the menu.
        h.state_mut().toggle_freeze(&ctx, 0);
        h.run();
        let app = h.state();
        assert!(!app.tracks[0].freeze);
        assert!(app.frozen_playing(&app.tracks[0]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
