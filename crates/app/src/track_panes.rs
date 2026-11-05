// SPDX-License-Identifier: GPL-3.0-or-later
//! The Instrument, Effects and Notation panes (UI polish plan, step
//! three): what were floating windows over the timeline, now panes of
//! the workspace.
//!
//! Each shows one track: the one last opened (from the inspector, a
//! mixer strip, a track header or a command), and then whichever track
//! is selected, so clicking another track shows its instrument, effects
//! or notation in the same place. The Effects pane can show the master's
//! chain too.
//!
//! The app keeps which track each shows (`App.instrument_open`,
//! `Effects.open_track` and `open_master`, `NotationUi.shown`); none
//! means the pane is closed. `sync_track_panes` keeps those and the
//! workspace in step each frame, the way `Workspace::sync` does for the
//! comments and the tuner. Commands `instrument-window`, `effects` and
//! `notation-settings` open them; `get_selection.panes` says what each
//! shows.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::App;
use crate::layers;
use crate::workspace::{Pane, Workspace};

/// The selected track when the panes last looked, to follow a new one.
#[derive(Default)]
pub(crate) struct Follow {
    /// None before the first look.
    seen: Option<Option<String>>,
}

/// Keeps one track pane and the track it shows in step: `shown` is none
/// while it is closed. A track that went is replaced by `pick`; with
/// nothing to pick, a pane open in the layout stays open with a line
/// saying so. True if the layout changed.
fn keep(
    ws: &mut Workspace,
    p: Pane,
    shown: &mut Option<usize>,
    valid: impl Fn(usize) -> bool,
    pick: Option<usize>,
) -> bool {
    if shown.is_some_and(|i| !valid(i)) {
        *shown = pick;
    }
    let mut open = shown.is_some() || (ws.layout.is_open(p) && pick.is_none());
    let changed = ws.sync(p, &mut open);
    if !open {
        *shown = None;
    } else if shown.is_none() {
        *shown = pick;
    }
    changed
}

impl App {
    /// The track the Instrument pane opens on: the selected track if it
    /// plays notes, else one with an instrument, else any that plays
    /// notes.
    pub(crate) fn instrument_pick(&mut self) -> Option<usize> {
        (self.inspected())
            .filter(|&i| self.tracks[i].kind.plays_notes())
            .or_else(|| self.instrument_track(None))
            .or_else(|| self.tracks.iter().position(|t| t.kind.plays_notes()))
    }

    /// The track the Notation pane opens on: the selected track if it
    /// plays notes, else the one notation commands act on.
    fn notation_pick(&mut self) -> Option<usize> {
        (self.inspected())
            .filter(|&i| self.tracks[i].kind.plays_notes())
            .or_else(|| self.notation_track())
    }

    /// Opens the Effects pane on the track it shows, else the selected
    /// track.
    pub(crate) fn open_effects_pane(&mut self) {
        let valid = self.effects.open_track.filter(|&i| i < self.tracks.len());
        match valid.or_else(|| self.inspected()) {
            Some(i) => {
                self.effects.open_track = Some(i);
                self.effects.open_master = false;
            }
            None => self.message = Some("There is no track yet.".into()),
        }
    }

    /// Shows track `ti` in the pane command `cmd` opens, opening it.
    pub(crate) fn show_in_track_pane(&mut self, cmd: crate::commands::Cmd, ti: usize) {
        use crate::commands::Cmd;
        match cmd {
            Cmd::InstrumentWindow if self.tracks[ti].kind.plays_notes() => {
                self.instrument_open = Some(ti);
            }
            Cmd::EffectsPane => {
                self.effects.open_track = Some(ti);
                self.effects.open_master = false;
            }
            Cmd::NotationSettings if self.tracks[ti].kind.plays_notes() => {
                self.notation.shown = Some(self.tracks[ti].key.clone());
            }
            _ => self.message = Some(format!("{} doesn't play notes.", self.tracks[ti].name)),
        }
    }

    /// A newly selected track is shown in the track panes that are open.
    fn follow_into_track_panes(&mut self) {
        let sel = self.inspected();
        let key = sel.map(|i| self.tracks[i].key.clone());
        let first = self.track_panes.seen.is_none();
        if self.track_panes.seen.as_ref() == Some(&key) {
            return;
        }
        self.track_panes.seen = Some(key.clone());
        let (Some(i), false) = (sel, first) else {
            return;
        };
        let notes = self.tracks[i].kind.plays_notes();
        if self.instrument_open.is_some() && notes {
            self.instrument_open = Some(i);
        }
        if self.effects.open_track.is_some() {
            self.effects.open_track = Some(i);
        }
        if self.notation.shown.is_some() && notes {
            self.notation.shown = key;
        }
    }

