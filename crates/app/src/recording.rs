// SPDX-License-Identifier: GPL-3.0-or-later
//! Recording depth in the app (roadmap 2.2): a punch range with pre-roll
//! and post-roll, retrospective capture ("keep what I just played"), and
//! a track for each input so a band can record all at once.
//!
//! Punching in by hand needs nothing here: pressing Record while playing
//! starts takes on the armed tracks, and pressing it again ends them.
//!
//! Recording at a slower speed (Phase 4 D5) needs nothing here either:
//! with the practice speed (or transpose) changed, armed tracks record as
//! played and the engine's recorder speeds each take back up to the
//! song's speed before it arrives. The speed commands are here.

use daw_engine::engine::Command;
use daw_engine::graph::Input;
use daw_engine::record::Punch;
use daw_engine::time::Frames;
use eframe::egui::{self, RichText};

use crate::{App, Audio, Track};

/// The choices for how much input retrospective capture keeps, seconds.
const CAPTURE_CHOICES: &[(u32, &str)] = &[
    (0, "Off"),
    (30, "30 seconds"),
    (60, "1 minute"),
    (120, "2 minutes"),
    (300, "5 minutes"),
];

pub struct RecordOptions {
    /// Record only between the punch bars (when Record is on).
    pub punch: bool,
    /// Bars, counted from 1: in at the start of `.0`, out at the start
    /// of `.1`.
    pub punch_bars: (i64, i64),
    /// Bars played before the punch-in and after the punch-out.
    pub pre_roll: u32,
    pub post_roll: u32,
    /// Seconds of input retrospective capture keeps; 0 is off.
    pub capture_secs: u32,
}

/// A count of bars, 0 to 8, said as "1 bar" or "2 bars".
fn bars(n: &mut u32) -> egui::DragValue<'_> {
    egui::DragValue::new(n)
        .range(0..=8)
        .custom_formatter(|n, _| match n as u32 {
            1 => "1 bar".into(),
            n => format!("{n} bars"),
        })
        .custom_parser(|s| s.trim().split(' ').next()?.parse().ok())
}

impl Default for RecordOptions {
    fn default() -> RecordOptions {
        RecordOptions {
            punch: false,
            punch_bars: (3, 5),
            pre_roll: 1,
            post_roll: 1,
            capture_secs: 120,
        }
    }
}

impl App {
    /// The interface's round trip: takes are placed this much earlier
    /// than they were captured.
    pub(crate) fn record_offset(&self) -> Frames {
        Frames(match (&self.audio, &self.profile) {
            (Audio::Running(r), Some(p)) => r.record_offset(p) as i64,
            _ => 0,
        })
    }

    /// The punch range on the timeline, if punching is on.
    fn punch(&self) -> Option<Punch> {
        let o = &self.record;
        let map = self.tempo_map();
        o.punch.then(|| Punch {
            start: map.frames_at(map.bar_start(o.punch_bars.0)),
            end: map.frames_at(map.bar_start(o.punch_bars.1)),
            pre_roll: o.pre_roll,
            post_roll: o.post_roll,
        })
    }

    /// Tells the engine the punch range, if any.
    pub(crate) fn send_punch(&mut self) {
        let offset = self.record_offset();
        let range = self.punch().map(|p| p.capture_range(offset));
        self.send(Command::SetPunch(range));
    }

    /// Before playing from stopped: with Record and a punch range on,
    /// starts the pre-roll before the punch-in and stops after the
    /// post-roll (unless looping, when each pass punches again).
    pub(crate) fn prepare_play(&mut self) {
        self.send_punch();
        let stop = match self.punch().filter(|_| self.recording) {
            Some(p) => {
                let map = self.tempo_map();
                self.send(Command::Seek(p.play_from(&map)));
                (!self.looping).then(|| p.stop_at(&map, self.record_offset()))
            }
            None => None,
        };
        self.send(Command::StopAt(stop));
    }

    /// Tells the engine how much input to keep for retrospective capture.
    pub(crate) fn send_capture(&mut self) {
        if self.ctl.set_capture(self.record.capture_secs).is_err() {
            self.message = Some("The audio engine is busy; try again.".into());
        }
    }

