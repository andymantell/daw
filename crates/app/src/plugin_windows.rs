// SPDX-License-Identifier: GPL-3.0-or-later
//! The plugin windows (Set C small items, from the user wish list):
//!
//! - **Tabs:** one window with a tab for each plugin on a track (its
//!   plugin instrument and layers first, then its effects). The tab
//!   shows the plugin's controls, and opens or closes the plugin's own
//!   window. While one of the track's own windows is open, choosing
//!   another tab moves it there, so one plugin window stands in for all.
//! - **Following the selected track:** while on (the default), selecting
//!   another track shows its plugins, and an own window that was open
//!   moves to the new track's tab.
//! - **The time each plugin takes:** each plugin's audio is timed where
//!   it runs (in the app, or by the helper process), and shown as the
//!   share of one processor core it used over the last moment.
//! - **The plugins a project uses:** every plugin on every track and the
//!   master, where it runs, its share, and any not installed here.
//!
//! Commands `plugin-windows`, `plugin-windows-follow`, `next-plugin-tab`,
//! `previous-plugin-tab` and `project-plugins` (View → Plugin windows),
//! and the assistant tools `get_plugin_windows` and `set_plugin_windows`,
//! reach it. It only changes what is shown, so there is no undo step.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use daw_engine::graph::{Insert, Split};
use daw_engine::midi::{Event, Instrument, Outputs};
use eframe::egui;
use serde_json::{Value, json};

use crate::agent::{Access, Tool};
use crate::commands::Cmd;
use crate::fx::FxSlot;
use crate::{App, Track};

/// How long a share is measured over, at least.
const OVER: Duration = Duration::from_millis(500);

/// The time an effect's or instrument's audio takes, counted on the
/// audio thread (or by its helper process) and read by the UI.
#[derive(Debug, Default)]
pub(crate) struct Load {
    /// Nanoseconds spent in its audio in the app, since it was made.
    busy: AtomicU64,
    /// The same, as its helper process last reported it.
    helper: AtomicU64,
    seen: Mutex<Seen>,
}

#[derive(Debug, Default)]
struct Seen {
    total: u64,
    at: Option<Instant>,
    share: Option<f32>,
}

impl Load {
    /// Adds time spent in its audio. Called on the audio thread: it
    /// neither locks nor allocates.
    pub(crate) fn add(&self, d: Duration) {
        self.busy.fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
    }

    /// The total its helper process reports.
    pub(crate) fn helper_busy(&self, ns: u64) {
        self.helper.store(ns, Ordering::Relaxed);
    }

    /// The share of one processor core its audio took over the last half
    /// second or so (0.05 is 5%); None until it has been measured once.
    /// `remote` reads the helper's count.
    pub(crate) fn share(&self, remote: bool) -> Option<f32> {
        self.share_at(remote, Instant::now())
    }

    fn share_at(&self, remote: bool, now: Instant) -> Option<f32> {
        let total = if remote {
            self.helper.load(Ordering::Relaxed)
        } else {
            self.busy.load(Ordering::Relaxed)
        };
        let mut s = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        match s.at {
            // A plugin made again (in a new helper) counts from 0.
            Some(at) if total >= s.total => {
                let gap = now.saturating_duration_since(at);
                if gap >= OVER {
                    s.share = Some((total - s.total) as f32 / gap.as_nanos() as f32);
                    s.total = total;
                    s.at = Some(now);
                }
            }
            _ => {
                s.total = total;
                s.at = Some(now);
            }
        }
        s.share
    }
}

/// An effect or instrument whose audio is timed into a [`Load`].
pub(crate) struct Timed<T> {
    inner: T,
    load: Arc<Load>,
}

impl<T> Timed<T> {
    pub(crate) fn new(inner: T, load: Arc<Load>) -> Timed<T> {
        Timed { inner, load }
    }
}

