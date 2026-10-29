// SPDX-License-Identifier: GPL-3.0-or-later
//! The phone remote's desktop side (roadmap 2.2). Settings → Phone
//! remote… shows a code to scan with the Android app, lists the phones
//! that have paired, and between frames the app answers what connected
//! phones ask for and tells them where playback is. The protocol and
//! the server live in `daw_remote`; this module ties them to the app.
//!
//! The desktop stays in charge: a phone's request is applied exactly as
//! the same button on the desktop would be, on the UI thread, and a
//! phone that vanishes mid-take changes nothing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use daw_engine::engine::Command;
use daw_engine::time::Frames;
use daw_remote::server::DEFAULT_PORT;
use daw_remote::{Devices, Event, FromPhone, QrImage, Server, ToPhone};
use eframe::egui::{self, Color32, RichText};

use crate::{App, timeline};

/// How often phones hear where playback is.
const STATUS_EVERY: Duration = Duration::from_millis(66);

#[derive(Default)]
pub(crate) struct Remote {
    pub(crate) open: bool,
    server: Option<Server>,
    devices: Devices,
    /// The code on show: the text in the image, and the image.
    showing: Option<(String, QrImage)>,
    connected: Vec<(u64, String)>,
    error: Option<String>,
    /// Something to tell the user in the window ("Phone paired").
    note: Option<String>,
    last_status: Option<Instant>,
    last_layout: Option<ToPhone>,
    /// The clips of the last recording, as (track key, clip key), for
    /// "undo last take" from a phone.
    last_take: Vec<(String, String)>,
}

/// Where the paired phones are kept, beside the settings.
fn devices_path() -> PathBuf {
    crate::config_dir().join("phones.json")
}

/// This computer's name, for the phone's screen.
fn desktop_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Desktop".into())
}

impl Remote {
    /// Loads the paired phones and, if there are any, starts listening
    /// so they can connect without the window being opened. Tests never
    /// touch the network or the user's settings.
    pub(crate) fn load() -> Remote {
        let mut r = Remote::default();
        if cfg!(test) {
            return r;
        }
        r.devices = std::fs::read_to_string(devices_path())
            .map(|t| Devices::from_text(&t))
            .unwrap_or_default();
        if !r.devices.phones.is_empty() {
            r.start();
        }
        r
    }

    /// Starts listening, if not already.
    fn start(&mut self) {
        if self.server.is_some() {
            return;
        }
        match Server::start(DEFAULT_PORT, &desktop_name(), self.devices.clone()) {
            Ok(s) => {
                self.server = Some(s);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Can't listen for phones: {e}")),
        }
    }

    fn save_devices(&mut self) {
        if cfg!(test) {
            return;
        }
        let path = devices_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, self.devices.to_text())
            .and_then(|()| std::fs::rename(&tmp, &path))
            .is_err()
        {
            self.error = Some("The paired phones couldn't be saved.".into());
        }
    }

    /// Shows a fresh code; the old one, if any, stops working.
    fn show_code(&mut self) {
        self.start();
        let Some(server) = &self.server else {
            return;
        };
        let host = match daw_remote::net::local_ip() {
            Some(ip) => ip.to_string(),
            None => {
                self.error =
                    Some("No network: connect this computer to Wi-Fi or a network first.".into());
                return;
            }
        };
        let code = daw_remote::new_code();
        let text = daw_remote::qr_text(&host, server.port(), &code);
        server.set_code(Some(code));
        self.showing = daw_remote::qr_image(&text).map(|img| (text, img));
        self.note = None;
    }

    fn hide_code(&mut self) {
        if let Some(s) = &self.server {
            s.set_code(None);
        }
        self.showing = None;
    }

    /// Remembers the clips of a recording that has just arrived.
    pub(crate) fn note_takes(&mut self, keys: Vec<(String, String)>) {
        if !keys.is_empty() {
            self.last_take = keys;
        }
    }

