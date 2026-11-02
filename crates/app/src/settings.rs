// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings, as one window with its sections listed down the side (UI
//! polish plan, step one): audio, MIDI, latency, plugins, drum kits, key
//! commands, theme, AI assistants, the phone remote and the performance
//! check. Each used to be a window of its own.
//!
//! Each section keeps the flag it had as a window (`show_devices`,
//! `latency.open`, `themes.open`...): the window shows the section whose
//! flag is set, so the commands, buttons and tests that set one open
//! Settings there. When another is set, the window moves to it and the
//! one before is cleared, so only one is ever set. File → Settings…
//! (`Cmd::Settings`) opens the section last shown.
//!
//! The guided audio setup stays a window of its own, as a step-by-step
//! wizard opened from the Audio section.

use eframe::egui;

use crate::App;
use crate::commands::Cmd;

/// A section of the Settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Audio,
    Midi,
    Latency,
    Plugins,
    DrumKits,
    KeyCommands,
    Theme,
    Assistants,
    PhoneRemote,
    Performance,
}

impl Section {
    /// In the order they are listed.
    pub(crate) const ALL: [Section; 10] = [
        Section::Audio,
        Section::Midi,
        Section::Latency,
        Section::Plugins,
        Section::DrumKits,
        Section::KeyCommands,
        Section::Theme,
        Section::Assistants,
        Section::PhoneRemote,
        Section::Performance,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Section::Audio => "Audio",
            Section::Midi => "MIDI",
            Section::Latency => "Latency",
            Section::Plugins => "Plugins",
            Section::DrumKits => "Drum kits",
            Section::KeyCommands => "Key commands",
            Section::Theme => "Theme",
            Section::Assistants => "AI assistants",
            Section::PhoneRemote => "Phone remote",
            Section::Performance => "Performance",
        }
    }

    /// Its name for assistants, in `get_selection`.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Section::Audio => "audio",
            Section::Midi => "midi",
            Section::Latency => "latency",
            Section::Plugins => "plugins",
            Section::DrumKits => "drum-kits",
            Section::KeyCommands => "key-commands",
            Section::Theme => "theme",
            Section::Assistants => "assistants",
            Section::PhoneRemote => "phone-remote",
            Section::Performance => "performance",
        }
    }

    /// The command that opens it.
    pub(crate) fn cmd(self) -> Cmd {
        match self {
            Section::Audio => Cmd::AudioDevice,
            Section::Midi => Cmd::MidiDevices,
            Section::Latency => Cmd::MeasureLatency,
            Section::Plugins => Cmd::Plugins,
            Section::DrumKits => Cmd::DrumKits,
            Section::KeyCommands => Cmd::KeyCommands,
            Section::Theme => Cmd::Theme,
            Section::Assistants => Cmd::Assistants,
            Section::PhoneRemote => Cmd::PhoneRemote,
            Section::Performance => Cmd::PerformanceCheck,
        }
    }
}

#[derive(Default)]
pub(crate) struct Settings {
    /// The MIDI section is showing: the one section that was no window
    /// of its own (it was the bottom of the audio device window).
    pub(crate) midi: bool,
    /// The section showing, or last shown.
    pub(crate) last: Option<Section>,
}

impl App {
    /// The flag that shows `s`.
    fn section_flag(&mut self, s: Section) -> &mut bool {
        match s {
            Section::Audio => &mut self.show_devices,
            Section::Midi => &mut self.settings.midi,
            Section::Latency => &mut self.latency.open,
            Section::Plugins => &mut self.effects.show_manager,
            Section::DrumKits => &mut self.kits.open,
            Section::KeyCommands => &mut self.commands.editor.open,
            Section::Theme => &mut self.themes.open,
            Section::Assistants => &mut self.assistants.open,
            Section::PhoneRemote => &mut self.remote.open,
            Section::Performance => &mut self.perf.open,
        }
    }

    /// The section Settings is showing, if it is open.
    pub(crate) fn settings_section(&self) -> Option<Section> {
        let on = |s: Section| match s {
            Section::Audio => self.show_devices,
            Section::Midi => self.settings.midi,
            Section::Latency => self.latency.open,
            Section::Plugins => self.effects.show_manager,
            Section::DrumKits => self.kits.open,
            Section::KeyCommands => self.commands.editor.open,
            Section::Theme => self.themes.open,
            Section::Assistants => self.assistants.open,
            Section::PhoneRemote => self.remote.open,
            Section::Performance => self.perf.open,
        };
        // A section just asked for wins over the one showing.
        (Section::ALL.into_iter())
            .filter(|&s| on(s))
            .find(|&s| Some(s) != self.settings.last)
            .or_else(|| self.settings.last.filter(|&s| on(s)))
    }

    /// Opens Settings on `s`.
    pub(crate) fn open_settings(&mut self, s: Section) {
        *self.section_flag(s) = true;
    }

