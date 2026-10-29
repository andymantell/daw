// SPDX-License-Identifier: GPL-3.0-or-later
//! Normalise, reverse or change the gain of clips (roadmap 2.1), from the
//! clip menu. Each clip's whole file is processed into a new file on a
//! thread of its own, so the clip can still be dragged longer; the clip
//! then plays the same part of the new file, in the same place. It keeps
//! its id in the project, so undo points it back at the original, which
//! is never changed.
//!
//! New files wait with the unsaved takes, and move into the project's
//! folder when it is saved.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use daw_engine::process::{self, Process};
use daw_engine::stream::StreamSource;

use crate::Track;
use crate::timeline::Clip;

/// Where processed files go until the project is saved.
fn processed_dir() -> PathBuf {
    crate::takes_dir().join("Processed")
}

type Result = std::result::Result<(Arc<StreamSource>, f32), String>;

/// A clip being processed.
struct Job {
    clip: u64,
    how: Process,
    /// What the clip played when it started, so a clip edited meanwhile
    /// isn't given audio that no longer fits.
    from: (usize, usize, usize),
    done: mpsc::Receiver<Result>,
}

#[derive(Default)]
pub struct Processing {
    jobs: Vec<Job>,
}

fn playing(c: &Clip) -> (usize, usize, usize) {
    (Arc::as_ptr(&c.source) as usize, c.offset, c.len)
}

impl Processing {
    /// Starts processing every clip in `selected`.
    pub fn start(&mut self, tracks: &[Track], selected: &HashSet<u64>, how: Process) {
        let dir = processed_dir();
        for c in tracks.iter().flat_map(|t| &t.clips) {
            if selected.contains(&c.id) {
                self.start_one(c, how, dir.join(format!("{}.wav", daw_project::new_id())));
            }
        }
    }

    fn start_one(&mut self, c: &Clip, how: Process, out: PathBuf) {
        let (tx, rx) = mpsc::channel();
        let (source, part) = (c.source.clone(), c.offset..c.offset + c.len);
        std::thread::spawn(move || {
            let r = process::process(&source, part, how, &out)
                .and_then(|p| Ok((StreamSource::open(&out)?, p.peak)));
            let _ = tx.send(r);
        });
        self.jobs.push(Job {
            clip: c.id,
            how,
            from: playing(c),
            done: rx,
        });
    }

    /// How many clips are being processed.
    pub fn busy(&self) -> usize {
        self.jobs.len()
    }