impl<T: Insert> Insert for Timed<T> {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let start = Instant::now();
        self.inner.process(left, right);
        self.load.add(start.elapsed());
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        let start = Instant::now();
        self.inner.process_keyed(left, right, key);
        self.load.add(start.elapsed());
    }

    fn set_param(&mut self, param: u32, value: f64) {
        self.inner.set_param(param, value);
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.inner.set_tempo(bpm);
    }

    fn as_split(&mut self) -> Option<&mut dyn Split> {
        self.inner.as_split()
    }
}

impl<T: Instrument> Instrument for Timed<T> {
    fn process(&mut self, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        let start = Instant::now();
        self.inner.process(events, left, right);
        self.load.add(start.elapsed());
    }

    fn process_outputs(
        &mut self,
        events: &[Event],
        left: &mut [f32],
        right: &mut [f32],
        outputs: &mut Outputs,
    ) {
        let start = Instant::now();
        self.inner.process_outputs(events, left, right, outputs);
        self.load.add(start.elapsed());
    }

    fn latency(&self) -> u32 {
        self.inner.latency()
    }

    fn automate(&mut self, param: u32, value: f64) {
        self.inner.automate(param, value);
    }

    fn clear_automation(&mut self) {
        self.inner.clear_automation();
    }
}

/// The share of a core `slot`'s audio took lately, if measured yet.
pub(crate) fn share_of(slot: &FxSlot) -> Option<f32> {
    let remote = slot.plugin().is_some_and(|p| p.is_remote());
    slot.load.share(remote)
}

/// A share as shown: "2.4%".
pub(crate) fn percent(share: Option<f32>) -> String {
    match share {
        Some(s) if s < 0.001 => "0%".into(),
        Some(s) if s < 0.1 => format!("{:.1}%", s * 100.0),
        Some(s) => format!("{:.0}%", s * 100.0),
        None => "…".into(),
    }
}

/// The plugin windows' state.
pub(crate) struct PluginWindows {
    pub(crate) open: bool,
    /// Show the selected track's plugins as the selection changes.
    pub(crate) follow: bool,
    /// The track shown, by key (the master's is `master`).
    track: Option<String>,
    /// The tab chosen on each track, by the plugin's key.
    tabs: HashMap<String, String>,
    /// The list of the project's plugins is shown.
    pub(crate) list: bool,
    /// The selected track when last looked, to follow a new one.
    seen: Option<Option<String>>,
}

impl Default for PluginWindows {
    fn default() -> PluginWindows {
        PluginWindows {
            open: false,
            follow: true,
            track: None,
            tabs: HashMap::new(),
            list: false,
            seen: None,
        }
    }
}

/// A track's plugins as tabs: the instrument and layers first, then the
/// effects in order.
fn plugins(t: &Track) -> Vec<&FxSlot> {
    let inst = (t.instrument.iter()).filter_map(|i| i.plugin());
    let layers = (t.layers.iter()).filter_map(|l| l.slot.plugin());
    let fx = t.fx.iter().filter(|s| s.is_plugin());
    inst.chain(layers).chain(fx).collect()
}

fn plugins_mut(t: &mut Track) -> Vec<&mut FxSlot> {
    let inst = (t.instrument.iter_mut()).filter_map(|i| i.plugin_mut());
    let layers = (t.layers.iter_mut()).filter_map(|l| l.slot.plugin_mut());
    let fx = t.fx.iter_mut().filter(|s| s.is_plugin());
    inst.chain(layers).chain(fx).collect()
}

/// One plugin the project uses, for the list and the assistant.
struct Used {
    track_key: String,
    track: String,
    key: String,
    name: String,
    instrument: bool,
    runs: &'static str,
    runs_in: Option<&'static str>,
    share: Option<f32>,
    /// Playing, bypassed, stopped (and why) or not installed here.
    state: String,
}

impl Used {
    fn of(t: &Track, s: &FxSlot) -> Used {
        Used {
            track_key: t.key.clone(),
            track: t.name.clone(),
            key: s.key.clone(),
            name: s.name.clone(),
            instrument: s.instrument,
            runs: s.runs(),
            runs_in: Some(s.sandbox.key()),
            share: share_of(s),
            state: match (&s.crashed, s.bypass) {
                (Some(why), _) => format!("stopped: {why}"),
                (None, true) => "bypassed".into(),
                (None, false) => "playing".into(),
            },
        }
    }