    /// File → Settings…: the section last shown, or Audio.
    pub(crate) fn open_last_settings(&mut self) {
        let s = self.settings.last.unwrap_or(Section::Audio);
        self.open_settings(s);
    }

    /// Leaves section `s`: what it was doing stops, as when its window
    /// was closed.
    fn leave_section(&mut self, s: Section) {
        *self.section_flag(s) = false;
        match s {
            Section::Latency if self.latency.busy() => {
                self.latency.cancel();
                self.restart_audio();
            }
            Section::DrumKits => self.kits.learn = None,
            Section::KeyCommands => {
                self.commands.editor.capturing = None;
                self.commands.editor.clash = None;
            }
            _ => {}
        }
    }

    /// The Settings window, while a section is open.
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(shown) = self.settings_section() else {
            return;
        };
        for s in Section::ALL {
            if s != shown && *self.section_flag(s) {
                self.leave_section(s);
            }
        }
        self.settings.last = Some(shown);
        let mut open = true;
        let mut pick = None;
        let screen = ctx.content_rect();
        let size = egui::vec2(
            (screen.width() - 80.0).clamp(400.0, 980.0),
            (screen.height() - 120.0).clamp(300.0, 680.0),
        );
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .default_size(size)
            .default_pos(screen.center() - size / 2.0)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(150.0);
                        for s in Section::ALL {
                            // ai: view
                            let r = ui
                                .add(
                                    egui::Button::selectable(s == shown, s.title())
                                        .min_size(egui::vec2(150.0, 0.0)),
                                )
                                .on_hover_text(self.commands.hover(s.cmd()));
                            if r.clicked() {
                                pick = Some(s);
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.heading(shown.title());
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .id_salt(("settings", shown.id()))
                            .auto_shrink(false)
                            .show(ui, |ui| self.settings_page(ui, shown));
                    });
                });
            });
        if !open {
            self.leave_section(shown);
        } else if let Some(s) = pick.filter(|&s| s != shown) {
            self.leave_section(shown);
            self.open_settings(s);
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui, s: Section) {
        match s {
            Section::Audio => self.device_page(ui),
            Section::Midi => self.midi_controls(ui),
            Section::Latency => self.latency_page(ui),
            Section::Plugins => self.effects.manager_page(ui),
            Section::DrumKits => self.kits_page(ui),
            Section::KeyCommands => {
                let simple = self.simple.on;
                self.commands.editor_page(ui, simple);
            }
            Section::Theme => self.themes.page(ui),
            Section::Assistants => self.assistants_page(ui),
            Section::PhoneRemote => self.remote.page(ui),
            Section::Performance => self.perf_page(ui),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_section_has_its_own_command_and_name() {
        for (i, a) in Section::ALL.iter().enumerate() {
            for b in &Section::ALL[i + 1..] {
                assert_ne!(a.cmd(), b.cmd());
                assert_ne!(a.id(), b.id());
            }
            // Key commands and Theme are in the Help and View menus too.
            let group = a.cmd().info().group;
            assert!(
                group == crate::menus::SETTINGS
                    || matches!(a, Section::KeyCommands | Section::Theme),
                "{a:?} is in {group}"
            );
        }
    }

    /// Through the assistant interface: the commands open Settings on
    /// their sections, one at a time, and `get_selection` says which;
    /// `list_commands` gives each command's new menu.
    #[test]
    fn assistants_open_each_section_and_see_which_is_showing() {
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        let section = |d: &mut crate::agent::driver::Driver| {
            d.call("get_selection", json!({})).unwrap()["settings"].clone()
        };
        assert_eq!(section(&mut d), json!(null), "closed to begin with");
        d.command("settings");
        assert_eq!(section(&mut d), "audio", "Settings opens on Audio");
        for s in Section::ALL {
            d.command(s.cmd().info().id());
            assert_eq!(section(&mut d), s.id(), "{s:?}");
            let on: Vec<Section> = (Section::ALL.into_iter())
                .filter(|&x| *d.app().section_flag(x))
                .collect();
            assert_eq!(on, [s], "one section at a time");
        }
        // Closed, File → Settings… opens the section last shown.
        let last = Section::ALL[Section::ALL.len() - 1];
        d.app().leave_section(last);
        assert_eq!(section(&mut d), json!(null));
        d.command("settings");
        assert_eq!(section(&mut d), last.id());

        let list = d.call("list_commands", json!({})).unwrap();
        let menu = |id: &str| {
            (list.as_array().unwrap().iter())
                .find(|c| c["id"] == id)
                .map(|c| c["menu"].clone())
                .unwrap()
        };
        assert_eq!(menu("fix-timing"), "Clip");
        assert_eq!(menu("add-bus"), "Track");
        assert_eq!(menu("play"), "Song");
        assert_eq!(menu("settings"), "File");
        assert_eq!(menu("plugins"), crate::menus::SETTINGS);
    }
}
