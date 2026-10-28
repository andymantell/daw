// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice settings kept in the project (roadmap 1.4): speed, transpose,
//! practice mute, the loop and the tempo trainer. Each computer keeps its
//! own, so a student's slowed-down loop with the teacher's part muted
//! is not the teacher's.

use daw_project::Practice;
use eframe::egui;

use crate::App;
use crate::share::device_id;

impl App {
    /// This computer's practice settings as the app has them now.
    pub(crate) fn practice_value(&self) -> Practice {
        Practice {
            speed_pct: self.speed_pct,
            semitones: self.semitones,
            cents: self.cents,
            looping: self.looping,
            muted: (self.tracks.iter())
                .filter(|t| t.practice_mute)
                .map(|t| t.key.clone())
                .collect(),
            trainer: self.trainer.settings(),
        }
    }

    /// Writes any change to this computer's practice settings into the
    /// document, unless one is under way (a value being dragged), as
    /// [`sync_doc`](App::sync_doc) does.
    pub(crate) fn sync_practice(&mut self, ctx: &egui::Context, force: bool) {
        if !force && (ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input()) {
            return;
        }
        if self.proj.loading() {
            return;
        }
        let p = self.practice_value();
        if p == self.proj.practice {
            return;
        }
        if self.proj.doc.set_practice(device_id(), &p) {
            self.proj.dirty = true;
        }
        self.proj.practice = p;
    }

    /// Takes on this computer's practice settings from the document, or
    /// the defaults if it has none there.
    pub(crate) fn apply_practice(&mut self) {
        let p = self.proj.doc.practice(device_id()).unwrap_or_default();
        self.speed_pct = p.speed_pct.clamp(50.0, 150.0);
        self.semitones = p.semitones.clamp(-12, 12);
        self.cents = p.cents.clamp(-50, 50);
        self.looping = p.looping;
        for t in &mut self.tracks {
            t.practice_mute = p.muted.contains(&t.key);
        }
        self.trainer.set(&p.trainer);
        self.proj.practice = self.practice_value();
        self.rebuild();
        self.send(self.practice());
        self.send_loop();
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;

    use crate::{App, Track};

    fn harness() -> Harness<'static, App> {
        Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()))
    }

    #[test]
    fn practice_settings_come_back_with_the_project() {
        let dir = std::env::temp_dir().join(format!("daw-practice-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut h = harness();
        let app = h.state_mut();
        app.tracks.push(Track::new("Backing".into()));
        app.tracks.push(Track::new("Riff".into()));
        app.tracks[1].practice_mute = true;
        app.speed_pct = 70.0;
        app.semitones = -1;
        app.cents = 15;
        app.looping = true;
        app.trainer.on = true;
        app.trainer.step = 3.0;
        app.trainer.every = 4;
        app.trainer.target = 90.0;
        let riff = app.tracks[1].key.clone();
        let want = app.practice_value();
        h.run();
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &dir).unwrap();

        // A new project starts with none of it.
        h.state_mut().new_project();
        let app = h.state();
        assert_eq!(app.practice_value(), daw_project::Practice::default());

        h.state_mut().open_project(dir.clone());
        for _ in 0..200 {
            h.step();
            if !h.state().proj.loading() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let app = h.state();
        assert_eq!(app.practice_value(), want);
        let muted: Vec<&str> = (app.tracks.iter())
            .filter(|t| t.practice_mute)
            .map(|t| t.key.as_str())
            .collect();
        assert_eq!(muted, vec![riff.as_str()]);
        assert!(
            !app.tracks[1].mix.mute,
            "practice mute stays apart from the mix's"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