    /// Points clips that have finished at their new audio. Returns true
    /// if any changed; problems go in `message`.
    pub fn finish(&mut self, tracks: &mut [Track], message: &mut Option<String>) -> bool {
        let mut changed = false;
        let mut i = 0;
        while i < self.jobs.len() {
            let result = match self.jobs[i].done.try_recv() {
                Ok(r) => r,
                Err(mpsc::TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("the processing stopped".into()),
            };
            let job = self.jobs.remove(i);
            let clip = (tracks.iter_mut().flat_map(|t| &mut t.clips)).find(|c| c.id == job.clip);
            match (result, clip) {
                // Deleted meanwhile: nothing to do.
                (_, None) => {}
                (Err(e), Some(c)) => *message = Some(format!("Couldn't process {}: {e}", c.name())),
                (Ok(_), Some(c)) if playing(c) != job.from => {
                    *message = Some(format!(
                        "{} changed while it was being processed: try again",
                        c.name()
                    ));
                }
                (Ok((source, peak)), Some(c)) => {
                    let path = source.path.clone();
                    c.asset = path.to_string_lossy().into_owned();
                    c.path = path;
                    if job.how == Process::Reverse {
                        // The same part, now backwards, with the volume
                        // line staying with the audio it was drawn on.
                        let frames = source.frames;
                        c.offset = process::reversed(c.offset + c.len - 1, frames);
                        for p in &mut c.envelope {
                            p.at = process::reversed(p.at, frames);
                        }
                        c.envelope.reverse();
                    }
                    c.source = source;
                    c.label = format!("{} ({})", c.label, job.how.label());
                    if peak > 1.0 {
                        *message = Some(format!(
                            "{} is now louder than full scale: turn the track down to keep it from distorting",
                            c.name()
                        ));
                    }
                    changed = true;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::tests::clip;
    use daw_engine::fade::GainPoint;

    fn track(clips: Vec<Clip>) -> Track {
        let mut t = Track::new("Guitar".into());
        t.clips = clips;
        t
    }

    fn wait(p: &mut Processing, tracks: &mut [Track], msg: &mut Option<String>) -> bool {
        let mut changed = false;
        while p.busy() > 0 {
            changed |= p.finish(tracks, msg);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        changed
    }

    fn out() -> PathBuf {
        std::env::temp_dir()
            .join(format!("daw-app-process-{}", std::process::id()))
            .join(format!("{}.wav", daw_project::new_id()))
    }

    #[test]
    fn a_processed_clip_plays_the_same_part_of_its_new_file() {
        let mut c = clip(4_800, 10_000);
        c.trim_start(5_800);
        c.trim_end(11_800);
        c.envelope = vec![
            GainPoint {
                at: 1_000,
                db: -3.0,
            },
            GainPoint { at: 6_999, db: 0.0 },
        ];
        let (before, key) = (c.source.clone(), c.key.clone());
        let mut tracks = vec![track(vec![c])];
        let mut p = Processing::default();
        p.start_one(&tracks[0].clips[0], Process::Reverse, out());
        let mut msg = None;
        assert!(wait(&mut p, &mut tracks, &mut msg));
        assert_eq!(msg, None);
        let c = &mut tracks[0].clips[0];
        // Frames 1000..7000 of 10000, backwards, are 3000..9000.
        assert_eq!((c.start, c.offset, c.len), (5_800, 3_000, 6_000));
        let at: Vec<(usize, f32)> = c.envelope.iter().map(|p| (p.at, p.db)).collect();
        assert_eq!(at, [(3_000, 0.0), (8_999, -3.0)]);
        assert_eq!(c.key, key, "the same clip, so undo puts the old file back");
        assert_eq!(c.source.frames, 10_000);
        assert_eq!(c.asset, c.source.path.to_string_lossy());
        assert!(c.label.ends_with("(reversed)"));
        // The whole file was processed, so the clip can be dragged longer.
        c.trim_start(0);
        c.trim_end(20_000);
        assert_eq!((c.start, c.offset, c.len), (2_800, 0, 10_000));
        // The original is untouched.
        assert!(before.path.exists());
        assert_ne!(before.path, c.source.path);
    }

    #[test]
    fn a_clip_trimmed_or_deleted_meanwhile_is_left_alone() {
        let mut tracks = vec![track(vec![clip(0, 2_000), clip(4_000, 2_000)])];
        let mut p = Processing::default();
        for c in &tracks[0].clips {
            p.start_one(c, Process::Gain(3.0), out());
        }
        tracks[0].clips[0].trim_end(1_000);
        tracks[0].clips.remove(1);
        let before = tracks[0].clips[0].source.clone();
        let mut msg = None;
        assert!(!wait(&mut p, &mut tracks, &mut msg));
        assert!(Arc::ptr_eq(&tracks[0].clips[0].source, &before));
        assert!(msg.unwrap().contains("try again"));
    }

    #[test]
    fn undo_puts_the_original_audio_back_and_redo_the_new() {
        let mut h = egui_kittest::Harness::builder()
            .with_size([1200.0, 800.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| crate::App::new(Vec::new()));
        h.run();
        let c = clip(0, 4_800);
        let (id, original) = (c.id, c.asset.clone());
        h.state_mut().tracks.push(track(vec![c]));
        h.run();
        let app = h.state_mut();
        app.timeline.selected.insert(id);
        let selected = app.timeline.selected.clone();
        app.processing
            .start(&app.tracks, &selected, Process::Reverse);
        // The status bar's spinner keeps repainting while it works.
        while h.state().processing.busy() > 0 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.run();
        let processed = h.state().tracks[0].clips[0].asset.clone();
        assert_ne!(processed, original);
        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        assert_eq!(h.state().tracks[0].clips[0].asset, original);
        h.state_mut().redo(&ctx);
        h.run();
        let c = &h.state().tracks[0].clips[0];
        assert_eq!(c.asset, processed);
        assert_eq!(c.source.path.to_string_lossy(), processed);
        let _ = std::fs::remove_file(&processed);
    }
}
