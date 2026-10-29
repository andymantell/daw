// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects in the app: the list of installed plugins and scanning for
//! them in the background (roadmap 1.3), the effects that come with the
//! app (2.5), each track's insert slots, and the windows that manage
//! them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use daw_engine::graph::{Insert, InsertSpec};
use daw_plugins::scan::{self, Available, PluginList};
use daw_plugins::{Module, Plugin};
use eframe::egui::{self, Color32, RichText};

/// One effect in a track's chain.
pub struct FxSlot {
    // Declared first so it is dropped first: the editor must be closed
    // before its plugin.
    #[cfg(windows)]
    editor: Option<daw_plugins::editor_win::EditorWindow>,
    /// Matches the engine's insert id.
    pub id: u64,
    /// The effect's id in the project document.
    pub key: String,
    pub name: String,
    pub bypass: bool,
    pub latency: u32,
    /// Show the host's own controls for its parameters.
    show_params: bool,
    kind: SlotKind,
}

enum SlotKind {
    Plugin {
        bundle: PathBuf,
        cid: vst3::Steinberg::TUID,
        plugin: Plugin,
    },
    Builtin(daw_effects::Effect),
}

impl FxSlot {
    /// The plugin's own settings, for saving. None for a built-in
    /// effect, whose settings are always in the document.
    pub fn state(&self) -> Option<daw_project::EffectState> {
        let SlotKind::Plugin { plugin, .. } = &self.kind else {
            return None;
        };
        let s = plugin.save_state().ok()?;
        Some(daw_project::EffectState {
            component: s.component,
            controller: s.controller,
        })
    }

    /// The effect as the document has it, without a plugin's own
    /// settings (read only when saving).
    pub fn value(&self) -> daw_project::Effect {
        let mut e = daw_project::Effect {
            id: self.key.clone(),
            name: self.name.clone(),
            bypass: self.bypass,
            ..Default::default()
        };
        match &self.kind {
            SlotKind::Plugin { bundle, cid, .. } => {
                e.class = cid.iter().map(|&b| b as u8).collect();
                e.bundle = bundle.to_string_lossy().into_owned();
            }
            SlotKind::Builtin(b) => {
                e.builtin = b.kind().id().into();
                e.params = b.values();
                // In the order the document reads them back.
                e.params.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        e
    }

    /// Takes a built-in effect's settings from the document. Plugins'
    /// settings come back by making the plugin again.
    pub fn set_params(&mut self, params: &[(String, f64)]) {
        if let SlotKind::Builtin(b) = &self.kind {
            b.load(params);
        }
    }

    pub fn is_plugin(&self) -> bool {
        matches!(self.kind, SlotKind::Plugin { .. })
    }

    pub fn spec(&self) -> InsertSpec {
        InsertSpec {
            id: self.id,
            bypass: self.bypass,
            latency: self.latency,
        }
    }

    /// Checks on the plugin once a frame. True if the graph needs
    /// rebuilding (its latency changed).
    pub fn poll(&mut self) -> bool {
        #[cfg(windows)]
        if self.editor.as_ref().is_some_and(|e| !e.is_open()) {
            self.editor = None;
        }
        let SlotKind::Plugin { plugin, .. } = &self.kind else {
            return false;
        };
        if plugin.take_restart_request() {
            let latency = plugin.latency();
            if latency != self.latency {
                self.latency = latency;
                return true;
            }
        }
        false
    }

    #[cfg(windows)]
    fn open_editor(&mut self) -> Result<(), String> {
        if let Some(e) = &self.editor {
            e.focus();
            return Ok(());
        }
        let SlotKind::Plugin { plugin, .. } = &self.kind else {
            return Err("this effect has no window of its own".into());
        };
        let view = plugin
            .create_editor()
            .ok_or("this plugin has no editor of its own")?;
        self.editor = Some(daw_plugins::editor_win::EditorWindow::open(
            view, &self.name,
        )?);
        Ok(())
    }
}

/// Something that must outlive an offline render of an effect: the
/// plugin behind it, if it is one.
pub type Keep = Option<Plugin>;

enum ScanMsg {
    Bundle(PathBuf),
    Done(PluginList),
}

/// Everything plugin-related the app keeps.
pub struct Effects {
    pub list: PluginList,
    /// Where `list` is saved.
    list_path: PathBuf,
    /// Modules loaded so far, kept loaded for the life of the app.
    modules: HashMap<PathBuf, Arc<Module>>,
    /// Effects made but not yet in a graph, by insert id.
    pub pending: Vec<(u64, Box<dyn Insert>)>,
    next_id: u64,
    scan: Option<mpsc::Receiver<ScanMsg>>,
    /// The bundle being scanned now.
    scanning: Option<PathBuf>,
    pub show_manager: bool,
    /// The track whose effects window is open.
    pub open_track: Option<usize>,
    /// The effect being picked in the "Add" menu.
    filter: String,
}

impl Effects {
    pub fn new(list_path: PathBuf) -> Effects {
        let list = std::fs::read_to_string(&list_path)
            .map(|t| PluginList::from_text(&t))
            .unwrap_or_default();
        Effects {
            list,
            list_path,
            modules: HashMap::new(),
            pending: Vec::new(),
            next_id: 1,
            scan: None,
            scanning: None,
            show_manager: false,
            open_track: None,
            filter: String::new(),
        }
    }

    /// Scans the standard folders on a background thread; each bundle
    /// is opened by a copy of the app, so a bad plugin cannot crash it.
    pub fn start_scan(&mut self) {
        if self.scan.is_some() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut list = self.list.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let progress = tx.clone();
            list.rescan(
                &scan::default_dirs(),
                |b| scan::scan_in_child(&exe, b, scan::SCAN_TIMEOUT),
                |b| {
                    let _ = progress.send(ScanMsg::Bundle(b.to_owned()));
                },
            );
            let _ = tx.send(ScanMsg::Done(list));
        });
        self.scan = Some(rx);
    }

