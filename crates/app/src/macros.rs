// SPDX-License-Identifier: GPL-3.0-or-later
//! Macros: several commands chained to run one after another from one
//! key (Set C small items). A macro is one of the commands `macro-1` to
//! `macro-9` in the shared command table, so it has keys like any other
//! command, is found in the command search, is listed in the Edit menu
//! and is run by an assistant with `run_command`. Its steps are
//! commands, by id. A macro can't be a step of a macro, so none runs
//! itself.
//!
//! Macros belong to the computer, like the keys: they are kept in
//! `macros.txt` beside `keys.txt`, one line per macro (its id, a tab,
//! its name, a tab, its steps' ids separated by spaces), and changing
//! one makes no undo step. Settings → Key commands → Macros changes
//! them; assistants use `get_macros` and `edit_macro`.
//!
//! Some commands wait for the timeline or the panes to be drawn before
//! they act. A macro whose step waits so runs the rest of its steps on
//! the next frame, after it, so the steps always act in order.

use std::collections::VecDeque;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};
use crate::commands::{Cmd, Commands};
use crate::icons::{Icon, IconButton};

/// How many macros there can be: `macro-1` to `macro-9`.
pub(crate) const MOST: usize = 9;
/// Most steps in one macro.
pub(crate) const MOST_STEPS: usize = 32;

/// One macro: its name and the commands it runs, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Macro {
    pub(crate) name: String,
    pub(crate) steps: Vec<Cmd>,
    /// As shown in menus and the command search: "Macro 2: Count in".
    label: String,
}

impl Macro {
    fn new(n: usize, name: &str, steps: Vec<Cmd>) -> Macro {
        let name = name.trim().to_string();
        let label = match name.as_str() {
            "" => format!("Macro {}", n + 1),
            name => format!("Macro {}: {name}", n + 1),
        };
        Macro { name, steps, label }
    }
}

/// The command of macro `n`, counted from 0.
pub(crate) fn cmd(n: usize) -> Cmd {
    Cmd::Macro(n as u8)
}

/// Whether `cmd` may be a step: a macro may not, so none runs itself.
pub(crate) fn may_be_step(cmd: Cmd) -> bool {
    !matches!(cmd, Cmd::Macro(_))
}

/// Every macro, by its place (`macro-1` is 0).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Macros {
    slots: Vec<Option<Macro>>,
}

impl Macros {
    /// From `macros.txt`. Lines it can't read, and steps naming
    /// commands this version doesn't have, are left out.
    pub(crate) fn from_text(text: &str) -> Macros {
        let mut m = Macros::default();
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(id), name, steps) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let Some(Cmd::Macro(n)) = Cmd::from_id(id.trim()) else {
                continue;
            };
            let steps: Vec<Cmd> = (steps.unwrap_or("").split_whitespace())
                .filter_map(Cmd::from_id)
                .filter(|c| may_be_step(*c))
                .take(MOST_STEPS)
                .collect();
            m.set(n as usize, name.unwrap_or(""), steps);
        }
        m
    }

    pub(crate) fn to_text(&self) -> String {
        let mut t = String::new();
        for (n, m) in self.iter() {
            let steps: Vec<&str> = m.steps.iter().map(|c| c.info().id()).collect();
            t.push_str(&format!(
                "{}\t{}\t{}\n",
                cmd(n).info().id(),
                m.name.replace(['\t', '\n', '\r'], " "),
                steps.join(" ")
            ));
        }
        t
    }

    pub(crate) fn get(&self, n: usize) -> Option<&Macro> {
        self.slots.get(n).and_then(Option::as_ref)
    }

    /// The macros there are, by place.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, &Macro)> {
        (self.slots.iter().enumerate()).filter_map(|(n, m)| Some((n, m.as_ref()?)))
    }

    /// The first place free for a new macro.
    pub(crate) fn free(&self) -> Option<usize> {
        (0..MOST).find(|&n| self.get(n).is_none())
    }

    pub(crate) fn set(&mut self, n: usize, name: &str, steps: Vec<Cmd>) {
        if n >= MOST {
            return;
        }
        if self.slots.len() <= n {
            self.slots.resize(n + 1, None);
        }
        self.slots[n] = Some(Macro::new(n, name, steps));
    }

    pub(crate) fn remove(&mut self, n: usize) {
        if let Some(s) = self.slots.get_mut(n) {
            *s = None;
        }
    }

    /// What macro `n` is called in menus and the command search, if
    /// there is one.
    pub(crate) fn label(&self, n: usize) -> Option<&str> {
        self.get(n).map(|m| m.label.as_str())
    }

    /// Whether macro `n` has something to run.
    pub(crate) fn runs(&self, n: usize) -> bool {
        self.get(n).is_some_and(|m| !m.steps.is_empty())
    }
}

