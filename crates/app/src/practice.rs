// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice settings kept in the project (roadmap 1.4): speed, transpose,
//! practice mute, the loop and the tempo trainer. Each computer keeps its
//! own, so a student's slowed-down loop with the teacher's part muted
//! is not the teacher's.
//!
//! The practice log (roadmap 3.7) is kept here too: each stretch of
//! playing, from play to stop or until the loop moves, is written to the
//! project with its time, times round the loop and top speed.

use daw_project::{Practice, PracticeSession};
use eframe::egui;

use crate::App;
use crate::share::device_id;

/// Playing for less than this isn't worth a line in the log.
pub(crate) const MIN_SESSION_SECONDS: f64 = 10.0;

/// A frame longer than this (the app was hidden, say) counts as this.
const MAX_STEP_SECONDS: f64 = 1.0;

/// What is playing now, for the log.
pub(crate) struct Playing {
    pub(crate) who: String,
    /// The bars looped, or `None` without the loop.
    pub(crate) bars: Option<(i64, i64)>,
    /// The engine's count of times round the loop.
    pub(crate) loops: u64,
    /// Percent.
    pub(crate) speed: f64,
    /// The song's tempo where the bars start.
    pub(crate) bpm: f64,
}

/// The practice session going on now, if any.
#[derive(Default)]
pub(crate) struct Logger {
    going: Option<Going>,
}

struct Going {
    session: PracticeSession,
    /// The engine's loop count when it began.
    loops_from: u64,
    /// The time, in seconds, last counted up to.
    last: f64,
}

impl Logger {
    /// Counts up to `now` (seconds, on any steady clock), given what is
    /// playing, if anything. Returns a session that has just ended, if
    /// it is long enough to keep. `wall_ms` is the time of day, for
    /// when a new one began.
    pub(crate) fn tick(
        &mut self,
        now: f64,
        wall_ms: i64,
        playing: Option<Playing>,
    ) -> Option<PracticeSession> {
        let Some(p) = playing else {
            return self.end();
        };
        if let Some(g) = &mut self.going
            && g.session.bars == p.bars
            && g.session.who == p.who
        {
            let s = &mut g.session;
            s.seconds += (now - g.last).clamp(0.0, MAX_STEP_SECONDS);
            s.loops = p.loops.saturating_sub(g.loops_from);
            s.top_speed = s.top_speed.max(p.speed);
            s.top_bpm = s.top_bpm.max(p.bpm * p.speed / 100.0);
            g.last = now;
            return None;
        }
        let ended = self.end();
        self.going = Some(Going {
            session: PracticeSession {
                id: daw_project::new_id(),
                who: p.who,
                started: wall_ms,
                seconds: 0.0,
                bars: p.bars,
                loops: 0,
                top_speed: p.speed,
                top_bpm: p.bpm * p.speed / 100.0,
            },
            loops_from: p.loops,
            last: now,
        });
        ended
    }

    /// The session going on now, if it is long enough to keep so far.
    pub(crate) fn current(&self) -> Option<&PracticeSession> {
        let s = &self.going.as_ref()?.session;
        (s.seconds >= MIN_SESSION_SECONDS).then_some(s)
    }

    /// Stops counting, returning the session if it is worth keeping.
    pub(crate) fn end(&mut self) -> Option<PracticeSession> {
        let s = self.going.take()?.session;
        (s.seconds >= MIN_SESSION_SECONDS).then_some(s)
    }
}

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

    /// Who the practice log is kept under: the member's key once the
    /// project has members, or else this computer's id.
    pub(crate) fn my_log_id(&self) -> String {
        if cfg!(test) {
            return "me".into();
        }
        match self.proj.doc.my_key() {
            Some(k) => k.to_string(),
            None => device_id().to_string(),
        }
    }

    /// Counts practice for the log, and writes a session to the
    /// project when it ends.
    pub(crate) fn log_practice(&mut self, ctx: &egui::Context) {
        if self.proj.loading() {
            return;
        }
        let st = self.ctl.status();
        let playing = st.playing().then(|| {
            let bars = self.looping.then_some(self.loop_bars);
            let map = self.tempo_map();
            let bar = bars.map_or(1, |b| b.0);
            Playing {
                who: self.my_log_id(),
                bars,
                loops: st.loops(),
                speed: self.speed_pct,
                bpm: map.tempo_at(map.bar_start(bar)).as_f64(),
            }
        });
        let now = ctx.input(|i| i.time);
        if let Some(s) = self
            .practice_log
            .tick(now, crate::lessons::now_ms(), playing)
        {
            self.write_session(&s);
        }
    }

    /// Writes the session going on now, so far, so a save keeps it.
    pub(crate) fn flush_practice(&mut self) {
        if let Some(s) = self.practice_log.current().cloned() {
            self.write_session(&s);
        }
    }

    fn write_session(&mut self, s: &PracticeSession) {
        if self.proj.doc.log_practice(s) {
            self.proj.dirty = true;
        }
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

    use super::{Logger, Playing};

    fn playing(bars: Option<(i64, i64)>, loops: u64, speed: f64) -> Option<Playing> {
        Some(Playing {
            who: "me".into(),
            bars,
            loops,
            speed,
            bpm: 120.0,
        })
    }

    #[test]
    fn the_log_counts_time_loops_and_top_speed_per_loop() {
        let mut log = Logger::default();
        let verse = Some((9, 17));
        assert!(log.tick(0.0, 1_000, playing(verse, 40, 60.0)).is_none());
        let mut t = 0.0;
        for loops in 40..52 {
            t += 1.0;
            let speed = 60.0 + (loops - 40) as f64;
            assert!(log.tick(t, 0, playing(verse, loops, speed)).is_none());
        }
        // A long gap (the window hidden) counts as a moment.
        assert!(log.tick(t + 30.0, 0, playing(verse, 52, 80.0)).is_none());
        assert_eq!(log.current().unwrap().seconds, 13.0);
        // Moving the loop ends it and starts another.
        let s = log.tick(t + 31.0, 2_000, playing(Some((1, 9)), 52, 80.0));
        let s = s.unwrap();
        assert_eq!(s.started, 1_000);
        assert_eq!(s.seconds, 13.0);
        assert_eq!(s.bars, verse);
        assert_eq!(s.loops, 12);
        assert_eq!(s.top_speed, 80.0);
        assert_eq!(s.top_bpm, 96.0);
        // Stopping soon after is too short to keep.
        assert!(log.tick(t + 33.0, 0, None).is_none());
        assert!(log.current().is_none());
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