    /// Keeps the Instrument, Effects and Notation panes in step with the
    /// tracks they show. True if the layout changed.
    pub(crate) fn sync_track_panes(&mut self) -> bool {
        self.follow_into_track_panes();
        let notes: Vec<bool> = self.tracks.iter().map(|t| t.kind.plays_notes()).collect();
        let plays = |i: usize| notes.get(i).copied().unwrap_or(false);

        let pick = self.instrument_pick();
        let mut shown = self.instrument_open;
        let mut changed = keep(
            &mut self.workspace,
            Pane::Instrument,
            &mut shown,
            plays,
            pick,
        );
        self.instrument_open = shown;

        let n = self.tracks.len();
        let pick = self.inspected();
        let ws = &mut self.workspace;
        let fx = &mut self.effects;
        if fx.open_master {
            fx.open_track = None;
            let mut open = true;
            changed |= ws.sync(Pane::Effects, &mut open);
            fx.open_master = open;
        } else {
            changed |= keep(ws, Pane::Effects, &mut fx.open_track, |i| i < n, pick);
        }

        let pick = self.notation_pick();
        let mut shown = (self.notation.shown.as_deref()).and_then(|k| self.track_index(k));
        if self.notation.shown.is_some() && shown.is_none() {
            // Its track went: keep it open, on another.
            shown = Some(usize::MAX);
        }
        changed |= keep(&mut self.workspace, Pane::Notation, &mut shown, plays, pick);
        self.notation.shown = shown.map(|i| self.tracks[i].key.clone());
        changed
    }

    /// What `get_selection` says of the track panes: whether each is
    /// open and the track it shows ("master" for the master's effects).
    pub(crate) fn track_panes_value(&self) -> Value {
        let ws = &self.workspace.layout;
        let key = |i: Option<usize>| i.and_then(|i| self.tracks.get(i)).map(|t| t.key.clone());
        let effects = if self.effects.open_master {
            Some("master".to_string())
        } else {
            key(self.effects.open_track)
        };
        json!({
            "instrument": {"open": ws.is_open(Pane::Instrument), "track": key(self.instrument_open)},
            "effects": {"open": ws.is_open(Pane::Effects), "track": effects},
            "notation": {"open": ws.is_open(Pane::Notation), "track": self.notation.shown},
        })
    }

    /// The Instrument pane.
    pub(crate) fn instrument_pane(&mut self, ui: &mut egui::Ui) {
        match self.instrument_open {
            Some(i) => self.instrument_panel(ui, i),
            None => {
                ui.label(
                    RichText::new("Add an instrument track: its instrument shows here.").weak(),
                );
            }
        }
    }

    /// The Effects pane: the master's chain, or the shown track's.
    pub(crate) fn effects_pane(&mut self, ui: &mut egui::Ui) {
        if self.effects.open_master {
            let (_, missing) = self.frozen_note(&self.master);
            let mut fx = std::mem::take(&mut self.master.fx);
            let changed = self.effects.track_panel(
                ui,
                "Master",
                &mut fx,
                &[],
                &missing,
                None,
                self.rate,
                &[],
                &mut self.master.rack,
                None,
                &mut self.message,
            );
            self.master.fx = fx;
            if changed {
                self.rebuild();
            }
            return;
        }
        let Some(i) = self.effects.open_track.filter(|&i| i < self.tracks.len()) else {
            ui.label(RichText::new("Add a track: its effects show here.").weak());
            return;
        };
        let (note, missing) = self.frozen_note(&self.tracks[i]);
        let others: Vec<(String, String)> = (self.tracks.iter().enumerate())
            .filter(|(j, _)| *j != i)
            .map(|(_, t)| (t.key.clone(), t.name.clone()))
            .collect();
        let t = &mut self.tracks[i];
        let layers = layers::chains(&t.layers);
        let instrument = t.instrument.as_ref().map(|s| s.name.clone());
        if self.effects.track_panel(
            ui,
            &t.name,
            &mut t.fx,
            &layers,
            &missing,
            note.as_deref(),
            self.rate,
            &others,
            &mut t.rack,
            instrument.as_deref(),
            &mut self.message,
        ) {
            self.rebuild();
        }
    }

