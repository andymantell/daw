// SPDX-License-Identifier: GPL-3.0-or-later
//! Electronic drum kits (roadmap 3.4): the profiles that say which note
//! each pad sends, the one used for each MIDI input, and the wizard that
//! learns a kit by asking for each pad in turn.
//!
//! Learned profiles are kept as files in `kits/` beside the device
//! settings, and the profile chosen for each input in `kits/in-use.txt`.
//! An input nobody chose a profile for gets the first one that knows its
//! name (learned before built-in), else General MIDI; the first time
//! that happens the app says so.
//!
//! Every step of the wizard is a command ([`Cmd::KitNext`] and the rest),
//! so it can be driven without the window.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use daw_engine::midi::{Captured, Kind};
use daw_project::kit::{
    Choke, Factory, GENERAL_MIDI, KIT_EXTENSION, KitProfile, Pedal, SLOTS, Slot, built_in_kits,
    for_port,
};
use eframe::egui::{self, RichText};

use crate::App;
use crate::commands::Cmd;

/// A step of the wizard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Step {
    /// Hit a pad or zone: the note it sends is kept.
    Hit(&'static Slot),
    /// Hold the hi-hat pedal down: the controller and its value are kept.
    PedalDown,
    /// Let it up.
    PedalUp,
    /// Grab a cymbal to stop it.
    Choke,
    /// Name the kit and save.
    Save,
}

/// Every step, in order: the pads up to the hi-hat pedal's note, the
/// pedal's range, the cymbals, then the choke.
pub(crate) fn steps() -> Vec<Step> {
    let mut v = Vec::new();
    for s in SLOTS {
        v.push(Step::Hit(s));
        if s.id == "hihat-pedal" {
            v.extend([Step::PedalDown, Step::PedalUp]);
        }
    }
    v.extend([Step::Choke, Step::Save]);
    v
}

/// A kit being learned.
pub(crate) struct Learn {
    steps: Vec<Step>,
    pub(crate) step: usize,
    pub(crate) profile: KitProfile,
    /// The name being typed at the last step.
    pub(crate) name: String,
    /// Channels heard so far: one means the kit sends on that channel.
    channels: BTreeSet<u8>,
    /// What came in during this step, to show.
    heard: Option<String>,
    /// The controller that moved most recently in a pedal step, and its
    /// latest value.
    controller: Option<(u8, u8)>,
    closed: Option<(u8, u8)>,
    /// Pads hit that are already taken by an earlier slot.
    pub(crate) warning: Option<String>,
}

impl Learn {
    fn new() -> Learn {
        Learn {
            steps: steps(),
            step: 0,
            profile: KitProfile::new(""),
            name: String::new(),
            channels: BTreeSet::new(),
            heard: None,
            controller: None,
            closed: None,
            warning: None,
        }
    }

    pub(crate) fn current(&self) -> Step {
        self.steps[self.step]
    }

    fn advance(&mut self) {
        self.heard = None;
        self.controller = None;
        self.step = (self.step + 1).min(self.steps.len() - 1);
    }

    /// Takes in one event from input port `port` (named `port_name`).
    /// True if it moved the wizard on.
    pub(crate) fn hear(&mut self, port_name: Option<&str>, c: &Captured) -> bool {
        let e = c.event;
        match (self.current(), e.kind) {
            (
                Step::Hit(slot),
                Kind::NoteOn {
                    pitch, velocity, ..
                },
            ) if velocity > 0 => {
                if let Some(p) = port_name
                    && !self.profile.ports.iter().any(|x| x == p)
                {
                    self.profile.ports.push(p.to_string());
                }
                self.channels.insert(e.channel);
                self.warning = (self.profile.slots_for(pitch).next())
                    .filter(|s| s.gm != slot.gm && !(is_hihat(s) && is_hihat(slot)))
                    .map(|s| {
                        format!(
                            "{} sent note {pitch}, the same as {}. The module may play one \
                             sound for both: give one another note in its setup, or skip it.",
                            slot.name, s.name
                        )
                    });
                self.profile.set(slot.id, Some(pitch));
                self.advance();
                true
            }
            (Step::PedalDown | Step::PedalUp, Kind::Control { number, value }) => {
                if let Some((n, _)) = self.closed
                    && n != number
                    && self.current() == Step::PedalUp
                {
                    // Only the controller that moved when it went down.
                    return false;
                }
                self.controller = Some((number, value));
                self.heard = Some(format!("Controller {number}: {value}"));
                false
            }
            (Step::Choke, Kind::KeyPressure { .. }) => {
                self.profile.choke = Some(Choke::KeyPressure);
                self.advance();
                true
            }
            (Step::Choke, Kind::Control { number, .. })
                if self.profile.pedal.is_none_or(|p| p.controller != number) =>
            {
                self.profile.choke = Some(Choke::Control(number));
                self.advance();
                true
            }
            _ => false,
        }
    }

