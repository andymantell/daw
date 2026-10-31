// SPDX-License-Identifier: GPL-3.0-or-later
//! The simpler mode for beginners (Phase 4 D1): one switch, View →
//! Simpler mode, that shows only the everyday commands and controls.
//! Each command in the table (`commands.rs`) is marked everyday or not,
//! and the menus, keys and command search all ask `App::offered`, as the
//! agent interface does when it lists the commands. What is left out
//! stays one click away: each menu ends in a More submenu, and the
//! search has a box to show the rest. Off, as it starts, nothing changes.
//!
//! Panes leave out their own advanced controls with a one-line check of
//! `App::simple.on` (the timeline's of `Timeline::simple`).
//!
//! The choice is kept per computer in `simple.txt`: one `on yes|no` line.

use std::path::PathBuf;

use eframe::egui;

use crate::App;
use crate::commands::{COMMANDS, Cmd};

pub(crate) struct Simple {
    pub(crate) on: bool,
    /// Where the choice is kept; none in tests.
    path: Option<PathBuf>,
    /// Left out of the menu being drawn, for its More submenu.
    held: Vec<Cmd>,
    /// Something has been drawn in the menu since it opened.
    drawn: bool,
    /// A separator is owed before the next entry.
    gap: bool,
}

pub(crate) fn from_text(text: &str) -> bool {
    text.lines()
        .any(|l| l.split_once(' ') == Some(("on", "yes")))
}

pub(crate) fn to_text(on: bool) -> String {
    format!("on {}\n", if on { "yes" } else { "no" })
}

impl Simple {
    pub(crate) fn new(path: Option<PathBuf>) -> Simple {
        let on = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|t| from_text(&t));
        Simple {
            on,
            path,
            held: Vec::new(),
            drawn: false,
            gap: false,
        }
    }

    /// Turns it on or off, and keeps the choice.
    pub(crate) fn set(&mut self, on: bool) -> std::io::Result<()> {
        self.on = on;
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, to_text(on))
    }
}

/// Whether a menu has anything to show in the simpler mode. One with
/// nothing everyday in it is left off the menu bar.
fn has_everyday(menu: &str) -> bool {
    COMMANDS.iter().any(|i| i.group == menu && i.everyday)
}

impl App {
    /// Whether the menus, keys, command search and agent interface offer
    /// `cmd` now. Every command still runs when asked for by name.
    pub(crate) fn offered(&self, cmd: Cmd) -> bool {
        !self.simple.on || cmd.info().everyday
    }

    pub(crate) fn toggle_simple(&mut self) {
        let on = !self.simple.on;
        if let Err(e) = self.simple.set(on) {
            self.message = Some(format!("Couldn't keep the simpler mode: {e}"));
        }
    }

    /// A menu on the menu bar. In the simpler mode, the commands it
    /// leaves out go in a More submenu at the end.
    pub(crate) fn top_menu(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        add: impl FnOnce(&mut App, &mut egui::Ui),
    ) {
        if self.simple.on && !has_everyday(title) {
            return;
        }
        ui.menu_button(title, |ui| {
            self.simple.held.clear();
            self.simple.drawn = false;
            self.simple.gap = false;
            add(self, ui);
            self.menu_more(ui);
        });
    }

    /// A separator, drawn only between entries that are shown.
    pub(crate) fn menu_gap(&mut self) {
        self.simple.gap = true;
    }

    /// Before an entry in a menu: the separator owed, if any.
    pub(crate) fn menu_mark(&mut self, ui: &mut egui::Ui) {
        if self.simple.gap && self.simple.drawn {
            ui.separator();
        }
        self.simple.gap = false;
        self.simple.drawn = true;
    }

    /// Leaves `cmd` out of the menu being drawn if the simpler mode
    /// hides it, for its More submenu. True when it is left out.
    pub(crate) fn menu_hold(&mut self, cmd: Cmd) -> bool {
        if self.offered(cmd) {
            return false;
        }
        self.simple.held.push(cmd);
        true
    }

    fn menu_more(&mut self, ui: &mut egui::Ui) {
        let held = std::mem::take(&mut self.simple.held);
        if held.is_empty() {
            return;
        }
        if self.simple.drawn {
            ui.separator();
        }
        ui.menu_button("More", |ui| {
            for cmd in held {
                self.menu_entry(ui, cmd);
            }
        })
        .response
        .on_hover_text("What the simpler mode leaves out");
    }