    pub fn scanning(&self) -> Option<String> {
        self.scan.as_ref()?;
        Some(match &self.scanning {
            Some(b) => format!(
                "Scanning plugins: {}",
                b.file_name().unwrap_or_default().to_string_lossy()
            ),
            None => "Looking for plugins…".into(),
        })
    }

    /// Takes in scan progress. Returns an error to show, if saving failed.
    pub fn poll_scan(&mut self) -> Option<String> {
        let rx = self.scan.as_ref()?;
        let mut done = None;
        let mut gone = false;
        loop {
            match rx.try_recv() {
                Ok(ScanMsg::Bundle(b)) => self.scanning = Some(b),
                Ok(ScanMsg::Done(list)) => done = Some(list),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
        if let Some(mut list) = done {
            // Keep what the user switched on or off while it ran.
            list.disabled = std::mem::take(&mut self.list.disabled);
            self.list = list;
            self.scan = None;
            self.scanning = None;
            return self
                .save()
                .err()
                .map(|e| format!("Could not save the plugin list: {e}"));
        }
        if gone {
            self.scan = None;
            self.scanning = None;
        }
        None
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.list_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.list_path.with_extension("tmp");
        std::fs::write(&tmp, self.list.to_text())?;
        std::fs::rename(tmp, &self.list_path)
    }

    fn module(&mut self, bundle: &Path) -> Result<Arc<Module>, String> {
        if let Some(m) = self.modules.get(bundle) {
            return Ok(m.clone());
        }
        let m = Arc::new(Module::load(bundle)?);
        self.modules.insert(bundle.to_owned(), m.clone());
        Ok(m)
    }

    /// Makes a new effect; its audio side waits in `pending` for the
    /// next graph.
    pub fn create(&mut self, a: &Available, rate: u32) -> Result<FxSlot, String> {
        self.make(&a.bundle, a.class.cid, &a.class.name, rate)
    }

    /// Makes a new built-in effect with its default settings.
    pub fn create_builtin(&mut self, kind: daw_effects::Kind, rate: u32) -> FxSlot {
        self.builtin(daw_effects::Effect::new(kind), rate)
    }

    fn builtin(&mut self, effect: daw_effects::Effect, rate: u32) -> FxSlot {
        let processor = effect.processor(rate);
        let id = self.next_id;
        self.next_id += 1;
        let latency = processor.latency();
        self.pending.push((id, Box::new(processor)));
        FxSlot {
            #[cfg(windows)]
            editor: None,
            id,
            key: daw_project::new_id(),
            name: effect.kind().name().to_string(),
            bypass: false,
            latency,
            show_params: true,
            kind: SlotKind::Builtin(effect),
        }
    }

    fn make(
        &mut self,
        bundle: &Path,
        cid: vst3::Steinberg::TUID,
        name: &str,
        rate: u32,
    ) -> Result<FxSlot, String> {
        let module = self.module(bundle)?;
        let (plugin, processor) =
            daw_plugins::instantiate(module, &cid, rate as f64, daw_audio_io::MAX_BLOCK)?;
        let id = self.next_id;
        self.next_id += 1;
        self.pending.push((id, Box::new(processor)));
        Ok(FxSlot {
            #[cfg(windows)]
            editor: None,
            id,
            key: daw_project::new_id(),
            name: name.to_string(),
            bypass: false,
            latency: plugin.latency(),
            show_params: false,
            kind: SlotKind::Plugin {
                bundle: bundle.to_path_buf(),
                cid,
                plugin,
            },
        })
    }

    /// Makes an effect saved in a project, with its settings. A plugin
    /// is found by its class id among those installed here, or else
    /// where it was when saved.
    pub fn restore(&mut self, e: &daw_project::Effect, rate: u32) -> Result<FxSlot, String> {
        if !e.is_plugin() {
            let kind = daw_effects::Kind::from_id(&e.builtin)
                .ok_or("it comes with a newer version of the app")?;
            let effect = daw_effects::Effect::new(kind);
            effect.load(&e.params);
            let mut slot = self.builtin(effect, rate);
            slot.key = e.id.clone();
            slot.bypass = e.bypass;
            slot.show_params = false;
            if !e.name.is_empty() {
                slot.name = e.name.clone();
            }
            return Ok(slot);
        }
        let mut cid: vst3::Steinberg::TUID = Default::default();
        if e.class.len() != cid.len() {
            return Err("its plugin id is damaged".into());
        }
        for (c, &b) in cid.iter_mut().zip(&e.class) {
            *c = b as _;
        }
        let bundle = self
            .list
            .plugins()
            .into_iter()
            .find(|a| a.class.cid == cid)
            .map(|a| a.bundle)
            .unwrap_or_else(|| PathBuf::from(&e.bundle));
        if !bundle.exists() {
            return Err("its plugin isn't installed on this computer".into());
        }
        let mut slot = self.make(&bundle, cid, &e.name, rate)?;
        slot.key = e.id.clone();
        slot.bypass = e.bypass;
        if let (Some(s), SlotKind::Plugin { plugin, .. }) = (&e.state, &slot.kind) {
            plugin.load_state(&daw_plugins::PluginState {
                component: s.component.clone(),
                controller: s.controller.clone(),
            })?;
            slot.latency = plugin.latency();
        }
        Ok(slot)
    }

    /// Makes `slot`'s effect again for a new engine or sample rate,
    /// keeping its settings.
    pub fn reload(&mut self, slot: &mut FxSlot, rate: u32) -> Result<(), String> {
        let module = match &slot.kind {
            SlotKind::Builtin(b) => {
                let processor = b.processor(rate);
                slot.latency = processor.latency();
                self.pending.push((slot.id, Box::new(processor)));
                return Ok(());
            }
            SlotKind::Plugin { bundle, .. } => self.module(&bundle.clone())?,
        };
        let SlotKind::Plugin { cid, plugin, .. } = &mut slot.kind else {
            unreachable!()
        };
        let state = plugin.save_state()?;
        #[cfg(windows)]
        let had_editor = slot.editor.take().is_some();
        let (fresh, processor) =
            daw_plugins::instantiate(module, cid, rate as f64, daw_audio_io::MAX_BLOCK)?;
        fresh.load_state(&state)?;
        slot.latency = fresh.latency();
        *plugin = fresh;
        self.pending.push((slot.id, Box::new(processor)));
        #[cfg(windows)]
        if had_editor {
            let _ = slot.open_editor();
        }
        Ok(())
    }

    /// A second instance of `slot`'s effect with the same settings, for
    /// rendering offline. Keep the [`Keep`] until rendering is done.
    pub fn fresh(&mut self, slot: &FxSlot, rate: u32) -> Result<(Keep, Box<dyn Insert>), String> {
        let (bundle, cid, plugin) = match &slot.kind {
            SlotKind::Builtin(b) => return Ok((None, Box::new(b.processor(rate)))),
            SlotKind::Plugin {
                bundle,
                cid,
                plugin,
            } => (bundle, cid, plugin),
        };
        let state = plugin.save_state()?;
        let module = self.module(bundle)?;
        let (plugin, processor) =
            daw_plugins::instantiate(module, cid, rate as f64, daw_audio_io::MAX_BLOCK)?;
        plugin.load_state(&state)?;
        Ok((Some(plugin), Box::new(processor)))
    }

    /// The window listing installed plugins.
    pub fn manager_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_manager;
        egui::Window::new("Plugins")
            .open(&mut open)
            .default_width(520.0)
            .max_width(640.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let busy = self.scan.is_some();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Rescan"))
                        .on_hover_text("Looks for new or updated plugins")
                        .clicked()
                    {
                        self.start_scan();
                    }
                    if let Some(s) = self.scanning() {
                        ui.spinner();
                        ui.label(s);
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "VST 3 folders: {}",
                        scan::default_dirs()
                            .iter()
                            .map(|d| d.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .small()
                    .weak(),
                );
                ui.separator();
                let plugins = self.list.plugins();
                if plugins.is_empty() && self.scan.is_none() {
                    ui.label("No VST 3 plugins found.");
                }
                let mut toggles = Vec::new();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        egui::Grid::new("plugin-list")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for p in &plugins {
                                    let mut on = p.enabled;
                                    if ui
                                        .checkbox(&mut on, "")
                                        .on_hover_text("Offer this plugin when adding effects")
                                        .changed()
                                    {
                                        toggles.push((p.class.cid, on));
                                    }
                                    ui.label(&p.class.name)
                                        .on_hover_text(p.bundle.display().to_string());
                                    ui.label(RichText::new(&p.class.vendor).weak());
                                    ui.label(
                                        RichText::new(if p.class.is_instrument() {
                                            "Instrument"
                                        } else {
                                            "Effect"
                                        })
                                        .weak(),
                                    );
                                    ui.end_row();
                                }
                            });
                        let failures: Vec<(PathBuf, String)> = self
                            .list
                            .failures()
                            .map(|(p, e)| (p.to_owned(), e.to_owned()))
                            .collect();
                        if !failures.is_empty() {
                            ui.add_space(8.0);
                            ui.strong("Could not be loaded");
                            for (p, e) in failures {
                                ui.horizontal(|ui| {
                                    ui.label(p.file_name().unwrap_or_default().to_string_lossy())
                                        .on_hover_text(p.display().to_string());
                                    if ui
                                        .add_enabled(
                                            self.scan.is_none(),
                                            egui::Button::new("Try again"),
                                        )
                                        .clicked()
                                    {
                                        self.list.forget(&p);
                                        self.start_scan();
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&e).color(Color32::LIGHT_RED),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&e);
                                });
                            }
                        }
                    });
                for (cid, on) in toggles {
                    self.list.set_enabled(&cid, on);
                    let _ = self.save();
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Instruments are listed but can't be used until MIDI tracks arrive.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.show_manager = open;
    }

    /// The effects window for one track, with the names of its effects
    /// whose plugins are missing here and a line about its frozen audio.
    /// Returns true if the chain changed and the graph needs rebuilding.
    #[allow(clippy::too_many_arguments)]
    pub fn track_window(
        &mut self,
        ctx: &egui::Context,
        track: &str,
        slots: &mut Vec<FxSlot>,
        missing: &[String],
        frozen: Option<&str>,
        rate: u32,
        message: &mut Option<String>,
    ) -> bool {
        let mut open = true;
        let mut changed = false;
        let mut remove = None;
        let mut swap = None;
        egui::Window::new(format!("Effects: {track}"))
            .id(egui::Id::unique("track-fx"))
            .open(&mut open)
            .default_pos(egui::pos2(300.0, 240.0))
            .default_width(420.0)
            .show(ctx, |ui| {
                if slots.is_empty() && missing.is_empty() {
                    ui.label("No effects on this track.");
                }
                for name in missing {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).strong().weak());
                        ui.label(RichText::new("not installed here, kept as it is").weak());
                    });
                    ui.separator();
                }
                let n = slots.len();
                for (i, s) in slots.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let mut on = !s.bypass;
                        if ui
                            .toggle_value(&mut on, "On")
                            .on_hover_text("Switch off to bypass this effect")
                            .changed()
                        {
                            s.bypass = !on;
                            changed = true;
                        }
                        ui.label(RichText::new(&s.name).strong());
                        if s.latency > 0 {
                            ui.label(RichText::new(format!("{} smp", s.latency)).weak())
                                .on_hover_text(
                                    "Delay this effect adds; other tracks are kept in time",
                                );
                        }
                        #[cfg(windows)]
                        if s.is_plugin()
                            && ui
                                .button("Edit")
                                .on_hover_text("Open the plugin's own window")
                                .clicked()
                            && let Err(e) = s.open_editor()
                        {
                            *message = Some(format!("{}: {e}", s.name));
                        }
                        ui.toggle_value(&mut s.show_params, "Controls")
                            .on_hover_text("Show the effect's settings here");
                        if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                            swap = Some((i - 1, i));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("▼")).clicked() {
                            swap = Some((i, i + 1));
                        }
                        if ui.button("✖").on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                    if s.show_params {
                        params(ui, s);
                    }
                    ui.separator();
                }
                let effects: Vec<Available> = self
                    .list
                    .plugins()
                    .into_iter()
                    .filter(|p| p.enabled && !p.class.is_instrument())
                    .collect();
                ui.horizontal(|ui| {
                    let mut pick = None;
                    let mut pick_builtin = None;
                    egui::ComboBox::from_id_salt("add-fx")
                        .selected_text("Add effect…")
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.filter).hint_text("Search"),
                            );
                            let f = self.filter.to_lowercase();
                            let fits =
                                |name: &str| f.is_empty() || name.to_lowercase().contains(&f);
                            ui.label(RichText::new("Built in").small().weak());
                            for k in daw_effects::Kind::ALL
                                .into_iter()
                                .filter(|k| fits(k.name()))
                            {
                                if ui
                                    .selectable_label(false, k.name())
                                    .on_hover_text(k.about())
                                    .clicked()
                                {
                                    pick_builtin = Some(k);
                                }
                            }
                            ui.separator();
                            ui.label(RichText::new("Plugins").small().weak());
                            for p in effects.iter().filter(|p| fits(&p.class.name)) {
                                if ui
                                    .selectable_label(false, &p.class.name)
                                    .on_hover_text(&p.class.vendor)
                                    .clicked()
                                {
                                    pick = Some(p.clone());
                                }
                            }
                            if effects.is_empty() {
                                ui.label(
                                    RichText::new("No plugins found. See Settings → Plugins.")
                                        .small()
                                        .weak(),
                                );
                            }
                        })
                        .response
                        .accessible_name("Add effect");
                    if let Some(k) = pick_builtin {
                        slots.push(self.create_builtin(k, rate));
                        changed = true;
                    }
                    if let Some(p) = pick {
                        match self.create(&p, rate) {
                            Ok(slot) => {
                                slots.push(slot);
                                changed = true;
                            }
                            Err(e) => *message = Some(format!("{}: {e}", p.class.name)),
                        }
                    }
                    if ui.button("Plugins…").clicked() {
                        self.show_manager = true;
                    }
                });
                if let Some(line) = frozen {
                    ui.add_space(4.0);
                    ui.label(RichText::new(line).weak());
                }
            });
        if let Some((a, b)) = swap {
            slots.swap(a, b);
            changed = true;
        }
        if let Some(i) = remove {
            // The audio side is dropped with the graph it was in.
            slots.remove(i);
            changed = true;
        }
        if !open {
            self.open_track = None;
        }
        changed
    }
}