/// The Macros part of Settings → Key commands, while it is drawn.
#[derive(Default)]
pub(crate) struct MacrosUi {
    /// Shown open: the Macros… command opens it.
    pub(crate) open: bool,
    /// What is typed to find a step to add, by macro.
    find: [String; MOST],
}

/// The steps of a macro that waited for the timeline or the panes, to
/// run on the next frame.
#[derive(Default)]
pub(crate) struct Waiting {
    pub(crate) steps: VecDeque<Cmd>,
    /// The macro's name, to say where it stopped.
    pub(crate) name: String,
}

impl Commands {
    /// Where macros are kept, beside the keys.
    pub(crate) fn macros_path(path: Option<&PathBuf>) -> Option<PathBuf> {
        path.map(|p| p.with_file_name("macros.txt"))
    }

    pub(crate) fn save_macros(&mut self) {
        let Some(path) = self.macros_path.clone() else {
            return;
        };
        let text = self.macros.to_text();
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(tmp, &path)
        };
        if let Err(e) = write() {
            self.editor.note = Some(format!("Couldn't save the macros: {e}"));
        }
    }

    /// The assistant's edit_macro.
    fn edit_macro(&mut self, args: &Value) -> Result<Value, String> {
        let n = match args["macro"].as_str() {
            Some(id) => match Cmd::from_id(id) {
                Some(Cmd::Macro(n)) => n as usize,
                _ => {
                    return Err(format!(
                        "There is no macro {id}: they are macro-1 to macro-9."
                    ));
                }
            },
            None => self
                .macros
                .free()
                .ok_or("There are nine macros already; change or delete one.")?,
        };
        let id = cmd(n).info().id();
        if args["delete"].as_bool() == Some(true) {
            if self.macros.get(n).is_none() {
                return Err(format!("There is no macro {id} to delete."));
            }
            self.macros.remove(n);
            self.save_macros();
            return Ok(json!({"macro": id, "deleted": true}));
        }
        let had = self.macros.get(n).cloned().unwrap_or_default();
        let step = |v: &Value| -> Result<Cmd, String> {
            let s = v.as_str().ok_or("Steps are command ids, as text.")?;
            let c = Cmd::from_id(s).ok_or_else(|| format!("There is no command {s}."))?;
            if !may_be_step(c) {
                return Err(format!("{s} is a macro; a macro can't run a macro."));
            }
            Ok(c)
        };
        let mut steps = match args.get("steps") {
            Some(Value::Array(list)) => list.iter().map(step).collect::<Result<Vec<_>, _>>()?,
            Some(Value::Null) | None => had.steps.clone(),
            Some(_) => return Err("steps is a list of command ids.".into()),
        };
        if let Some(v) = args.get("add_step").filter(|v| !v.is_null()) {
            steps.push(step(v)?);
        }
        if steps.len() > MOST_STEPS {
            return Err(format!("A macro has at most {MOST_STEPS} steps."));
        }
        let name = args["name"].as_str().unwrap_or(&had.name).to_string();
        self.macros.set(n, &name, steps);
        self.save_macros();
        Ok(self.macro_value(n))
    }

    fn macro_value(&self, n: usize) -> Value {
        let Some(m) = self.macros.get(n) else {
            return Value::Null;
        };
        let c = cmd(n);
        json!({
            "macro": c.info().id(),
            "name": m.name,
            "shown_as": m.label,
            "keys": self.keymap.text(c),
            "steps": (m.steps.iter())
                .map(|s| json!({"id": s.info().id(), "name": s.info().name}))
                .collect::<Vec<_>>(),
        })
    }

    pub(crate) fn macros_value(&self) -> Value {
        let list: Vec<Value> = (self.macros.iter())
            .map(|(n, _)| self.macro_value(n))
            .collect();
        json!({"macros": list, "free": MOST - list.len(), "most_steps": MOST_STEPS})
    }

    /// Settings → Key commands → Macros.
    pub(crate) fn macros_page(&mut self, ui: &mut egui::Ui) {
        let open = std::mem::take(&mut self.macros_ui.open);
        let head = egui::CollapsingHeader::new(RichText::new("Macros").strong())
            .id_salt("macros")
            .open(open.then_some(true))
            .show(ui, |ui| {
                ui.weak(
                    "A macro runs several commands one after another. Give it a key \
                     below, under its name, or run it from the Edit menu.",
                );
                let mut delete = None;
                let mut changed = None;
                for (n, m) in self.macros.iter() {
                    ui.separator();
                    let mut name = m.name.clone();
                    let mut steps = m.steps.clone();
                    let mut dirty = false;
                    ui.horizontal(|ui| {
                        ui.strong(format!("Macro {}", n + 1));
                        if ui
                            .add(
                                // ai: edit_macro.name
                                egui::TextEdit::singleline(&mut name)
                                    .hint_text("Name")
                                    .desired_width(180.0),
                            )
                            .accessible_name(format!("Name of macro {}", n + 1))
                            .changed()
                        {
                            dirty = true;
                        }
                        let keys = self.keymap.label(cmd(n));
                        ui.weak(if keys.is_empty() {
                            "no key".into()
                        } else {
                            keys
                        });
                        let waiting = self.editor.capturing == Some(cmd(n));
                        if ui
                            // ai: edit_keys.add
                            .add(egui::Button::selectable(
                                waiting,
                                if waiting { "Press a key…" } else { "Add key" },
                            ))
                            .accessible_name(format!("Add a key for macro {}", n + 1))
                            .clicked()
                        {
                            self.editor.capturing = (!waiting).then_some(cmd(n));
                            self.editor.clash = None;
                            self.editor.note = None;
                        }
                        if ui
                            // ai: edit_macro.delete
                            .add(IconButton::labelled(Icon::Close, "Delete"))
                            .accessible_name(format!("Delete macro {}", n + 1))
                            .on_hover_text("Delete this macro")
                            .clicked()
                        {
                            delete = Some(n);
                        }
                    });
                    if steps.is_empty() {
                        ui.weak("No steps yet: add the commands it runs.");
                    }
                    let mut swap = None;
                    let mut drop = None;
                    for (k, s) in steps.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}. {}", k + 1, s.info().name));
                            ui.weak(s.info().group);
                            if ui
                                // ai: edit_macro.steps
                                .add_enabled(k > 0, egui::Button::new("Up"))
                                .accessible_name(format!("Move step {} up", k + 1))
                                .clicked()
                            {
                                swap = Some(k - 1);
                            }
                            if ui
                                // ai: edit_macro.steps
                                .add_enabled(k + 1 < steps.len(), egui::Button::new("Down"))
                                .accessible_name(format!("Move step {} down", k + 1))
                                .clicked()
                            {
                                swap = Some(k);
                            }
                            if ui
                                // ai: edit_macro.steps
                                .add(IconButton::labelled(Icon::Close, "Remove"))
                                .accessible_name(format!("Remove step {}", k + 1))
                                .clicked()
                            {
                                drop = Some(k);
                            }
                        });
                    }
                    if let Some(k) = swap {
                        steps.swap(k, k + 1);
                        dirty = true;
                    }
                    if let Some(k) = drop {
                        steps.remove(k);
                        dirty = true;
                    }
                    if steps.len() < MOST_STEPS
                        && let Some(c) = add_step(ui, n, &mut self.macros_ui.find[n])
                    {
                        steps.push(c);
                        dirty = true;
                    }
                    if dirty {
                        changed = Some((n, name, steps));
                    }
                }
                ui.separator();
                if ui
                    // ai: edit_macro
                    .add_enabled(self.macros.free().is_some(), egui::Button::new("New macro"))
                    .on_hover_text("Up to nine macros, macro-1 to macro-9")
                    .clicked()
                    && let Some(n) = self.macros.free()
                {
                    self.macros.set(n, "", Vec::new());
                    self.save_macros();
                }
                if let Some((n, name, steps)) = changed {
                    self.macros.set(n, &name, steps);
                    self.save_macros();
                }
                if let Some(n) = delete {
                    self.macros.remove(n);
                    self.save_macros();
                }
            });
        if open {
            head.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
    }
}