    /// Goes on from a step that waits for Next: the pedal steps keep
    /// what was heard. A pad step with nothing heard is skipped. At the
    /// last step, gives the finished profile.
    pub(crate) fn next(&mut self) -> Option<KitProfile> {
        match self.current() {
            Step::PedalDown => {
                self.closed = self.controller;
                if self.closed.is_none() {
                    // No controller: the pedal sends notes only.
                    self.advance();
                }
                self.advance();
            }
            Step::PedalUp => {
                if let (Some((controller, closed)), Some((_, open))) =
                    (self.closed, self.controller)
                    && open != closed
                {
                    self.profile.pedal = Some(Pedal {
                        controller,
                        closed,
                        open,
                    });
                }
                self.advance();
            }
            Step::Save => return self.finish(),
            Step::Hit(_) | Step::Choke => self.skip(),
        }
        None
    }

    /// Goes on without learning this step: the pad isn't on this kit.
    pub(crate) fn skip(&mut self) {
        match self.current() {
            Step::Hit(slot) => self.profile.set(slot.id, None),
            Step::PedalDown => {
                self.closed = None;
                self.profile.pedal = None;
                self.advance();
            }
            Step::PedalUp => self.profile.pedal = None,
            Step::Choke => self.profile.choke = None,
            Step::Save => return,
        }
        self.warning = None;
        self.advance();
    }

    /// Goes back a step, to learn it again.
    pub(crate) fn back(&mut self) {
        self.heard = None;
        self.controller = None;
        self.warning = None;
        self.step = self.step.saturating_sub(1);
        if self.current() == Step::PedalUp && self.closed.is_none() {
            // Skipped together with the step before it.
            self.step -= 1;
        }
    }

    fn finish(&mut self) -> Option<KitProfile> {
        let name = self.name.trim();
        if name.is_empty() {
            self.warning = Some("Give the kit a name first.".into());
            return None;
        }
        let mut k = self.profile.clone();
        k.name = name.to_string();
        k.channel = (self.channels.len() == 1)
            .then(|| self.channels.first().copied())
            .flatten();
        Some(k)
    }

    /// The default name: the input it was played on.
    fn suggest_name(&mut self) {
        if self.current() == Step::Save && self.name.is_empty() {
            self.name = self.profile.ports.first().cloned().unwrap_or_default();
        }
    }
}

fn is_hihat(s: &Slot) -> bool {
    s.id.starts_with("hihat")
}

/// What the app knows about drum kits.
pub(crate) struct Kits {
    pub(crate) open: bool,
    pub(crate) learn: Option<Learn>,
    built_in: Vec<KitProfile>,
    /// Profiles learned or imported on this computer, by name.
    pub(crate) learned: Vec<KitProfile>,
    /// Input port name → the profile chosen for it, by name.
    chosen: BTreeMap<String, String>,
    /// Inputs already told which profile they got.
    told: BTreeSet<String>,
    /// Where learned profiles are kept; `None` in tests.
    dir: Option<PathBuf>,
    /// The profile picked in the window, for exporting.
    pub(crate) selected: Option<String>,
}

const IN_USE: &str = "in-use.txt";