/// The settings of an effect: a built-in effect's own, or the host's
/// controls for a plugin's parameters.
fn params(ui: &mut egui::Ui, s: &mut FxSlot) {
    match &s.kind {
        SlotKind::Builtin(b) => builtin_params(ui, s.id, b),
        SlotKind::Plugin { plugin, .. } => plugin_params(ui, s.id, plugin),
    }
}

fn builtin_params(ui: &mut egui::Ui, id: u64, b: &daw_effects::Effect) {
    let kind = b.kind();
    egui::Grid::new(("builtin", id))
        .num_columns(2)
        .show(ui, |ui| {
            for (i, p) in kind.params().iter().enumerate() {
                let mut v = b.get(i);
                ui.label(p.name);
                let changed = if p.unit == daw_effects::Unit::Switch {
                    let mut on = v >= 0.5;
                    let r = ui.checkbox(&mut on, "").accessible_name(p.name);
                    v = if on { 1.0 } else { 0.0 };
                    r.changed()
                } else if p.unit == daw_effects::Unit::Choice {
                    let mut pick = v as usize;
                    let r = egui::ComboBox::from_id_salt(("choice", id, i))
                        .selected_text(p.text(v))
                        .show_index(ui, &mut pick, p.choices.len(), |c| p.choices[c]);
                    v = pick as f64;
                    r.changed()
                } else {
                    let text = |x: f64, _| p.text(x);
                    ui.add(
                        egui::Slider::new(&mut v, p.min..=p.max)
                            .logarithmic(p.log)
                            .custom_formatter(text)
                            .min_decimals(1),
                    )
                    .accessible_name(p.name)
                    .changed()
                };
                if changed {
                    b.set(i, v);
                }
                ui.end_row();
            }
            if kind.has_meter() {
                let db = b.reduction_db();
                ui.label("Turning down");
                let amount = (-db / 24.0).clamp(0.0, 1.0);
                ui.add(
                    egui::ProgressBar::new(amount)
                        .desired_width(160.0)
                        .text(format!("{:.1} dB", (-db).max(0.0))),
                )
                .on_hover_text("How far the level is being turned down now");
                ui.end_row();
                // The meter moves with the music.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
            }
        });
}