    /// On the menu bar while the simpler mode is on, to turn it off.
    pub(crate) fn simple_badge(&mut self, ui: &mut egui::Ui) {
        if !self.simple.on {
            return;
        }
        let r = ui
            .add(egui::Button::selectable(true, "Simpler mode"))
            .on_hover_text("Showing the everyday commands. Click to show everything");
        if r.clicked() {
            self.toggle_simple();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_is_written_and_read_back() {
        assert!(from_text(&to_text(true)));
        assert!(!from_text(&to_text(false)));
        assert!(!from_text(""), "off until turned on");
    }

    #[test]
    fn the_switch_and_the_ways_back_are_everyday() {
        for cmd in [Cmd::SimpleMode, Cmd::FindCommand, Cmd::PlayStop, Cmd::Save] {
            assert!(cmd.info().everyday, "{cmd:?}");
        }
    }

    #[test]
    fn most_commands_are_advanced_and_most_menus_stay() {
        let everyday = COMMANDS.iter().filter(|i| i.everyday).count();
        assert!(
            everyday * 2 < COMMANDS.len(),
            "{everyday} of {}",
            COMMANDS.len()
        );
        for menu in [
            "File",
            "Edit",
            "Notation",
            "Transport",
            "View",
            "Settings",
            "Practice",
            "Help",
        ] {
            assert!(has_everyday(menu), "{menu}");
        }
        assert!(!has_everyday("Notes"));
    }
}

/// Through the accessibility tree, as in `a11y`.
#[cfg(test)]
mod ui_tests {
    use eframe::egui;
    use eframe::egui::{Key, Modifiers};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT as _, Queryable as _};

    use crate::commands::Cmd;
    use crate::workspace::Pane;
    use crate::{App, Track};

    fn harness(simple: bool) -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        let app = h.state_mut();
        app.tracks.push(Track::new("Guitar".into()));
        app.rebuild();
        app.simple.on = simple;
        h.run();
        h
    }

    fn shown(h: &Harness<'_, App>, label: &str) -> bool {
        h.query_all_by_label(label).next().is_some()
    }

    /// A menu entry, whose name is followed by its keys.
    fn entry(h: &Harness<'_, App>, name: &str) -> bool {
        h.query_all_by_label_contains(name).next().is_some()
    }

    /// The menu bar's menus.
    fn menus(h: &Harness<'_, App>) -> Vec<String> {
        h.query_all_by_role(egui::accesskit::Role::Button)
            .filter_map(|n| n.accesskit_node().label())
            .filter(|l| {
                [
                    "File",
                    "Edit",
                    "Notes",
                    "Notation",
                    "Transport",
                    "View",
                    "Settings",
                    "Practice",
                    "Help",
                ]
                .contains(&l.as_str())
            })
            .collect()
    }

    #[test]
    fn off_every_menu_and_command_is_there() {
        let mut h = harness(false);
        assert_eq!(menus(&h).len(), 9);
        h.get_by_label("Edit").click();
        h.run();
        assert!(entry(&h, "Fix timing…"));
        assert!(!shown(&h, "More"));
    }

    #[test]
    fn on_advanced_commands_wait_under_more() {
        let mut h = harness(true);
        assert!(
            !menus(&h).contains(&"Notes".to_string()),
            "a menu with nothing everyday goes"
        );
        assert_eq!(menus(&h).len(), 8);
        assert!(shown(&h, "Simpler mode"), "a way back on the menu bar");
        h.get_by_label("Edit").click();
        h.run();
        assert!(entry(&h, "Undo"));
        assert!(!entry(&h, "Edit groups…"));
        h.get_by_label("More").click();
        h.run();
        h.get_by_label_contains("Edit groups…").click();
        h.run();
        assert!(h.state().groups_ui.open, "one click away, and it runs");
    }

    #[test]
    fn on_advanced_keys_say_why_they_do_nothing() {
        let mut h = harness(true);
        h.key_press_modifiers(Modifiers::COMMAND, Key::B);
        h.run();
        let app = h.state();
        assert!(!app.workspace.layout.is_open(Pane::Browser));
        assert!(app.message.as_ref().unwrap().contains("Content browser"));

        // Everyday keys still work.
        h.key_press(Key::K);
        h.run();
        assert!(h.state().metronome.enabled);

        // Off, the key opens it.
        h.state_mut().simple.on = false;
        h.key_press_modifiers(Modifiers::COMMAND, Key::B);
        h.run();
        h.run();
        assert!(h.state().workspace.layout.is_open(Pane::Browser));
    }

    #[test]
    fn on_the_search_shows_advanced_commands_when_asked() {
        let mut h = harness(true);
        h.key_press_modifiers(Modifiers::COMMAND, Key::K);
        h.run();
        h.get_by_label("Type a command's name")
            .type_text("edit groups");
        h.run();
        assert!(!shown(&h, "Edit groups…"));
        h.get_by_label_contains("Show advanced commands too")
            .click();
        h.run();
        h.get_by_label_contains("Edit groups…").click();
        h.run();
        assert!(h.state().groups_ui.open);
    }

    #[test]
    fn on_the_mixer_leaves_out_routing() {
        for simple in [false, true] {
            let mut h = harness(simple);
            h.state_mut().workspace.open(Pane::Mixer);
            h.run();
            h.run();
            assert!(shown(&h, "Guitar volume"));
            for label in ["Sends", "+ Add bus"] {
                assert_eq!(shown(&h, label), !simple, "{label}");
            }
        }
    }

    #[test]
    fn the_switch_is_a_command() {
        let mut h = harness(false);
        let ctx = h.ctx.clone();
        h.state_mut().run(&ctx, Cmd::SimpleMode);
        assert!(h.state().simple.on);
        h.state_mut().run(&ctx, Cmd::SimpleMode);
        assert!(!h.state().simple.on);
    }
}