impl Kits {
    pub(crate) fn new(dir: Option<PathBuf>) -> Kits {
        let mut k = Kits {
            open: false,
            learn: None,
            built_in: built_in_kits(),
            learned: Vec::new(),
            chosen: BTreeMap::new(),
            told: BTreeSet::new(),
            dir,
            selected: None,
        };
        if let Some(d) = &k.dir {
            if let Ok(entries) = std::fs::read_dir(d) {
                let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
                paths.sort();
                for p in paths.iter().filter(|p| is_kit_file(p)) {
                    if let Ok(t) = std::fs::read_to_string(p)
                        && let Ok(kit) = KitProfile::from_text(&t)
                    {
                        k.learned.push(kit);
                    }
                }
            }
            if let Ok(t) = std::fs::read_to_string(d.join(IN_USE)) {
                k.chosen = (t.lines())
                    .filter_map(|l| l.split_once('\t'))
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect();
            }
        }
        k
    }

    /// Every profile: learned first, then built in.
    pub(crate) fn all(&self) -> impl Iterator<Item = &KitProfile> {
        self.learned.iter().chain(&self.built_in)
    }

    fn by_name(&self, name: &str) -> Option<&KitProfile> {
        self.all().find(|k| k.name == name)
    }

    /// The profile recognised by a port's name, if any.
    fn recognised(&self, port: &str) -> Option<&KitProfile> {
        for_port(&self.learned, port).or_else(|| for_port(&self.built_in, port))
    }

    /// The profile used for an input port.
    pub(crate) fn for_input(&self, port: &str) -> &KitProfile {
        (self.chosen.get(port))
            .and_then(|n| self.by_name(n))
            .or_else(|| self.recognised(port))
            .unwrap_or(&self.built_in[0])
    }

    /// Chooses the profile for an input port.
    pub(crate) fn choose(&mut self, port: &str, name: &str) {
        self.chosen.insert(port.to_string(), name.to_string());
        self.save_chosen();
    }

    /// For inputs seen for the first time that a profile recognises:
    /// what to tell the user.
    pub(crate) fn notice(&mut self, inputs: &[String]) -> Option<String> {
        let mut said = None;
        for p in inputs {
            if self.told.contains(p) {
                continue;
            }
            self.told.insert(p.clone());
            if self.chosen.contains_key(p) {
                continue;
            }
            if let Some(k) = self.recognised(p) {
                said = Some(format!(
                    "{p} looks like the {} drum kit: its pad map is used for it. \
                     Change this in Settings → Drum kits.",
                    k.name
                ));
            }
        }
        said
    }

    /// Takes in what came from the MIDI inputs while the wizard is open.
    pub(crate) fn hear(&mut self, captured: &[Captured], inputs: &[String]) {
        let Some(l) = &mut self.learn else {
            return;
        };
        for c in captured {
            l.hear(inputs.get(c.port).map(String::as_str), c);
        }
        l.suggest_name();
    }

