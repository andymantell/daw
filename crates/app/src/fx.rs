// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin effects in the app (roadmap 1.3): the list of installed
//! plugins, scanning for them in the background, each track's insert
//! slots, and the windows that manage them.

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
    pub name: String,
    pub bundle: PathBuf,
    pub cid: vst3::Steinberg::TUID,
    pub bypass: bool,
    pub latency: u32,
    /// Show the host's own controls for its parameters.
    show_params: bool,
    plugin: Plugin,
}

impl FxSlot {
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
        if self.plugin.take_restart_request() {
            let latency = self.plugin.latency();
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
        let view = self
            .plugin
            .create_editor()
            .ok_or("this plugin has no editor of its own")?;
        self.editor = Some(daw_plugins::editor_win::EditorWindow::open(
            view, &self.name,
        )?);
        Ok(())
    }
}

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
        let module = self.module(&a.bundle)?;
        let (plugin, processor) =
            daw_plugins::instantiate(module, &a.class.cid, rate as f64, daw_audio_io::MAX_BLOCK)?;
        let id = self.next_id;
        self.next_id += 1;
        self.pending.push((id, Box::new(processor)));
        Ok(FxSlot {
            #[cfg(windows)]
            editor: None,
            id,
            name: a.class.name.clone(),
            bundle: a.bundle.clone(),
            cid: a.class.cid,
            bypass: false,
            latency: plugin.latency(),
            show_params: false,
            plugin,
        })
    }

    /// Makes `slot`'s plugin again for a new engine or sample rate,
    /// keeping its settings.
    pub fn reload(&mut self, slot: &mut FxSlot, rate: u32) -> Result<(), String> {
        let state = slot.plugin.save_state()?;
        #[cfg(windows)]
        let had_editor = slot.editor.take().is_some();
        let module = self.module(&slot.bundle)?;
        let (plugin, processor) =
            daw_plugins::instantiate(module, &slot.cid, rate as f64, daw_audio_io::MAX_BLOCK)?;
        plugin.load_state(&state)?;
        slot.latency = plugin.latency();
        slot.plugin = plugin;
        self.pending.push((slot.id, Box::new(processor)));
        #[cfg(windows)]
        if had_editor {
            let _ = slot.open_editor();
        }
        Ok(())
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

    /// The effects window for one track. Returns true if the chain
    /// changed and the graph needs rebuilding.
    pub fn track_window(
        &mut self,
        ctx: &egui::Context,
        track: &str,
        slots: &mut Vec<FxSlot>,
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
            .default_pos(egui::pos2(440.0, 110.0))
            .default_width(420.0)
            .show(ctx, |ui| {
                if slots.is_empty() {
                    ui.label("No effects on this track.");
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
                        if ui
                            .button("Edit")
                            .on_hover_text("Open the plugin's own window")
                            .clicked()
                            && let Err(e) = s.open_editor()
                        {
                            *message = Some(format!("{}: {e}", s.name));
                        }
                        ui.toggle_value(&mut s.show_params, "Controls")
                            .on_hover_text("Show the plugin's parameters here");
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
                    egui::ComboBox::from_id_salt("add-fx")
                        .selected_text("Add effect…")
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.filter).hint_text("Search"),
                            );
                            let f = self.filter.to_lowercase();
                            for p in effects.iter().filter(|p| {
                                f.is_empty() || p.class.name.to_lowercase().contains(&f)
                            }) {
                                if ui
                                    .selectable_label(false, &p.class.name)
                                    .on_hover_text(&p.class.vendor)
                                    .clicked()
                                {
                                    pick = Some(p.clone());
                                }
                            }
                            if effects.is_empty() {
                                ui.label("No effects found. See Settings → Plugins.");
                            }
                        });
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

/// The host's own controls for a plugin's parameters.
fn params(ui: &mut egui::Ui, s: &mut FxSlot) {
    const MOST: usize = 64;
    let list = s.plugin.params();
    if list.is_empty() {
        ui.label(RichText::new("No parameters.").weak());
        return;
    }
    egui::Grid::new(("params", s.id))
        .num_columns(3)
        .show(ui, |ui| {
            for p in list.iter().filter(|p| !p.bypass).take(MOST) {
                let Some(mut v) = s.plugin.param(p.id) else {
                    continue;
                };
                ui.label(&p.title);
                if ui
                    .add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    s.plugin.set_param(p.id, v);
                }
                let text = s.plugin.param_text(p.id, v).unwrap_or_default();
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