    /// The window. Returns phones to forget or drop.
    fn window(&mut self, ctx: &egui::Context) {
        let mut open = self.open;
        let mut show = false;
        let mut hide = false;
        let mut forget = None;
        let mut drop_phone = None;
        egui::Window::new("Phone remote")
            .open(&mut open)
            .default_pos([260.0, 120.0])
            .default_size([440.0, 420.0])
            .show(ctx, |ui| {
                ui.label(
                    "Record and run the transport from your phone, on the same Wi-Fi. \
                     Install the app, then scan the code it asks for here.",
                );
                ui.add_space(6.0);
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                if let Some(n) = &self.note {
                    ui.label(RichText::new(n).strong());
                }
                match &self.showing {
                    Some((text, img)) => {
                        qr(ui, img);
                        ui.label("Or type this into the app:");
                        ui.add(egui::Label::new(RichText::new(text).monospace()).selectable(true));
                        ui.label(
                            RichText::new("The code works once, for the next phone that scans it.")
                                .weak(),
                        );
                        if ui.button("Hide code").clicked() {
                            hide = true;
                        }
                    }
                    None => {
                        if ui.button("Show a code to pair a phone").clicked() {
                            show = true;
                        }
                    }
                }
                ui.separator();
                ui.strong("Connected now");
                if self.connected.is_empty() {
                    ui.label(RichText::new("No phone is connected.").weak());
                }
                for (id, name) in &self.connected {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if ui
                            .button("Disconnect")
                            .accessible_name(format!("Disconnect {name}"))
                            .clicked()
                        {
                            drop_phone = Some(*id);
                        }
                    });
                }
                ui.add_space(6.0);
                ui.strong("Paired phones");
                if self.devices.phones.is_empty() {
                    ui.label(RichText::new("None yet.").weak());
                }
                for p in &self.devices.phones {
                    ui.horizontal(|ui| {
                        ui.label(&p.name);
                        if ui
                            .button("Forget")
                            .accessible_name(format!("Forget {}", p.name))
                            .on_hover_text("It will need to scan a code again")
                            .clicked()
                        {
                            forget = Some(p.token.clone());
                        }
                    });
                }
                ui.add_space(6.0);
                match &self.server {
                    Some(s) => {
                        ui.label(
                            RichText::new(format!(
                                "Listening on port {} of this computer's local network address.",
                                s.port()
                            ))
                            .weak(),
                        );
                    }
                    None => {
                        ui.label(RichText::new("Not listening until a phone is paired.").weak());
                    }
                }
            });
        self.open = open;
        if show {
            self.show_code();
        }
        if hide {
            self.hide_code();
        }
        if let Some(token) = forget {
            self.devices.forget(&token);
            if let Some(s) = &self.server {
                s.forget(&token);
            }
            self.save_devices();
        }
        if let (Some(id), Some(s)) = (drop_phone, &self.server) {
            s.disconnect(id);
        }
    }
}

/// Draws a QR image at a readable size with a quiet border.
fn qr(ui: &mut egui::Ui, img: &QrImage) {
    let cell = (260.0 / (img.size + 8) as f32).floor().max(2.0);
    let side = cell * (img.size + 8) as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let p = ui.painter();
    // Black on white whatever the theme, so phone cameras can read it.
    p.rect_filled(rect, 0.0, Color32::WHITE);
    let origin = rect.min + egui::vec2(4.0 * cell, 4.0 * cell);
    for y in 0..img.size {
        for x in 0..img.size {
            if img.dark[y * img.size + x] {
                let min = origin + egui::vec2(x as f32 * cell, y as f32 * cell);
                p.rect_filled(
                    egui::Rect::from_min_size(min, egui::vec2(cell, cell)),
                    0.0,
                    Color32::BLACK,
                );
            }
        }
    }
}

impl App {
    /// Settings → Phone remote…, and everything the phones need each
    /// frame: their requests applied, and the latest layout and status
    /// sent.
    pub(crate) fn remote_tick(&mut self, ctx: &egui::Context) {
        if self.remote.open {
            self.remote.window(ctx);
        }
        let Some(server) = &self.remote.server else {
            return;
        };
        let events = server.events();
        for e in events {
            match e {
                Event::Paired(p) => {
                    self.remote.devices = self.remote.server.as_ref().unwrap().devices();
                    self.remote.save_devices();
                    self.remote.showing = None;
                    self.remote.note = Some(format!("{} is paired.", p.name));
                }
                Event::Connected { id, name } => self.remote.connected.push((id, name)),
                Event::Disconnected { id } => self.remote.connected.retain(|c| c.0 != id),
                Event::Command { cmd, .. } => self.apply_remote(cmd),
            }
        }
        if self.remote.connected.is_empty() {
            self.remote.last_layout = None;
            return;
        }
        let layout = self.remote_layout();
        if self.remote.last_layout.as_ref() != Some(&layout) {
            self.remote.server.as_ref().unwrap().send_layout(&layout);
            self.remote.last_layout = Some(layout);
        }
        let due = self
            .remote
            .last_status
            .is_none_or(|t| t.elapsed() >= STATUS_EVERY);
        if due {
            let status = self.remote_status();
            self.remote.server.as_ref().unwrap().send_status(&status);
            self.remote.last_status = Some(Instant::now());
        }
        // Phones hear the position even while the window is idle.
        ctx.request_repaint_after(STATUS_EVERY);
    }