    fn missing(t: &Track, e: &daw_project::Effect, instrument: bool) -> Used {
        Used {
            track_key: t.key.clone(),
            track: t.name.clone(),
            key: e.id.clone(),
            name: e.name.clone(),
            instrument,
            runs: "nowhere: not installed here",
            runs_in: None,
            share: None,
            state: "not installed here".into(),
        }
    }

    fn value(&self) -> Value {
        json!({
            "track": self.track_key, "track_name": self.track, "effect": self.key,
            "name": self.name, "kind": if self.instrument { "instrument" } else { "effect" },
            "runs_in": self.runs_in, "cpu_percent": self.share.map(|s| s * 100.0),
            "state": self.state,
        })
    }
}

/// What a tab click or the assistant asks for, done after drawing.
enum Ask {
    Track(Option<String>),
    Tab(String),
    OwnWindow(String, bool),
    Follow(bool),
    List(bool),
    Show(String, String),
}

impl App {
    /// The track the plugin windows show, by index (None: the master).
    fn plugin_track(&self) -> Option<Option<usize>> {
        match self.plugin_windows.track.as_deref() {
            Some(daw_project::Master::KEY) => Some(None),
            Some(k) => self.track_index(k).map(Some),
            None => None,
        }
    }

    fn plugin_track_ref(&self, at: Option<usize>) -> &Track {
        match at {
            Some(i) => &self.tracks[i],
            None => &self.master,
        }
    }

    fn plugin_track_mut(&mut self, at: Option<usize>) -> &mut Track {
        match at {
            Some(i) => &mut self.tracks[i],
            None => &mut self.master,
        }
    }

    /// The selected track, for following; a track gone is replaced.
    fn plugin_windows_follow(&mut self) {
        let sel = self.inspected().map(|i| self.tracks[i].key.clone());
        let gone = self.plugin_windows.track.is_some() && self.plugin_track().is_none();
        if gone {
            self.plugin_windows.track = None;
        }
        let new = self.plugin_windows.seen.as_ref() != Some(&sel);
        if new {
            self.plugin_windows.seen = Some(sel.clone());
        }
        let follow = self.plugin_windows.follow && new;
        if (follow || self.plugin_windows.track.is_none()) && sel.is_some() {
            self.show_plugin_track(sel);
        }
    }

    /// Shows another track's plugins. An own window open on the track
    /// shown before moves to the new track's chosen tab.
    fn show_plugin_track(&mut self, key: Option<String>) {
        if key == self.plugin_windows.track {
            return;
        }
        let mut moving = false;
        if let Some(at) = self.plugin_track() {
            for s in plugins_mut(self.plugin_track_mut(at)) {
                if s.editor_open() {
                    s.close_editor();
                    moving = true;
                }
            }
        }
        self.plugin_windows.track = key;
        if moving && let Some(k) = self.chosen_tab() {
            self.own_window(&k, true);
        }
    }

    /// The chosen tab on the track shown: the one chosen last, or its
    /// first plugin.
    fn chosen_tab(&self) -> Option<String> {
        let at = self.plugin_track()?;
        let t = self.plugin_track_ref(at);
        let list = plugins(t);
        let kept = self.plugin_windows.tabs.get(&t.key);
        (list.iter())
            .find(|s| Some(&s.key) == kept)
            .or(list.first())
            .map(|s| s.key.clone())
    }

    /// Chooses a tab. An own window open on the track moves to it.
    fn choose_tab(&mut self, key: &str) -> Result<(), String> {
        let at = self.plugin_track().ok_or("No track is shown.")?;
        let t = self.plugin_track_mut(at);
        let tk = t.key.clone();
        let mut list = plugins_mut(t);
        if !list.iter().any(|s| s.key == key) {
            return Err(format!("There is no plugin {key} on this track."));
        }
        let mut moving = false;
        for s in list.iter_mut().filter(|s| s.key != key) {
            if s.editor_open() {
                s.close_editor();
                moving = true;
            }
        }
        self.plugin_windows.tabs.insert(tk, key.to_string());
        if moving {
            self.own_window(key, true);
        }
        Ok(())
    }