    /// Keeps a profile, replacing a learned one of the same name, and
    /// uses it for the inputs it was learned on.
    pub(crate) fn keep(&mut self, kit: KitProfile) -> Result<(), String> {
        if let Some(d) = &self.dir {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            std::fs::write(d.join(file_name(&kit.name)), kit.to_text())
                .map_err(|e| format!("Could not save the kit: {e}"))?;
        }
        for p in &kit.ports {
            self.chosen.insert(p.clone(), kit.name.clone());
        }
        self.save_chosen();
        self.selected = Some(kit.name.clone());
        self.learned.retain(|k| k.name != kit.name);
        self.learned.push(kit);
        self.learned.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    fn save_chosen(&self) {
        let Some(d) = &self.dir else {
            return;
        };
        let t: String = (self.chosen.iter())
            .map(|(p, k)| format!("{p}\t{k}\n"))
            .collect();
        let _ = std::fs::create_dir_all(d);
        let _ = std::fs::write(d.join(IN_USE), t);
    }

    /// The profile to export: the one picked in the window, else the
    /// latest learned.
    pub(crate) fn to_export(&self) -> Option<&KitProfile> {
        (self.selected.as_deref())
            .and_then(|n| self.by_name(n))
            .or_else(|| self.learned.last())
    }

    pub(crate) fn learning(&self) -> bool {
        self.learn.is_some()
    }

    /// The window: the profile for each input, the profiles, and the
    /// wizard. Gives the command a button asked for.
    pub(crate) fn window(&mut self, ctx: &egui::Context, inputs: &[String]) -> Option<Cmd> {
        if !self.open {
            return None;
        }
        let mut cmd = None;
        let mut open = self.open;
        egui::Window::new("Drum kits")
            .open(&mut open)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                if self.learn.is_some() {
                    cmd = self.learn_ui(ui);
                } else {
                    cmd = self.list_ui(ui, inputs);
                }
            });
        if !open {
            self.open = false;
            self.learn = None;
        }
        cmd
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, inputs: &[String]) -> Option<Cmd> {
        let mut cmd = None;
        ui.label(
            "Which note each pad of an electronic kit sends. A kit not listed can be \
             learned: you hit each pad when asked.",
        );
        ui.add_space(4.0);
        if inputs.is_empty() {
            ui.label(RichText::new("No MIDI inputs are connected.").weak());
        } else {
            egui::Grid::new("kit-inputs").num_columns(2).show(ui, |ui| {
                let names: Vec<String> = self.all().map(|k| k.name.clone()).collect();
                for p in inputs {
                    let label = ui.label(p);
                    let now = self.for_input(p).name.clone();
                    let mut pick = now.clone();
                    egui::ComboBox::from_id_salt(("kit-for", p))
                        .selected_text(&pick)
                        .show_ui(ui, |ui| {
                            for n in &names {
                                ui.selectable_value(&mut pick, n.clone(), n);
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    if pick != now {
                        self.choose(p, &pick);
                    }
                    ui.end_row();
                }
            });
        }
        ui.separator();
        ui.label("Kits:");
        let names: Vec<(String, bool)> = (self.learned.iter().map(|k| (k.name.clone(), true)))
            .chain(self.built_in.iter().map(|k| (k.name.clone(), false)))
            .collect();
        for (n, learned) in names {
            let picked = self.selected.as_deref() == Some(n.as_str());
            let label = if learned {
                n.clone()
            } else {
                format!("{n} (built in)")
            };
            if ui.selectable_label(picked, label).clicked() {
                self.selected = Some(n);
            }
        }
        ui.horizontal(|ui| {
            for c in [Cmd::LearnKit, Cmd::ImportKit, Cmd::ExportKit] {
                if ui.button(c.info().name).clicked() {
                    cmd = Some(c);
                }
            }
        });
        cmd
    }

    fn learn_ui(&mut self, ui: &mut egui::Ui) -> Option<Cmd> {
        let mut cmd = None;
        let l = self.learn.as_mut()?;
        let total = l.steps.len();
        ui.label(RichText::new(format!("Step {} of {total}", l.step + 1)).weak());
        let waits = match l.current() {
            Step::Hit(slot) => {
                ui.heading(slot.name);
                ui.label(slot.prompt);
                ui.label(RichText::new("Waiting for a hit…").weak());
                false
            }
            Step::PedalDown => {
                ui.heading("Hi-hat pedal: down");
                ui.label(
                    "Press the hi-hat pedal all the way down, rock it a little, and keep it \
                     there. Then press Next.",
                );
                true
            }
            Step::PedalUp => {
                ui.heading("Hi-hat pedal: up");
                ui.label("Now let the pedal all the way up, then press Next.");
                true
            }
            Step::Choke => {
                ui.heading("Choke");
                ui.label(
                    "Hit a cymbal, then grab its edge to stop it. Skip this if your \
                     cymbals can't be choked.",
                );
                ui.label(RichText::new("Waiting…").weak());
                false
            }
            Step::Save => {
                let h = ui.heading("Name the kit");
                ui.text_edit_singleline(&mut l.name).labelled_by(h.id);
                ui.label("Are these the notes the module came with?");
                for f in Factory::ALL {
                    ui.radio_value(&mut l.profile.factory, f, f.label());
                }
                let learned = l.profile.notes.len();
                ui.label(format!("{learned} pads and zones learned."));
                if let Some(p) = l.profile.pedal {
                    ui.label(format!(
                        "Hi-hat pedal: controller {}, {} shut to {} open.",
                        p.controller, p.closed, p.open
                    ));
                }
                true
            }
        };
        if let Some(h) = &l.heard {
            ui.label(RichText::new(h).strong());
        }
        if let Some(w) = &l.warning {
            ui.colored_label(ui.visuals().warn_fg_color, w);
        }
        // What was learned so far, newest last.
        let done: Vec<String> = (l.profile.notes.iter().rev().take(3).rev())
            .filter_map(|(id, n)| daw_project::kit::slot_by_id(id).map(|s| (s, n)))
            .map(|(s, n)| format!("{}: note {n}", s.name))
            .collect();
        if !done.is_empty() {
            ui.label(RichText::new(done.join("   ")).weak());
        }
        ui.horizontal(|ui| {
            let next = if waits {
                if l.current() == Step::Save {
                    "Save"
                } else {
                    "Next"
                }
            } else {
                "Skip: not on my kit"
            };
            if ui.button("Back").clicked() {
                cmd = Some(Cmd::KitBack);
            }
            if ui.button(next).clicked() {
                cmd = Some(if waits { Cmd::KitNext } else { Cmd::KitSkip });
            }
            if ui.button("Cancel").clicked() {
                cmd = Some(Cmd::KitCancel);
            }
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
        cmd
    }
}

fn is_kit_file(p: &Path) -> bool {
    (p.file_name().and_then(|n| n.to_str())).is_some_and(|n| n.ends_with(KIT_EXTENSION))
}

/// A file name for a kit: its name with only safe characters.
fn file_name(name: &str) -> String {
    let safe: String = (name.chars())
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.{KIT_EXTENSION}")
}

impl App {
    /// Runs one of the drum kit commands.
    pub(crate) fn kit_command(&mut self, cmd: Cmd) {
        let k = &mut self.kits;
        match cmd {
            Cmd::DrumKits => k.open = true,
            Cmd::LearnKit => {
                k.open = true;
                k.learn = Some(Learn::new());
            }
            Cmd::KitNext => {
                if let Some(l) = &mut k.learn
                    && let Some(kit) = l.next()
                {
                    let name = kit.name.clone();
                    k.learn = None;
                    self.message = Some(match k.keep(kit) {
                        Ok(()) => format!("Saved the {name} drum kit."),
                        Err(e) => e,
                    });
                }
            }
            Cmd::KitSkip => {
                if let Some(l) = &mut k.learn {
                    l.skip();
                    l.suggest_name();
                }
            }
            Cmd::KitBack => {
                if let Some(l) = &mut k.learn {
                    l.back();
                }
            }
            Cmd::KitCancel => k.learn = None,
            Cmd::ImportKit => {
                let picked = rfd::FileDialog::new()
                    .set_title("Import drum kit")
                    .set_directory(crate::user_dir())
                    .add_filter("Drum kit", &["txt"])
                    .pick_file();
                if let Some(p) = picked {
                    self.import_kit(&p);
                }
            }
            Cmd::ExportKit => {
                let Some(kit) = k.to_export() else {
                    self.message = Some("Learn or import a kit first.".into());
                    return;
                };
                let picked = rfd::FileDialog::new()
                    .set_title("Export drum kit")
                    .set_directory(crate::user_dir())
                    .set_file_name(file_name(&kit.name))
                    .add_filter("Drum kit", &["txt"])
                    .save_file();
                if let Some(p) = picked {
                    self.export_kit(&p);
                }
            }
            _ => {}
        }
    }

    /// Adds the kit profile in a file to the learned ones.
    pub(crate) fn import_kit(&mut self, path: &Path) {
        let kit = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))
            .and_then(|t| KitProfile::from_text(&t));
        self.message = Some(match kit {
            Ok(kit) if kit.name == GENERAL_MIDI => {
                "That name is the built-in General MIDI map's: rename the kit in the file.".into()
            }
            Ok(kit) => {
                let name = kit.name.clone();
                match self.kits.keep(kit) {
                    Ok(()) => format!("Added the {name} drum kit."),
                    Err(e) => e,
                }
            }
            Err(e) => e,
        });
    }

    /// Writes the kit picked in the window (or the latest learned) to a
    /// file, to share or to build into the app.
    pub(crate) fn export_kit(&mut self, path: &Path) {
        let Some(kit) = self.kits.to_export() else {
            return;
        };
        if let Err(e) = std::fs::write(path, kit.to_text()) {
            self.message = Some(format!("Could not write {}: {e}", path.display()));
        }
    }

    /// The window, and the note when a known kit is plugged in.
    pub(crate) fn kits_update(&mut self, ctx: &egui::Context) {
        if let Some(said) = self.kits.notice(&self.midi.ports.inputs) {
            self.message = Some(said);
        }
        let inputs = self.midi.ports.inputs.clone();
        if let Some(c) = self.kits.window(ctx, &inputs) {
            self.run(ctx, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::midi::Event;

    fn cap(event: Event) -> Captured {
        Captured {
            port: 0,
            event,
            at: 0,
            playing: false,
            recording: false,
        }
    }

    fn hit(l: &mut Learn, pitch: u8) -> bool {
        l.hear(Some("Kit USB"), &cap(Event::note_on(0, 9, pitch, 100, 0)))
    }

    fn cc(l: &mut Learn, number: u8, value: u8) {
        l.hear(
            Some("Kit USB"),
            &cap(Event {
                frame: 0,
                channel: 9,
                kind: Kind::Control { number, value },
            }),
        );
    }

    fn to(l: &mut Learn, step: Step) {
        while l.current() != step {
            l.skip();
        }
    }

    #[test]
    fn hits_are_learned_in_turn_and_pads_not_on_the_kit_are_skipped() {
        let mut l = Learn::new();
        assert_eq!(l.current(), Step::Hit(&SLOTS[0]));
        // A note-off or a controller doesn't count as a hit.
        assert!(!l.hear(None, &cap(Event::note_off(0, 9, 36, 0))));
        cc(&mut l, 4, 10);
        assert!(hit(&mut l, 36));
        assert!(hit(&mut l, 38));
        l.skip();
        assert!(hit(&mut l, 37));
        assert_eq!(l.profile.note("kick"), Some(36));
        assert_eq!(l.profile.note("snare-rim"), None);
        assert_eq!(l.profile.note("snare-cross-stick"), Some(37));
        assert_eq!(l.profile.ports, ["Kit USB"]);
        // Back, and learn it again.
        l.back();
        assert!(hit(&mut l, 39));
        assert_eq!(l.profile.note("snare-cross-stick"), Some(39));
    }

    #[test]
    fn a_pad_on_a_note_already_taken_is_warned_about() {
        let mut l = Learn::new();
        hit(&mut l, 36);
        assert!(l.warning.is_none());
        hit(&mut l, 36);
        assert!(l.warning.as_deref().unwrap().contains("Kick"));
        // The hi-hat's open and closed hits can share a note.
        to(
            &mut l,
            Step::Hit(daw_project::kit::slot_by_id("hihat-open").unwrap()),
        );
        hit(&mut l, 46);
        to(
            &mut l,
            Step::Hit(daw_project::kit::slot_by_id("hihat-closed").unwrap()),
        );
        hit(&mut l, 46);
        assert!(l.warning.is_none());
    }

    #[test]
    fn the_pedal_range_is_learned_from_its_controller() {
        let mut l = Learn::new();
        to(&mut l, Step::PedalDown);
        cc(&mut l, 4, 90);
        cc(&mut l, 4, 127);
        l.next();
        assert_eq!(l.current(), Step::PedalUp);
        // Another controller moving doesn't count.
        cc(&mut l, 11, 5);
        cc(&mut l, 4, 0);
        l.next();
        assert_eq!(
            l.profile.pedal,
            Some(Pedal {
                controller: 4,
                closed: 127,
                open: 0
            })
        );
        assert_eq!(l.current(), Step::Hit(&SLOTS[17]));
    }

    #[test]
    fn a_pedal_that_sends_no_controller_skips_its_range() {
        let mut l = Learn::new();
        to(&mut l, Step::PedalDown);
        l.next();
        assert_eq!(l.current(), Step::Hit(&SLOTS[17]));
        assert_eq!(l.profile.pedal, None);
        // Back goes over the skipped pedal steps.
        l.back();
        assert_eq!(l.current(), Step::PedalDown);
    }

    #[test]
    fn a_choke_is_pressure_or_a_controller_other_than_the_pedal() {
        let mut l = Learn::new();
        to(&mut l, Step::Choke);
        l.profile.pedal = Some(Pedal {
            controller: 4,
            closed: 127,
            open: 0,
        });
        cc(&mut l, 4, 3);
        assert_eq!(l.current(), Step::Choke);
        l.hear(
            None,
            &cap(Event {
                frame: 0,
                channel: 9,
                kind: Kind::KeyPressure {
                    pitch: 49,
                    value: 100,
                },
            }),
        );
        assert_eq!(l.profile.choke, Some(Choke::KeyPressure));
        assert_eq!(l.current(), Step::Save);
    }

    #[test]
    fn saving_needs_a_name_and_keeps_the_channel_heard() {
        let mut l = Learn::new();
        hit(&mut l, 36);
        to(&mut l, Step::Save);
        l.suggest_name();
        assert_eq!(l.name, "Kit USB");
        l.name.clear();
        assert!(l.next().is_none());
        assert!(l.warning.is_some());
        l.name = "My kit".into();
        let k = l.next().unwrap();
        assert_eq!((k.name.as_str(), k.channel), ("My kit", Some(9)));
    }

    #[test]
    fn a_kept_kit_is_saved_used_for_its_input_and_read_back() {
        let dir = std::env::temp_dir().join(format!("daw-kits-{}", daw_project::new_id()));
        let mut kits = Kits::new(Some(dir.clone()));
        assert_eq!(kits.for_input("Kit USB").name, GENERAL_MIDI);
        let mut k = KitProfile::new("My kit");
        k.ports = vec!["Kit USB".into()];
        k.set("kick", Some(30));
        kits.keep(k.clone()).unwrap();
        assert_eq!(kits.for_input("Kit USB").name, "My kit");
        let again = Kits::new(Some(dir.clone()));
        assert_eq!(again.learned, [k]);
        assert_eq!(again.for_input("Kit USB").name, "My kit");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_recognised_kit_is_used_and_said_once_unless_one_was_chosen() {
        let mut kits = Kits::new(None);
        let mut k = KitProfile::new("Known");
        k.ports = vec!["EXS-5".into()];
        kits.built_in.push(k);
        let inputs = vec!["EXS-5 MIDI 1".to_string(), "Piano".to_string()];
        assert!(kits.notice(&inputs).unwrap().contains("Known"));
        assert!(kits.notice(&inputs).is_none());
        assert_eq!(kits.for_input("EXS-5 MIDI 1").name, "Known");
        assert_eq!(kits.for_input("Piano").name, GENERAL_MIDI);
        kits.choose("EXS-5 MIDI 1", GENERAL_MIDI);
        assert_eq!(kits.for_input("EXS-5 MIDI 1").name, GENERAL_MIDI);
    }

    mod ui {
        use super::*;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        #[test]
        fn a_kit_is_learned_and_saved_through_the_commands() {
            let mut h = Harness::builder()
                .with_size([1200.0, 800.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let ctx = h.ctx.clone();
            // The steps can't run with no wizard going.
            h.state_mut().run(&ctx, Cmd::KitSkip);
            assert!(!h.state().kits.learning());
            h.state_mut().run(&ctx, Cmd::LearnKit);
            h.run();
            assert!(h.query_by_label("Kick").is_some());
            let inputs = vec!["Kit USB".to_string()];
            let kick = cap(Event::note_on(0, 9, 22, 100, 0));
            h.state_mut().kits.hear(&[kick], &inputs);
            h.run();
            assert!(h.query_by_label("Snare head").is_some());
            assert!(h.query_by_label_contains("Kick: note 22").is_some());
            while h.state().kits.learn.as_ref().unwrap().current() != Step::Save {
                h.state_mut().run(&ctx, Cmd::KitSkip);
            }
            h.run();
            assert_eq!(h.state().kits.learn.as_ref().unwrap().name, "Kit USB");
            h.state_mut().run(&ctx, Cmd::KitNext);
            h.run();
            let app = h.state();
            assert!(!app.kits.learning());
            assert_eq!(app.kits.for_input("Kit USB").note("kick"), Some(22));
            assert!(h.query_by_label("Kit USB").is_some());
        }
    }
}