    /// Does what a phone asked, as the desktop's own control would.
    fn apply_remote(&mut self, cmd: FromPhone) {
        let may_record = self.may_edit_song();
        match cmd {
            FromPhone::Hello { .. } | FromPhone::Ping => {}
            FromPhone::Play => {
                if !self.ctl.status().playing() {
                    self.toggle_play();
                }
            }
            FromPhone::Stop => self.send(Command::Stop),
            FromPhone::ToStart => self.send(Command::Seek(Frames(0))),
            FromPhone::Record { on } => {
                if may_record && self.recording != on {
                    self.recording = on;
                    self.send(Command::SetRecording(on));
                }
            }
            FromPhone::Seek { frames } => self.send(Command::Seek(Frames(frames.max(0)))),
            FromPhone::SeekBar { bar } => {
                let map = self.tempo_map();
                let at = map.frames_at(map.bar_start(bar.max(1)));
                self.send(Command::Seek(at));
            }
            FromPhone::Arm { track, on } => {
                if may_record
                    && let Some(t) = self.tracks.get_mut(track)
                    && t.input.is_some()
                    && t.arm != on
                {
                    t.arm = on;
                    self.rebuild();
                }
            }
            FromPhone::CountIn { on } => self.count_in = on,
            FromPhone::Metronome { on } => {
                if self.metronome.enabled != on {
                    self.metronome.enabled = on;
                    self.send(Command::SetMetronome(self.metronome));
                }
            }
            FromPhone::Loop { on } => {
                if self.looping != on {
                    self.looping = on;
                    self.send_loop();
                }
            }
            FromPhone::UndoLastTake => {
                if may_record {
                    self.undo_last_take();
                }
            }
        }
    }

    /// Takes the clips of the last recording off their tracks, so the
    /// take before plays again. An ordinary edit: Edit → Undo on the
    /// desktop brings the take back.
    pub(crate) fn undo_last_take(&mut self) -> bool {
        let keys = std::mem::take(&mut self.remote.last_take);
        let mut any = false;
        for (track, clip) in keys {
            let Some(t) = self.tracks.iter_mut().find(|t| t.key == track) else {
                continue;
            };
            let before = t.clips.len();
            t.clips.retain(|c| c.key != clip);
            if t.clips.len() != before {
                any = true;
                let newest = timeline::take_count(&t.clips).checked_sub(1);
                timeline::choose_take(&mut t.clips, newest);
            }
        }
        if any {
            self.rebuild();
        }
        any
    }