    /// Opens or closes a plugin's own window, on the track shown.
    fn own_window(&mut self, key: &str, open: bool) -> Option<String> {
        let at = self.plugin_track()?;
        let mut list = plugins_mut(self.plugin_track_mut(at));
        let s = list.iter_mut().find(|s| s.key == key)?;
        if !open {
            s.close_editor();
            return None;
        }
        s.open_editor().err().map(|e| format!("{}: {e}", s.name))
    }

    fn step_tab(&mut self, by: isize) {
        self.plugin_windows.open = true;
        self.plugin_windows_follow();
        let Some(at) = self.plugin_track() else {
            self.message = Some("There is no track to show plugins of.".into());
            return;
        };
        let keys: Vec<String> = (plugins(self.plugin_track_ref(at)).iter())
            .map(|s| s.key.clone())
            .collect();
        if keys.is_empty() {
            self.message = Some("This track has no plugins.".into());
            return;
        }
        let now = self.chosen_tab();
        let i = keys
            .iter()
            .position(|k| Some(k) == now.as_ref())
            .unwrap_or(0) as isize;
        let next = (i + by).rem_euclid(keys.len() as isize) as usize;
        let _ = self.choose_tab(&keys[next].clone());
    }

    pub(crate) fn plugin_windows_command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::PluginWindows => self.plugin_windows.open = !self.plugin_windows.open,
            Cmd::PluginWindowsFollow => {
                self.plugin_windows.follow = !self.plugin_windows.follow;
                if self.plugin_windows.follow {
                    // Catch up with the selection now.
                    self.plugin_windows.seen = None;
                }
            }
            Cmd::NextPluginTab => self.step_tab(1),
            Cmd::PrevPluginTab => self.step_tab(-1),
            Cmd::ProjectPlugins => {
                self.plugin_windows.open = true;
                self.plugin_windows.list = true;
            }
            _ => {}
        }
    }

    /// Every plugin the project uses: the tracks' in order, then the
    /// master's, with those not installed here.
    fn project_plugins(&self) -> Vec<Used> {
        let mut out = Vec::new();
        for t in self.tracks.iter().chain(std::iter::once(&self.master)) {
            for s in plugins(t) {
                out.push(Used::of(t, s));
            }
            let missing_inst = (t.instrument.iter())
                .chain(t.layers.iter().map(|l| &l.slot))
                .filter_map(|i| i.missing());
            for e in missing_inst {
                out.push(Used::missing(t, e, true));
            }
            for (_, e) in self.proj.missing_fx.get(&t.key).into_iter().flatten() {
                if e.is_plugin() {
                    out.push(Used::missing(t, e, false));
                }
            }
        }
        out
    }

    /// The Plugin windows window.
    pub(crate) fn plugin_windows_window(&mut self, ctx: &egui::Context) {
        if !self.plugin_windows.open {
            return;
        }
        self.plugin_windows_follow();
        // The shares move: look again now and then.
        ctx.request_repaint_after(OVER);
        let mut open = true;
        let mut asks = Vec::new();
        let chosen = self.chosen_tab();
        let shown = self.plugin_track();
        let used = if self.plugin_windows.list {
            self.project_plugins()
        } else {
            Vec::new()
        };
        let mut choices: Vec<(String, String)> = (self.tracks.iter())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        choices.push((daw_project::Master::KEY.into(), "Master".into()));
        let pw = &self.plugin_windows;
        let (mut follow, mut list) = (pw.follow, pw.list);
        let title = |at: Option<usize>| match at {
            Some(i) => self.tracks[i].name.clone(),
            None => "Master".to_string(),
        };
        egui::Window::new("Plugin windows")
            .open(&mut open)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let now = shown.map(title).unwrap_or_else(|| "No track".into());
                    egui::ComboBox::from_id_salt("plugin-windows-track")
                        .selected_text(now)
                        .show_ui(ui, |ui| {
                            for (k, name) in &choices {
                                let on = pw.track.as_ref() == Some(k);
                                if ui
                                    // ai: set_plugin_windows.track
                                    .selectable_label(on, name)
                                    .clicked()
                                {
                                    asks.push(Ask::Track(Some(k.clone())));
                                }
                            }
                        })
                        .response
                        .accessible_name("Track whose plugins are shown");
                    if ui
                        // ai: set_plugin_windows.follow
                        .checkbox(&mut follow, "Follow the selected track")
                        .on_hover_text(Cmd::PluginWindowsFollow.info().hint_text())
                        .changed()
                    {
                        asks.push(Ask::Follow(follow));
                    }
                    if ui
                        // ai: set_plugin_windows.list
                        .toggle_value(&mut list, "Plugins in this project")
                        .on_hover_text(Cmd::ProjectPlugins.info().hint_text())
                        .changed()
                    {
                        asks.push(Ask::List(list));
                    }
                });
                ui.separator();
                let Some(at) = shown else {
                    ui.weak("Select a track to see its plugins.");
                    return;
                };
                let tabs = plugins(self.plugin_track_ref(at));
                if tabs.is_empty() {
                    ui.weak("This track has no plugins. Add one in its effects or instrument.");
                }
                ui.horizontal_wrapped(|ui| {
                    for s in &tabs {
                        let on = chosen.as_deref() == Some(s.key.as_str());
                        let text = format!("{}  {}", s.name, percent(share_of(s)));
                        if ui
                            // ai: set_plugin_windows.tab
                            .selectable_label(on, text)
                            .on_hover_text(format!("Show {}", s.name))
                            .clicked()
                        {
                            asks.push(Ask::Tab(s.key.clone()));
                        }
                    }
                });
                let Some(s) = tabs.iter().find(|s| chosen.as_deref() == Some(&s.key)) else {
                    return;
                };
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&s.name);
                    ui.weak(format!(
                        "{} · runs {} · {} of a processor core",
                        if s.instrument { "instrument" } else { "effect" },
                        s.runs(),
                        percent(share_of(s))
                    ));
                });
                if let Some(why) = &s.crashed {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Stopped: {why}. Start it again from the track's effects."),
                    );
                }
                let open = s.editor_open();
                ui.horizontal(|ui| {
                    if ui
                        // ai: set_plugin_windows.own_window
                        .button(if open {
                            "Close its own window"
                        } else {
                            "Open its own window"
                        })
                        .on_hover_text("The plugin's own window, as its makers drew it")
                        .clicked()
                    {
                        asks.push(Ask::OwnWindow(s.key.clone(), !open));
                    }
                });
                if let Some(p) = s.plugin() {
                    egui::ScrollArea::vertical()
                        .id_salt("plugin-windows-controls")
                        .max_height(320.0)
                        .show(ui, |ui| crate::fx::plugin_params(ui, s.id, p));
                }
            });
        if self.plugin_windows.list {
            let mut list_open = true;
            egui::Window::new("Plugins in this project")
                .open(&mut list_open)
                .default_width(520.0)
                .show(ctx, |ui| {
                    if used.is_empty() {
                        ui.weak("This project uses no plugins.");
                        return;
                    }
                    let total: f32 = used.iter().filter_map(|u| u.share).sum();
                    ui.weak(format!(
                        "{} plugins, together {} of a processor core.",
                        used.len(),
                        percent(Some(total))
                    ));
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            egui::Grid::new("project-plugins")
                                .striped(true)
                                .num_columns(5)
                                .show(ui, |ui| {
                                    ui.strong("Plugin");
                                    ui.strong("Track");
                                    ui.strong("Runs");
                                    ui.strong("Share");
                                    ui.strong("");
                                    ui.end_row();
                                    for u in &used {
                                        ui.label(&u.name);
                                        ui.label(&u.track);
                                        ui.weak(u.runs);
                                        ui.label(percent(u.share));
                                        if u.runs_in.is_some() {
                                            if ui
                                                // ai: set_plugin_windows.tab
                                                .button("Show")
                                                .on_hover_text(format!(
                                                    "Show {} in the plugin windows ({})",
                                                    u.name, u.state
                                                ))
                                                .clicked()
                                            {
                                                asks.push(Ask::Show(
                                                    u.track_key.clone(),
                                                    u.key.clone(),
                                                ));
                                            }
                                        } else {
                                            ui.weak(&u.state);
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                });
            if !list_open {
                asks.push(Ask::List(false));
            }
        }
        if !open {
            self.plugin_windows.open = false;
        }
        for a in asks {
            if let Err(e) = self.plugin_windows_ask(a) {
                self.message = Some(e);
            }
        }
    }

    fn plugin_windows_ask(&mut self, a: Ask) -> Result<(), String> {
        match a {
            Ask::Track(k) => {
                if let Some(k) = &k
                    && k != daw_project::Master::KEY
                    && self.track_index(k).is_none()
                {
                    return Err(format!("There is no track {k}."));
                }
                self.show_plugin_track(k);
            }
            Ask::Tab(k) => self.choose_tab(&k)?,
            Ask::OwnWindow(k, open) => {
                if let Some(e) = self.own_window(&k, open) {
                    return Err(e);
                }
            }
            Ask::Follow(on) => {
                self.plugin_windows.follow = on;
                if on {
                    self.plugin_windows.seen = None;
                }
            }
            Ask::List(on) => self.plugin_windows.list = on,
            Ask::Show(track, k) => {
                self.plugin_windows.open = true;
                self.show_plugin_track(Some(track));
                self.choose_tab(&k)?;
            }
        }
        Ok(())
    }

    fn plugin_windows_value(&mut self) -> Value {
        self.plugin_windows_follow();
        let chosen = self.chosen_tab();
        let shown = self.plugin_track();
        let tabs: Vec<Value> = match shown {
            Some(at) => (plugins(self.plugin_track_ref(at)).iter())
                .map(|s| {
                    json!({
                        "effect": s.key, "name": s.name,
                        "kind": if s.instrument { "instrument" } else { "effect" },
                        "chosen": chosen.as_deref() == Some(s.key.as_str()),
                        "own_window_open": s.editor_open(),
                        "runs_in": s.sandbox.key(),
                        "cpu_percent": share_of(s).map(|x| x * 100.0),
                        "stopped": s.crashed,
                    })
                })
                .collect(),
            None => Vec::new(),
        };
        let used = self.project_plugins();
        let total: f32 = used.iter().filter_map(|u| u.share).sum();
        let pw = &self.plugin_windows;
        json!({
            "open": pw.open,
            "follow": pw.follow,
            "list_open": pw.list,
            "track": shown.map(|at| {
                let t = self.plugin_track_ref(at);
                json!({"id": t.key, "name": t.name})
            }),
            "tabs": tabs,
            "project_plugins": used.iter().map(Used::value).collect::<Vec<_>>(),
            "total_cpu_percent": total * 100.0,
        })
    }

    pub(crate) fn plugin_windows_read(&mut self, name: &str) -> Option<Result<Value, String>> {
        (name == "get_plugin_windows").then(|| Ok(self.plugin_windows_value()))
    }

    pub(crate) fn plugin_windows_tool(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Option<Result<(Value, Option<String>), String>> {
        (name == "set_plugin_windows").then(|| self.set_plugin_windows(args))
    }

    fn set_plugin_windows(&mut self, args: &Value) -> Result<(Value, Option<String>), String> {
        if let Some(on) = args["open"].as_bool() {
            self.plugin_windows.open = on;
        }
        if let Some(on) = args["follow"].as_bool() {
            self.plugin_windows_ask(Ask::Follow(on))?;
        }
        if let Some(on) = args["list"].as_bool() {
            self.plugin_windows.list = on;
        }
        self.plugin_windows_follow();
        if let Some(k) = args["track"].as_str() {
            self.plugin_windows_ask(Ask::Track(Some(k.to_string())))?;
        }
        if let Some(k) = args["tab"].as_str() {
            self.choose_tab(k)?;
        }
        if let Some(open) = args["own_window"].as_bool() {
            let k = self.chosen_tab().ok_or("The track shown has no plugins.")?;
            if let Some(e) = self.own_window(&k, open) {
                return Err(e);
            }
        }
        Ok((self.plugin_windows_value(), None))
    }
}

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_plugin_windows",
        description: "The plugin windows (View → Plugin windows): whether open, whether they \
                      follow the selected track, the track shown, a tab for each of its \
                      plugins (which is chosen, whether its own window is open, where it \
                      runs, and the share of one processor core its audio took lately as \
                      cpu_percent, null until measured), and every plugin the project \
                      uses on any track or the master, with those not installed here.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "set_plugin_windows",
        description: "Opens or closes the plugin windows, sets whether they follow the \
                      selected track, shows a track's plugins (track: its id, or master), \
                      chooses a tab (tab: the plugin's effect id, as get_plugin_windows \
                      gives it), opens or closes the chosen plugin's own window \
                      (own_window), or shows the list of the project's plugins (list). \
                      Only changes what is shown; no undo step.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "open": {"type": "boolean"},
                "follow": {"type": "boolean"},
                "track": {"type": "string"},
                "tab": {"type": "string"},
                "own_window": {"type": "boolean"},
                "list": {"type": "boolean"}
            }})
        },
    },
];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::driver::Driver;
    use crate::freeze::test_plugin;

    #[test]
    fn a_share_is_the_time_taken_over_the_time_gone() {
        let l = Load::default();
        let t0 = Instant::now();
        assert_eq!(l.share_at(false, t0), None, "not measured yet");
        l.add(Duration::from_millis(50));
        assert_eq!(l.share_at(false, t0 + Duration::from_millis(100)), None);
        let s = l.share_at(false, t0 + Duration::from_secs(1)).unwrap();
        assert!((s - 0.05).abs() < 1e-4, "{s}");
        // A helper's count, made again, starts from 0.
        l.helper_busy(1_000_000_000);
        let r = Load::default();
        r.helper_busy(500_000_000);
        assert_eq!(r.share_at(true, t0), None);
        r.helper_busy(600_000_000);
        let s = r.share_at(true, t0 + Duration::from_secs(1)).unwrap();
        assert!((s - 0.1).abs() < 1e-4, "{s}");
        r.helper_busy(10);
        assert_eq!(r.share_at(true, t0 + Duration::from_secs(2)), Some(s));
        assert_eq!(percent(Some(0.0123)), "1.2%");
        assert_eq!(percent(Some(0.25)), "25%");
        assert_eq!(percent(None), "…");
    }

    #[test]
    fn a_timed_effect_counts_its_time_and_plays_the_same() {
        struct Slow;
        impl Insert for Slow {
            fn process(&mut self, left: &mut [f32], _right: &mut [f32]) {
                std::thread::sleep(Duration::from_millis(2));
                left[0] = 1.0;
            }
        }
        let load = Arc::new(Load::default());
        let mut t = Timed::new(Slow, load.clone());
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        t.process(&mut l, &mut r);
        assert_eq!(l[0], 1.0);
        assert!(load.busy.load(Ordering::Relaxed) >= 2_000_000);
    }

    fn with_plugins() -> Driver {
        let mut d = Driver::new();
        let app = d.app();
        app.effects.helper = Some(test_plugin::helper());
        let bundle = test_plugin::bundle();
        app.effects.list.bundles = vec![daw_plugins::scan::BundleEntry {
            result: Ok(daw_plugins::scan::scan_here(&bundle).unwrap()),
            path: bundle,
            modified: 0,
        }];
        d
    }

    fn track(d: &mut Driver) -> String {
        let v = d.call("add_track", json!({"kind": "audio"})).unwrap();
        v["track"].as_str().unwrap().to_string()
    }

    fn effect(d: &mut Driver, t: &str, e: &str) -> String {
        let v = d
            .call("add_effect", json!({"track": t, "effect": e}))
            .unwrap();
        v["effect"].as_str().unwrap().to_string()
    }

    #[test]
    fn an_assistant_reads_and_moves_the_plugin_windows() {
        let mut d = with_plugins();
        let a = track(&mut d);
        let b = track(&mut d);
        let g1 = effect(&mut d, &a, daw_test_plugin::NAME);
        effect(&mut d, &a, "eq");
        let g2 = effect(&mut d, &a, daw_test_plugin::NAME);
        let g3 = effect(&mut d, &b, daw_test_plugin::NAME);

        d.command("plugin-windows");
        d.call("select_track", json!({"track": a})).ok();
        let v = d
            .call("set_plugin_windows", json!({"track": a, "follow": false}))
            .unwrap();
        assert_eq!(v["open"], true);
        assert_eq!(v["track"]["id"], json!(a));
        // A tab per plugin; the built-in EQ is no plugin.
        let tabs = v["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2, "{v}");
        assert_eq!(
            (tabs[0]["effect"].clone(), tabs[0]["chosen"].clone()),
            (json!(g1), json!(true))
        );
        assert_eq!(v["project_plugins"].as_array().unwrap().len(), 3);
        assert!(v["total_cpu_percent"].is_number());

        // Tabs step round, by command and by the tool.
        d.command("next-plugin-tab");
        let v = d.call("get_plugin_windows", json!({})).unwrap();
        assert_eq!(v["tabs"][1]["chosen"], true);
        assert_eq!(v["tabs"][1]["effect"], json!(g2));
        d.command("next-plugin-tab");
        let v = d.call("get_plugin_windows", json!({})).unwrap();
        assert_eq!(v["tabs"][0]["chosen"], true, "round to the first");
        d.call("set_plugin_windows", json!({"tab": g2})).unwrap();
        assert!(
            d.call("set_plugin_windows", json!({"tab": g3}))
                .unwrap_err()
                .contains("no plugin")
        );

        // Another track; the tab chosen on each is kept.
        let v = d.call("set_plugin_windows", json!({"track": b})).unwrap();
        assert_eq!(v["tabs"][0]["effect"], json!(g3));
        let v = d.call("set_plugin_windows", json!({"track": a})).unwrap();
        assert_eq!(v["tabs"][1]["chosen"], true);
        assert!(
            d.call("set_plugin_windows", json!({"track": "nope"}))
                .is_err()
        );

        // The list of the project's plugins, by command.
        d.command("project-plugins");
        let v = d.call("get_plugin_windows", json!({})).unwrap();
        assert_eq!(v["list_open"], true);
        let used = v["project_plugins"].as_array().unwrap();
        assert!(used.iter().all(|u| u["state"] == "playing"), "{v}");
        assert_eq!(used[2]["track"], json!(b));
        d.command("plugin-windows");
        assert_eq!(
            d.call("get_plugin_windows", json!({})).unwrap()["open"],
            false
        );
    }

    #[test]
    fn the_plugin_windows_follow_the_selected_track() {
        let mut d = with_plugins();
        let a = track(&mut d);
        let b = track(&mut d);
        effect(&mut d, &a, daw_test_plugin::NAME);
        let gb = effect(&mut d, &b, daw_test_plugin::NAME);
        d.call("set_plugin_windows", json!({"open": true, "track": a}))
            .unwrap();
        // Selecting b shows b's plugins.
        assert!(d.app().inspect_track(&b));
        let v = d.call("get_plugin_windows", json!({})).unwrap();
        assert_eq!(v["follow"], true);
        assert_eq!(v["track"]["id"], json!(b));
        assert_eq!(v["tabs"][0]["effect"], json!(gb));
        // Not while it doesn't follow.
        d.command("plugin-windows-follow");
        assert!(d.app().inspect_track(&a));
        let v = d.call("get_plugin_windows", json!({})).unwrap();
        assert_eq!(
            (v["follow"].clone(), v["track"]["id"].clone()),
            (json!(false), json!(b))
        );
        // Back on, it catches up.
        d.command("plugin-windows-follow");
        let v = d.call("get_plugin_windows", json!({})).unwrap();
        assert_eq!(v["track"]["id"], json!(a));
    }
}