/// The field to find a command to add as a step, and the commands that
/// match it. Gives back the one clicked.
fn add_step(ui: &mut egui::Ui, n: usize, find: &mut String) -> Option<Cmd> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.add(
            // ai: view
            egui::TextEdit::singleline(find)
                .hint_text("Add a step: type a command's name")
                .desired_width(260.0),
        )
        .accessible_name(format!("Find a command to add to macro {}", n + 1));
    });
    if find.trim().is_empty() {
        return None;
    }
    let found: Vec<Cmd> = crate::commands::search_named(find, &Default::default())
        .into_iter()
        .filter(|c| may_be_step(*c))
        .take(6)
        .collect();
    if found.is_empty() {
        ui.weak("No command has that name.");
    }
    ui.horizontal_wrapped(|ui| {
        for c in found {
            if ui
                // ai: edit_macro.add_step
                .selectable_label(false, c.info().name)
                .on_hover_text(format!(
                    "Add {} as a step ({})",
                    c.info().name,
                    c.info().group
                ))
                .clicked()
            {
                picked = Some(c);
            }
        }
    });
    if picked.is_some() {
        find.clear();
    }
    picked
}

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_macros",
        description: "The macros: each one's id (macro-1 to macro-9, which run_command runs \
                      and edit_keys gives keys to), its name, keys, and the commands it runs \
                      in order, by id.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "edit_macro",
        description: "Makes or changes a macro: several commands (by their ids, from \
                      list_commands) run one after another from one key or run_command. \
                      Without macro, makes a new one in the first free place. steps \
                      replaces its steps; add_step adds one at the end. A macro can't be \
                      a step. delete takes it away (its keys stay with its id). Kept on \
                      this computer like the keys, so there is no undo step. edit_keys \
                      gives it a key.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "macro": {"type": "string", "description": "macro-1 to macro-9."},
                "name": {"type": "string"},
                "steps": {"type": "array", "items": {"type": "string"},
                          "description": "Command ids, in the order they run."},
                "add_step": {"type": "string", "description": "A command id to run last."},
                "delete": {"type": "boolean"}
            }})
        },
    },
];