    /// The project as a phone draws it.
    fn remote_layout(&self) -> ToPhone {
        let map = self.tempo_map();
        let bar_frames =
            (map.frames_at(map.bar_start(2)).0 - map.frames_at(map.bar_start(1)).0) as f64;
        let last = self
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.end())
            .max()
            .unwrap_or(0);
        ToPhone::Layout {
            project: self
                .proj
                .folder
                .as_ref()
                .map_or("Untitled".into(), |f| f.name()),
            sample_rate: self.rate,
            tempo_bpm: self.tempo_bpm,
            beats_per_bar: self.beats_per_bar,
            bar_frames,
            end_frames: last.max((bar_frames * 8.0) as i64),
            count_in: self.count_in,
            metronome: self.metronome.enabled,
            looping: self.looping,
            loop_bars: [self.loop_bars.0, self.loop_bars.1],
            may_record: self.may_edit_song(),
            tracks: self
                .tracks
                .iter()
                .map(|t| daw_remote::Track {
                    name: t.name.clone(),
                    armed: t.arm,
                    can_arm: t.input.is_some(),
                    clips: t
                        .clips
                        .iter()
                        .map(|c| daw_remote::Clip {
                            start: c.start,
                            len: c.len as i64,
                            take: c.take,
                            muted: c.muted,
                        })
                        .collect(),
                })
                .collect(),
            markers: self
                .markers
                .iter()
                .map(|m| daw_remote::Marker {
                    bar: m.bar,
                    name: m.name.clone(),
                })
                .collect(),
        }
    }

    /// Where playback is and the meters, as the desktop shows them.
    fn remote_status(&self) -> ToPhone {
        let map = self.tempo_map();
        let status = self.ctl.status();
        let pos = status.position();
        let bb = map.bar_beat(map.ticks_at(pos));
        ToPhone::Status {
            playing: status.playing(),
            counting_in: status.counting_in(),
            recording: self.recording,
            position: pos.0,
            bar: bb.bar,
            beat: bb.beat,
            seconds: map.seconds(pos),
            levels: self.tracks.iter().map(|t| [t.meter.0, t.meter.1]).collect(),
            master: [self.master_meter.0, self.master_meter.1],
            can_undo_take: !self.remote.last_take.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use daw_engine::stream::StreamSource;

    use super::*;
    use crate::timeline::Clip;
    use crate::{Track, timeline};

    fn harness() -> Harness<'static, App> {
        Harness::builder()
            .with_size([1200.0, 800.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()))
    }

    fn source() -> Arc<StreamSource> {
        timeline::tests::source(48_000)
    }

    #[test]
    fn a_phone_sees_the_project_and_its_requests_are_applied() {
        let mut h = harness();
        let app = h.state_mut();
        let mut t = Track::new("Drums".into());
        t.input = Some(daw_engine::graph::Input::mono(0));
        t.clips
            .push(Clip::new(source(), PathBuf::from("a.wav"), 0, None));
        app.tracks.push(t);
        app.tracks.push(Track::new("Guitar".into()));
        app.markers.push(timeline::Marker {
            id: "m".into(),
            bar: 3,
            name: "Chorus".into(),
        });

        let ToPhone::Layout {
            tracks,
            markers,
            bar_frames,
            may_record,
            ..
        } = app.remote_layout()
        else {
            panic!("not a layout");
        };
        assert_eq!(tracks.len(), 2);
        assert!(tracks[0].can_arm && !tracks[1].can_arm);
        assert_eq!(tracks[0].clips.len(), 1);
        assert_eq!(markers[0].name, "Chorus");
        assert_eq!(bar_frames, 96_000.0, "4 beats at 120 bpm, 48 kHz");
        assert!(may_record);

        app.apply_remote(FromPhone::Arm { track: 0, on: true });
        app.apply_remote(FromPhone::Arm { track: 1, on: true });
        assert!(app.tracks[0].arm, "a track with an input arms");
        assert!(!app.tracks[1].arm, "one without can't");
        app.apply_remote(FromPhone::Record { on: true });
        assert!(app.recording);
        app.apply_remote(FromPhone::Metronome { on: true });
        assert!(app.metronome.enabled);
        app.apply_remote(FromPhone::CountIn { on: true });
        assert!(app.count_in);
        app.apply_remote(FromPhone::Loop { on: true });
        assert!(app.looping);
        app.apply_remote(FromPhone::SeekBar { bar: 3 });
        app.apply_remote(FromPhone::Play);
        // An armed track keeps the meters repainting, so one step.
        h.step();
        let app = h.state();
        let ToPhone::Status {
            recording, levels, ..
        } = app.remote_status()
        else {
            panic!("not a status");
        };
        assert!(recording);
        assert_eq!(levels.len(), 2);
    }

    #[test]
    fn undo_last_take_takes_the_newest_take_off_and_leaves_the_rest() {
        let mut h = harness();
        let app = h.state_mut();
        let mut t = Track::new("Drums".into());
        t.clips
            .push(Clip::new(source(), PathBuf::from("file.wav"), 0, None));
        timeline::add_take(
            &mut t.clips,
            Clip::new(source(), PathBuf::from("take1.wav"), 0, None),
        );
        timeline::add_take(
            &mut t.clips,
            Clip::new(source(), PathBuf::from("take2.wav"), 0, None),
        );
        let track_key = t.key.clone();
        let take2 = t.clips[2].key.clone();
        app.tracks.push(t);
        assert!(app.tracks[0].clips[1].muted, "take 1 is under take 2");

        assert!(!app.undo_last_take(), "nothing recorded yet");
        app.remote.note_takes(vec![(track_key, take2)]);
        assert!(app.remote_status_can_undo());
        assert!(app.undo_last_take());
        let clips = &app.tracks[0].clips;
        assert_eq!(clips.len(), 2);
        assert!(!clips[1].muted, "take 1 plays again");
        assert!(clips[0].take.is_none(), "the file is untouched");
        assert!(!app.undo_last_take(), "only once");
    }

    #[test]
    fn the_window_offers_a_code_and_lists_phones() {
        let mut h = harness();
        h.state_mut().remote.open = true;
        h.run();
        h.get_by_label("Show a code to pair a phone");
        let app = h.state_mut();
        app.remote.devices.phones.push(daw_remote::Paired {
            name: "Andy's phone".into(),
            token: "T".into(),
            since_ms: 0,
        });
        h.run();
        h.get_by_label("Forget Andy's phone").click();
        h.run();
        assert!(h.state().remote.devices.phones.is_empty());
    }

    impl App {
        fn remote_status_can_undo(&self) -> bool {
            matches!(
                self.remote_status(),
                ToPhone::Status {
                    can_undo_take: true,
                    ..
                }
            )
        }
    }
}