    /// The Notation pane.
    pub(crate) fn notation_pane(&mut self, ui: &mut egui::Ui) {
        match (self.notation.shown.as_deref()).and_then(|k| self.track_index(k)) {
            Some(i) => self.notation_panel(ui, i),
            None => {
                ui.label(
                    RichText::new("Add an instrument track: its notation lane is set here.").weak(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    /// An assistant opens the three panes, reads which track each shows,
    /// sees them follow the selected track, and closes them.
    #[test]
    fn track_panes_work_with_an_assistant() {
        let mut d = Driver::new();
        let id = |v: serde_json::Value| v["track"].as_str().unwrap().to_string();
        let guitar = id(d
            .call("add_track", json!({"kind": "audio", "name": "Guitar"}))
            .unwrap());
        let keys = id(d
            .call("add_track", json!({"kind": "instrument", "name": "Keys"}))
            .unwrap());
        let bass = id(d
            .call("add_track", json!({"kind": "instrument", "name": "Bass"}))
            .unwrap());
        d.h.run();
        let panes = |d: &mut Driver| d.call("get_selection", json!({})).unwrap()["panes"].clone();
        let p = panes(&mut d);
        for name in ["instrument", "effects", "notation"] {
            assert_eq!(p[name]["open"], false, "{name}");
            assert_eq!(p[name]["track"], json!(null), "{name}");
        }

        // The audio track is selected, so the instrument opens on the
        // first instrument track; it is a pane, not a window.
        d.command("instrument-window");
        d.h.run();
        let p = panes(&mut d);
        assert_eq!(p["instrument"]["open"], true);
        assert_eq!(p["instrument"]["track"], json!(keys));
        assert!(d.app().workspace.layout.docked(Pane::Instrument));
        assert!(
            d.h.query_all_by_label("Instrument").count() >= 2,
            "the tab and the choice"
        );

        // Selecting another instrument track shows it there.
        d.call("select_track", json!({"track": bass})).unwrap();
        d.h.run();
        assert_eq!(panes(&mut d)["instrument"]["track"], json!(bass));

        // Effects open on the selected track, and follow the audio
        // track; the instrument stays on the last one that plays notes.
        d.command("effects");
        d.h.run();
        assert_eq!(panes(&mut d)["effects"]["track"], json!(bass));
        d.h.get_by_label("Add effect");
        d.call("select_track", json!({"track": guitar})).unwrap();
        d.h.run();
        let p = panes(&mut d);
        assert_eq!(p["effects"]["track"], json!(guitar));
        assert_eq!(p["instrument"]["track"], json!(bass));

        // A track given shows that track.
        d.call(
            "run_command",
            json!({"id": "notation-settings", "track": keys}),
        )
        .unwrap();
        d.h.run();
        let p = panes(&mut d);
        assert_eq!(p["notation"]["open"], true);
        assert_eq!(p["notation"]["track"], json!(keys));
        d.h.get_by_label("Show a notation lane under the track")
            .click();
        d.h.run();
        assert!(d.app().tracks[1].notation.is_some());

        // Run again, each closes.
        for c in ["instrument-window", "effects", "notation-settings"] {
            d.command(c);
        }
        d.h.run();
        let p = panes(&mut d);
        for name in ["instrument", "effects", "notation"] {
            assert_eq!(p[name]["open"], false, "{name}");
        }
    }

    /// Closing a pane from its tab forgets its track, and the master's
    /// effects show in the same pane.
    #[test]
    fn closing_the_pane_forgets_its_track() {
        let mut d = Driver::new();
        d.call("add_track", json!({"kind": "instrument", "name": "Keys"}))
            .unwrap();
        d.h.run();
        d.app().instrument_open = Some(0);
        d.h.run();
        assert!(d.app().workspace.layout.is_open(Pane::Instrument));
        d.app().workspace.close(Pane::Instrument);
        d.h.run();
        assert_eq!(d.app().instrument_open, None);

        d.app().effects.open_master = true;
        d.h.run();
        let p = d.call("get_selection", json!({})).unwrap()["panes"].clone();
        assert_eq!(p["effects"], json!({"open": true, "track": "master"}));
        d.app().workspace.close(Pane::Effects);
        d.h.run();
        assert!(!d.app().effects.open_master);
    }
}