    /// The transport's recording row: punch range, pre-roll and
    /// post-roll, and retrospective capture.
    pub(crate) fn recording_controls(&mut self, ui: &mut egui::Ui) {
        let o = &mut self.record;
        let mut punch = ui
            .checkbox(&mut o.punch, "Punch bars")
            .on_hover_text("With Record on, record only from the start of the first bar to the start of the second")
            .changed();
        punch |= ui
            .add(egui::DragValue::new(&mut o.punch_bars.0).range(1..=999))
            .accessible_name("Punch in at bar")
            .changed();
        ui.label("to");
        punch |= ui
            .add(egui::DragValue::new(&mut o.punch_bars.1).range(2..=1000))
            .accessible_name("Punch out at bar")
            .changed();
        if o.punch_bars.1 <= o.punch_bars.0 {
            o.punch_bars.1 = o.punch_bars.0 + 1;
        }
        ui.label("Pre-roll");
        ui.add(bars(&mut o.pre_roll))
            .accessible_name("Pre-roll bars")
            .on_hover_text("Bars played before the punch-in");
        ui.label("Post-roll");
        punch |= ui
            .add(bars(&mut o.post_roll))
            .accessible_name("Post-roll bars")
            .on_hover_text("Bars played after the punch-out, then playback stops")
            .changed();
        ui.separator();

        let before = o.capture_secs;
        let label = |s: u32| {
            CAPTURE_CHOICES
                .iter()
                .find(|c| c.0 == s)
                .map_or(format!("{s} s"), |c| c.1.to_string())
        };
        ui.label("Always keep");
        egui::ComboBox::from_id_salt("capture")
            .width(90.0)
            .selected_text(label(o.capture_secs))
            .show_ui(ui, |ui| {
                for &(s, name) in CAPTURE_CHOICES {
                    ui.selectable_value(&mut o.capture_secs, s, name);
                }
            })
            .response
            .accessible_name("Always keep the last of armed tracks' input")
            .on_hover_text(
                "Armed tracks' input is kept in memory while playing without \
                 recording, so a good part played without Record on can still be kept",
            );
        let changed = o.capture_secs != before;
        let held = self.ctl.captured_frames();
        let secs = held as f64 / self.rate.max(1) as f64;
        let keep = ui
            .add_enabled(
                held > 0,
                egui::Button::new(format!(
                    "Keep last {}:{:02}",
                    secs as u64 / 60,
                    secs as u64 % 60
                )),
            )
            .on_hover_text("Make takes from armed tracks' input that wasn't recorded")
            .on_disabled_hover_text(if self.record.capture_secs == 0 {
                "Choose how much to keep first"
            } else {
                "Nothing kept yet: arm a track and play"
            });
        if keep.clicked() {
            self.ctl.keep_capture();
        }
        if changed {
            self.send_capture();
        }
        if punch && self.ctl.status().playing() {
            self.send_punch();
        }
        ui.separator();
        ui.label("Notes");
        egui::ComboBox::from_id_salt("notes-mode")
            .width(170.0)
            .selected_text(self.midi.mode.label())
            .show_ui(ui, |ui| {
                for m in [crate::midi::Mode::NewClip, crate::midi::Mode::Merge] {
                    ui.selectable_value(&mut self.midi.mode, m, m.label());
                }
            })
            .response
            .accessible_name("Notes recording mode")
            .on_hover_text(
                "Each recording pass on an instrument track makes a new note clip, or \
                 merges its notes into the clip already there (to build up a part)",
            );
        let keep_notes = ui
            .add_enabled(self.midi.has_recent(), egui::Button::new("Keep notes"))
            .on_hover_text("Make clips from notes played on instrument tracks without Record on")
            .on_disabled_hover_text(
                "Nothing played yet: monitor or arm an instrument track and play",
            );
        if keep_notes.clicked() && self.keep_notes() {
            self.rebuild();
        }
        if self.record.capture_secs > 0 && self.tracks.iter().any(|t| t.arm) {
            // The kept time counts up.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// A button beside "Add track": one armed track for each input not
    /// already used, to record a band at once. Returns true if any were
    /// added.
    pub(crate) fn input_tracks_button(&mut self, ui: &mut egui::Ui, inputs: usize) -> bool {
        let used = |c: usize| {
            self.tracks.iter().any(|t| {
                t.input
                    .is_some_and(|i| i.first == c || (i.stereo && i.first + 1 == c))
            })
        };
        let free: Vec<usize> = (0..inputs).filter(|&c| !used(c)).collect();
        let b = ui
            .add_enabled(
                !free.is_empty(),
                egui::Button::new(RichText::new(format!(
                    "+ A track for each input ({})",
                    free.len()
                ))),
            )
            .on_hover_text("Adds an armed track on every input not already in use")
            .on_disabled_hover_text("Every input already has a track");
        if !b.clicked() {
            return false;
        }
        for c in free {
            let mut t = Track::new(format!("In {}", c + 1));
            t.input = Some(Input::mono(c));
            t.arm = true;
            self.tracks.push(t);
        }
        true
    }
}

impl App {
    /// Practice speed `by` percent faster (negative: slower), to the
    /// nearest 5%, within the stretcher's range.
    pub(crate) fn nudge_speed(&mut self, by: f64) {
        self.speed_pct = (((self.speed_pct + by) / 5.0).round() * 5.0).clamp(50.0, 150.0);
        self.send(self.practice());
    }

    pub(crate) fn normal_speed(&mut self) {
        (self.speed_pct, self.semitones, self.cents) = (100.0, 0, 0);
        self.send(self.practice());
    }

    /// Said beside the practice controls while recording with them
    /// changed.
    pub(crate) fn slowed_note(&self) -> String {
        let speed = self.speed_pct != 100.0;
        let pitch = (self.semitones, self.cents) != (0, 0);
        let what = match (speed, pitch) {
            (true, true) => "speed and pitch",
            (true, false) => "speed",
            _ => "pitch",
        };
        format!(
            "Recording at {:.0}%: takes return to the song's {what} when you stop",
            self.speed_pct
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use daw_audio_io::SilentInput;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::commands::Cmd;

    /// The app on the silent stand-in with two channels of test tone in,
    /// a bar a second (240 bpm), and one armed track on input 1.
    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        let app = h.state_mut();
        let eng = app.replace_engine();
        app.audio = Audio::Silent(
            daw_audio_io::start_silent_with(app.rate, 256, eng, SilentInput::Tones),
            String::new(),
        );
        app.tempo_bpm = 240.0;
        let mut t = Track::new("Guitar".into());
        t.input = Some(Input::mono(0));
        t.arm = true;
        app.tracks.push(t);
        app.rebuild();
        h.run();
        h
    }

    fn until(h: &mut Harness<'_, App>, secs: u64, what: impl Fn(&App) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            h.step();
            if what(h.state()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_punch_records_one_bar_between_pre_and_post_roll() {
        let mut h = harness();
        let app = h.state_mut();
        app.record.punch = true;
        app.record.punch_bars = (2, 3);
        app.record.pre_roll = 1;
        app.record.post_roll = 1;
        app.recording = true;
        app.send(Command::SetRecording(true));
        // Played from bar 3: the pre-roll starts at bar 1 anyway.
        app.send(Command::Seek(Frames(3 * 48_000)));
        h.step();
        h.state_mut().toggle_play();
        assert!(until(&mut h, 2, |a| a.ctl.status().playing()));
        // Stops by itself after the post-roll, at bar 4.
        assert!(until(&mut h, 10, |a| !a.ctl.status().playing()));
        assert_eq!(h.state().ctl.status().position(), Frames(3 * 48_000));
        assert!(until(&mut h, 2, |a| !a.tracks[0].clips.is_empty()));
        let c = &h.state().tracks[0].clips[0];
        // The silent stand-in has no round trip, so the take is exactly
        // bar 2.
        assert_eq!((c.start, c.end()), (48_000, 2 * 48_000));
    }

    #[test]
    fn keeping_what_was_played_without_recording() {
        let mut h = harness();
        h.state_mut().toggle_play();
        assert!(until(&mut h, 5, |a| a.ctl.captured_frames() >= 24_000));
        h.state_mut().toggle_play();
        // The audio thread stops in its own time; until then each frame
        // asks for another.
        assert!(until(&mut h, 2, |a| !a.ctl.status().playing()));
        h.run();
        h.get_by_label_contains("Keep last").click();
        assert!(until(&mut h, 2, |a| !a.tracks[0].clips.is_empty()));
        let clips = &h.state().tracks[0].clips;
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].start, 0);
        assert!(clips[0].end() >= 24_000);
        assert_eq!(h.state().ctl.captured_frames(), 0);
    }

    #[test]
    fn a_track_for_each_free_input() {
        let mut h = harness();
        h.state_mut().tracks[0].input = Some(Input::mono(1));
        h.run();
        h.get_by_label("+ A track for each input (1)").click();
        h.run();
        let t = &h.state().tracks;
        assert_eq!(t.len(), 2);
        assert_eq!(
            (t[1].name.as_str(), t[1].input, t[1].arm),
            ("In 1", Some(Input::mono(0)), true)
        );
        // Now every input has a track.
        assert!(h.query_by_label("+ A track for each input (0)").is_some());
    }

    #[test]
    fn a_bar_recorded_at_70_percent_lands_as_one_bar() {
        let mut h = harness();
        let ctx = h.ctx.clone();
        for _ in 0..6 {
            h.state_mut().run(&ctx, Cmd::SlowerSpeed);
        }
        assert_eq!(h.state().speed_pct, 70.0);
        let app = h.state_mut();
        app.record.punch = true;
        app.record.punch_bars = (2, 3);
        app.record.pre_roll = 1;
        app.record.post_roll = 1;
        app.recording = true;
        app.send(Command::SetRecording(true));
        h.run();
        assert!(h.query_by_label_contains("Recording at 70%").is_some());
        h.state_mut().toggle_play();
        assert!(until(&mut h, 2, |a| a.ctl.status().playing()));
        // Three bars at 70% take about 4.3 seconds.
        assert!(until(&mut h, 15, |a| !a.ctl.status().playing()));
        assert!(until(&mut h, 5, |a| !a.tracks[0].clips.is_empty()));
        let c = &h.state().tracks[0].clips[0];
        // Bar 2, at the song's speed.
        assert_eq!(c.start, 48_000);
        assert!((c.end() - 2 * 48_000).abs() <= 1, "{}", c.end());
        // And back to normal.
        h.state_mut().run(&ctx, Cmd::NormalSpeed);
        assert_eq!(h.state().practice_value().speed_pct, 100.0);
    }
}