impl App {
    pub(crate) fn macros_read(&mut self, name: &str) -> Option<Result<Value, String>> {
        (name == "get_macros").then(|| Ok(self.commands.macros_value()))
    }

    pub(crate) fn macros_tool(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Option<Result<(Value, Option<String>), String>> {
        (name == "edit_macro").then(|| self.commands.edit_macro(args).map(|v| (v, None)))
    }

    /// Runs macro `n`'s steps in order. A step that waits for the
    /// timeline or the panes to be drawn holds the rest back until the
    /// next frame; a step that can't run stops the macro and says so.
    pub(crate) fn run_macro(&mut self, ctx: &egui::Context, n: usize) {
        let Some(m) = self.commands.macros.get(n) else {
            return;
        };
        self.macro_waiting = Waiting {
            steps: m.steps.iter().copied().collect(),
            name: m.label.clone(),
        };
        self.macro_steps(ctx);
    }

    /// Runs the waiting steps of a macro until one has to wait.
    pub(crate) fn macro_steps(&mut self, ctx: &egui::Context) {
        while let Some(step) = self.macro_waiting.steps.pop_front() {
            if !self.can_run(step) {
                self.message = Some(format!(
                    "{} stopped: {} can't run now.",
                    self.macro_waiting.name,
                    self.commands.name(step).trim_end_matches('…')
                ));
                self.macro_waiting.steps.clear();
                return;
            }
            let queued = (self.commands.timeline.len(), self.workspace_cmds.len());
            self.run(ctx, step);
            if (self.commands.timeline.len(), self.workspace_cmds.len()) != queued {
                // It acts when the timeline or panes are next drawn.
                ctx.request_repaint();
                return;
            }
        }
    }
}

/// Every macro command is in the table, in order.
#[cfg(test)]
pub(crate) fn every_macro_is_a_command() -> bool {
    (0..MOST).all(|n| crate::commands::COMMANDS.iter().any(|i| i.cmd == cmd(n)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::driver::Driver;

    #[test]
    fn macros_are_written_and_read_back() {
        assert!(every_macro_is_a_command());
        let mut m = Macros::default();
        m.set(0, "Count in", vec![Cmd::CountIn, Cmd::Metronome]);
        m.set(4, "", vec![Cmd::AddBus]);
        let text = m.to_text();
        assert_eq!(
            text,
            "macro-1\tCount in\tcount-in metronome\nmacro-5\t\tadd-bus\n"
        );
        assert_eq!(Macros::from_text(&text), m);
        assert_eq!(m.label(0), Some("Macro 1: Count in"));
        assert_eq!(m.label(4), Some("Macro 5"));
        assert_eq!(m.free(), Some(1));
        // Commands this version lacks, and macros as steps, are left out.
        let back = Macros::from_text("macro-2\tX\tadd-bus no-such macro-1\nnonsense\n");
        assert_eq!(back.get(1).unwrap().steps, [Cmd::AddBus]);
        assert_eq!(back.iter().count(), 1);
    }

    #[test]
    fn an_assistant_makes_a_macro_gives_it_a_key_and_runs_it() {
        let mut d = Driver::new();
        let tracks = |d: &mut Driver| d.project()["tracks"].as_array().unwrap().len();
        let before = tracks(&mut d);
        // Nothing to run yet.
        let e = d.call("run_command", json!({"id": "macro-1"})).unwrap_err();
        assert!(e.contains("can't run now"), "{e}");

        let m = d
            .call(
                "edit_macro",
                json!({"name": "Two buses", "steps": ["add-bus", "add-bus"]}),
            )
            .unwrap();
        assert_eq!(m["macro"], "macro-1");
        assert_eq!(m["shown_as"], "Macro 1: Two buses");
        let m = d
            .call(
                "edit_macro",
                json!({"macro": "macro-1", "add_step": "add-audio-track"}),
            )
            .unwrap();
        assert_eq!(m["steps"].as_array().unwrap().len(), 3);
        assert_eq!(m["steps"][2]["id"], "add-audio-track");
        // Refused: a macro as a step, a command that isn't there.
        assert!(
            d.call(
                "edit_macro",
                json!({"macro": "macro-1", "add_step": "macro-1"})
            )
            .unwrap_err()
            .contains("can't run a macro")
        );
        assert!(d.call("edit_macro", json!({"steps": ["no-such"]})).is_err());
        assert!(
            d.call("edit_macro", json!({"macro": "macro-10", "name": "x"}))
                .is_err()
        );

        // A key, as any command has; the command list shows its name.
        let k = d
            .call("edit_keys", json!({"command": "macro-1", "add": "F7"}))
            .unwrap();
        assert_eq!(k["keys"].as_array().unwrap().len(), 1);
        let list = d.call("list_commands", json!({})).unwrap();
        let row = (list.as_array().unwrap().iter())
            .find(|c| c["id"] == "macro-1")
            .unwrap()
            .clone();
        assert_eq!(row["name"], "Macro 1: Two buses");
        assert_eq!(row["can_run"], true);
        let got = d.call("get_macros", json!({})).unwrap();
        assert_eq!(got["macros"][0]["name"], "Two buses");
        assert_eq!(got["free"], 8);

        // Running it runs every step, in order.
        d.command("macro-1");
        assert_eq!(tracks(&mut d), before + 3);
        let kinds: Vec<String> = (d.project()["tracks"].as_array().unwrap().iter())
            .map(|t| t["kind"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            (kinds.iter()).filter(|k| *k == "bus").count(),
            2,
            "{kinds:?}"
        );

        // Its key runs it too.
        d.h.key_press(egui::Key::F7);
        d.h.run();
        assert_eq!(tracks(&mut d), before + 6);

        d.app().commands.keymap = crate::commands::Keymap::default();
        d.call("edit_macro", json!({"macro": "macro-1", "delete": true}))
            .unwrap();
        assert!(
            d.call("get_macros", json!({})).unwrap()["macros"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            d.call("run_command", json!({"id": "macro-1"}))
                .unwrap_err()
                .contains("can't run now")
        );
    }

    #[test]
    fn a_step_that_waits_for_the_timeline_holds_the_rest_back() {
        let mut d = Driver::new();
        d.call(
            "edit_macro",
            json!({"steps": ["zoom-in", "add-bus"], "name": "Zoom then bus"}),
        )
        .unwrap();
        let before = d.project()["tracks"].as_array().unwrap().len();
        d.command("macro-1");
        // The bus comes once the zoom has been done, a frame later.
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), before + 1);
        assert!(d.app().macro_waiting.steps.is_empty());
    }

    #[test]
    fn a_step_that_cant_run_stops_the_macro() {
        let mut d = Driver::new();
        d.call("edit_macro", json!({"steps": ["redo", "add-bus"]}))
            .unwrap();
        let before = d.project()["tracks"].as_array().unwrap().len();
        d.command("macro-1");
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), before);
        let msg = d.app().message.clone().unwrap_or_default();
        assert!(msg.contains("stopped"), "{msg}");
    }
}