fn plugin_params(ui: &mut egui::Ui, id: u64, plugin: &Plugin) {
    const MOST: usize = 64;
    let list = plugin.params();
    if list.is_empty() {
        ui.label(RichText::new("No parameters.").weak());
        return;
    }
    egui::Grid::new(("params", id))
        .num_columns(3)
        .show(ui, |ui| {
            for p in list.iter().filter(|p| !p.bypass).take(MOST) {
                let Some(mut v) = plugin.param(p.id) else {
                    continue;
                };
                ui.label(&p.title);
                if ui
                    .add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    plugin.set_param(p.id, v);
                }
                let text = plugin.param_text(p.id, v).unwrap_or_default();
                ui.label(format!("{text} {}", p.units).trim().to_string());
                ui.end_row();
            }
        });
    if list.len() > MOST {
        ui.label(
            RichText::new(format!(
                "Showing the first {MOST} of {} parameters.",
                list.len()
            ))
            .small()
            .weak(),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use daw_effects::Kind;
    use daw_engine::engine::render_with;
    use daw_engine::time::Frames;
    use daw_project::{ProjectDoc, ProjectFolder};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::SlotKind;
    use crate::App;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-builtin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    fn until(h: &mut Harness<'_, App>, what: impl Fn(&App) -> bool) {
        for _ in 0..1000 {
            h.step();
            if what(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    /// A second of a tone, mono, at `rate`.
    fn tone(dir: &Path, rate: u32) -> PathBuf {
        let path = dir.join("guitar.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate {
            let x = (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin();
            w.write_sample((x * 16_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    fn render(app: &mut App, frames: usize) -> Vec<f32> {
        let s = app.session();
        let mut keep = Vec::new();
        let mut fx = Vec::new();
        for slot in app.tracks.iter().flat_map(|t| &t.fx) {
            let (p, proc) = app.effects.fresh(slot, app.rate).unwrap();
            keep.extend(p);
            fx.push((slot.id, proc));
        }
        let g = s.compile_with(daw_audio_io::MAX_BLOCK, &mut fx).unwrap();
        let mut out = Vec::new();
        render_with(g, Frames(0), frames, 256, |l, r| {
            for (a, b) in l.iter().zip(r) {
                out.extend([*a, *b]);
            }
            true
        });
        out
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, v| m.max(v.abs()))
    }

    fn builtin(app: &App, i: usize) -> &daw_effects::Effect {
        match &app.tracks[0].fx[i].kind {
            SlotKind::Builtin(b) => b,
            SlotKind::Plugin { .. } => panic!("a plugin"),
        }
    }

    #[test]
    fn a_built_in_effect_is_picked_heard_saved_undone_and_reopened() {
        let dir = scratch("eq");
        let mut h = harness();
        let rate = h.state().rate;
        h.state_mut().add_file(&tone(&dir, rate));
        until(&mut h, |a| !a.tracks.is_empty());
        let frames = rate as usize;

        // Picked from the track's effects list, beside plugins.
        h.state_mut().effects.open_track = Some(0);
        h.run();
        h.get_by_label("Add effect").click();
        h.run();
        h.get_by_label("EQ").click();
        h.run();
        assert_eq!(h.state().tracks[0].fx.len(), 1);
        assert_eq!(h.state().tracks[0].fx[0].name, "EQ");
        // Its settings show straight away.
        assert!(h.query_all_by_label("Output").next().is_some());

        // Heard: turning its output down 6 dB halves the level.
        let dry = peak(&render(h.state_mut(), frames));
        builtin(h.state(), 0).set(12, -6.0);
        h.run();
        let wet = peak(&render(h.state_mut(), frames));
        assert!((wet / dry - 0.501).abs() < 0.01, "{dry} {wet}");

        // Saved in the project, with no frozen audio: every computer has it.
        let song = dir.join("Song");
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &song).unwrap();
        h.run();
        assert!(!h.state().freezer.busy());
        let (_, bytes) = ProjectFolder::open(&song).unwrap();
        let saved = ProjectDoc::from_bytes(&bytes, 1).unwrap().get();
        let e = &saved.tracks[0].effects[0];
        assert_eq!(e.builtin, "eq");
        assert!(e.params.contains(&("output".into(), -6.0)));
        assert!(saved.tracks[0].frozen.is_none());
        assert!(
            h.state().frozen_note(&h.state().tracks[0]).0.is_none(),
            "no line about frozen audio"
        );

        // A change of setting is one undo step.
        builtin(h.state(), 0).set(1, 4.0);
        h.run();
        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        assert_eq!(builtin(h.state(), 0).get(1), 0.0);
        assert_eq!(builtin(h.state(), 0).get(12), -6.0);
        h.state_mut().save(&ctx).unwrap();
        drop(h);

        // Reopened: the same effect and settings, heard the same.
        let mut h = harness();
        h.state_mut().open_project(song.clone());
        until(&mut h, |a| !a.proj.loading());
        h.run();
        assert!(h.state().proj.missing_fx.is_empty());
        assert_eq!(h.state().tracks[0].fx.len(), 1);
        assert_eq!(builtin(h.state(), 0).get(12), -6.0);
        let again = peak(&render(h.state_mut(), frames));
        assert!((again - wet).abs() < 1e-4);
    }

    #[test]
    fn an_effect_from_a_newer_version_is_kept_as_it_is() {
        let mut h = harness();
        let e = daw_project::Effect {
            id: "e1".into(),
            name: "Tape echo".into(),
            builtin: "tape-echo".into(),
            params: vec![("wow".into(), 0.5)],
            ..Default::default()
        };
        let rate = h.state().rate;
        let err = h.state_mut().effects.restore(&e, rate).err().unwrap();
        assert!(err.contains("newer version"), "{err}");
        // Each kind made from its saved form comes back as it was.
        for k in Kind::ALL {
            let app = h.state_mut();
            let slot = app.effects.create_builtin(k, rate);
            let saved = slot.value();
            let back = app.effects.restore(&saved, rate).unwrap();
            assert_eq!(back.value(), saved);
        }
    }
}
