// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects in the app: the list of installed plugins and scanning for
//! them in the background (roadmap 1.3), the effects that come with the
//! app (2.5), each track's insert slots, and the windows that manage
//! them. Each plugin runs where the user chose for it (2.4): in the
//! app, in a helper process shared with others, or in one of its own,
//! so that a crash takes down the helper and not the app. Presets are
//! saved and loaded from the effects window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use daw_engine::graph::{Insert, InsertSpec};
use daw_plugins::presets::{self, Preset};
use daw_plugins::proto::ProgramList;
use daw_plugins::scan::{self, Available, PluginList};
use daw_plugins::{Host, Module, Plugin, PluginState, Processor, Sandbox};
use eframe::egui::{self, Color32, RichText};

/// One effect in a track's chain.
pub struct FxSlot {
    /// Matches the engine's insert id.
    pub id: u64,
    /// The effect's id in the project document.
    pub key: String,
    pub name: String,
    pub bypass: bool,
    pub latency: u32,
    /// Where it runs (in the app, for a built-in effect).
    pub sandbox: Sandbox,
    /// Why it stopped working: its helper process crashed or hung. The
    /// audio passes through it unchanged until it is started again.
    pub crashed: Option<String>,
    /// The settings last saved or loaded, to start again from after a
    /// crash.
    last_state: Option<PluginState>,
    /// Show the host's own controls for its parameters.
    show_params: bool,
    /// The name typed for a new preset.
    preset_name: String,
    /// The presets and program lists found, until something changes.
    presets: Option<(Vec<Preset>, Vec<ProgramList>)>,
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
    pub fn state(&mut self) -> Option<daw_project::EffectState> {
        let SlotKind::Plugin { plugin, .. } = &self.kind else {
            return None;
        };
        let s = match plugin.save_state() {
            Ok(s) => s,
            // A crashed plugin keeps the settings it last had.
            Err(_) => self.last_state.clone()?,
        };
        self.last_state = Some(s.clone());
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
                e.response = b.response_ref();
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

    /// A built-in reverb's response, as the project saves it.
    pub fn response_ref(&self) -> String {
        match &self.kind {
            SlotKind::Builtin(b) => b.response_ref(),
            SlotKind::Plugin { .. } => String::new(),
        }
    }

    /// Renames the response file it uses, once the file has come into
    /// the project, keeping what was read from it.
    pub fn rename_response(&self, name: &str) {
        if let SlotKind::Builtin(b) = &self.kind {
            b.rename_response(name.to_string());
        }
    }

    pub fn is_plugin(&self) -> bool {
        matches!(self.kind, SlotKind::Plugin { .. })
    }

    /// The plugin behind this effect, if it is one.
    fn plugin(&self) -> Option<&Plugin> {
        match &self.kind {
            SlotKind::Plugin { plugin, .. } => Some(plugin),
            SlotKind::Builtin(_) => None,
        }
    }

    fn plugin_mut(&mut self) -> Option<&mut Plugin> {
        match &mut self.kind {
            SlotKind::Plugin { plugin, .. } => Some(plugin),
            SlotKind::Builtin(_) => None,
        }
    }

    /// The plugin's class id, if it is one.
    fn cid(&self) -> Option<&vst3::Steinberg::TUID> {
        match &self.kind {
            SlotKind::Plugin { cid, .. } => Some(cid),
            SlotKind::Builtin(_) => None,
        }
    }

    /// Where the effect runs, for the effects window.
    pub fn runs(&self) -> &'static str {
        match self.sandbox {
            Sandbox::InApp => "in the app",
            Sandbox::Shared => "in the shared helper",
            Sandbox::Own => "in its own process",
        }
    }

    /// Blocks the helper was late with, played as silence.
    pub fn late(&self) -> u64 {
        match self.plugin() {
            Some(Plugin::Remote(p)) => p.late(),
            _ => 0,
        }
    }

    pub fn spec(&self) -> InsertSpec {
        InsertSpec {
            id: self.id,
            bypass: self.bypass,
            latency: self.latency,
        }
    }

    /// Checks on the plugin once a frame. True if the graph needs
    /// rebuilding (its latency changed, or it just crashed and should
    /// pass audio through).
    pub fn poll(&mut self) -> bool {
        if self.crashed.is_some() {
            return false;
        }
        let SlotKind::Plugin { plugin, .. } = &mut self.kind else {
            return false;
        };
        if let Some(why) = plugin.failure() {
            self.crashed = Some(why);
            return true;
        }
        let status = plugin.poll();
        if status.latency != self.latency {
            self.latency = status.latency;
            return true;
        }
        false
    }

    fn open_editor(&mut self) -> Result<(), String> {
        if let Some(why) = &self.crashed {
            return Err(why.clone());
        }
        let title = self.name.clone();
        let SlotKind::Plugin { plugin, .. } = &mut self.kind else {
            return Err("this effect has no window of its own".into());
        };
        plugin.open_editor(&title)
    }

    fn load(&mut self, state: &PluginState) -> Result<(), String> {
        let SlotKind::Plugin { plugin, .. } = &self.kind else {
            return Err("this effect has no settings to load".into());
        };
        plugin.load_state(state)?;
        self.last_state = Some(state.clone());
        self.latency = plugin.latency();
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
    /// Where the user's presets are saved.
    pub(crate) presets_dir: PathBuf,
    /// The program run as a helper process: the app itself. None where
    /// there is none (in tests), so every plugin runs in the app.
    pub helper: Option<PathBuf>,
    /// The helper shared by plugins set to share one, while it lives.
    shared: Option<Host>,
    /// Frames per block the audio device gives, so a live helper
    /// processor can be set up to run one such block behind.
    pub block: usize,
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
    /// The project's folder, where reverb response files are kept once
    /// saved.
    pub folder: Option<PathBuf>,
    /// Responses read so far, by reference.
    responses: HashMap<String, Arc<daw_effects::Response>>,
    /// The list of recorded rooms' credits is open.
    pub show_credits: bool,
}

/// A room picked for a convolution reverb.
enum RoomPick {
    /// One made here, by index.
    Made(usize),
    Recorded(&'static daw_effects::library::Room),
    /// A sound file, to be chosen.
    File,
}

impl Effects {
    pub fn new(list_path: PathBuf) -> Effects {
        let list = std::fs::read_to_string(&list_path)
            .map(|t| PluginList::from_text(&t))
            .unwrap_or_default();
        Effects {
            list,
            presets_dir: list_path.with_file_name("presets"),
            list_path,
            helper: (!cfg!(test))
                .then(|| std::env::current_exe().ok())
                .flatten(),
            shared: None,
            block: 256,
            modules: HashMap::new(),
            pending: Vec::new(),
            next_id: 1,
            scan: None,
            scanning: None,
            show_manager: false,
            open_track: None,
            filter: String::new(),
            folder: None,
            responses: HashMap::new(),
            show_credits: false,
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

    /// The shared helper, started if there is none or the last one
    /// died.
    fn shared_host(&mut self) -> Result<Host, String> {
        if let Some(h) = &self.shared
            && h.failure().is_none()
        {
            return Ok(h.clone());
        }
        let exe = self.helper.as_ref().ok_or("no plugin helper here")?;
        let h = Host::spawn(exe)?;
        self.shared = Some(h.clone());
        Ok(h)
    }

    /// Makes the two sides of a plugin where `sandbox` says, live (one
    /// block behind in a helper) or for rendering (waiting on it).
    fn sides(
        &mut self,
        bundle: &Path,
        cid: &vst3::Steinberg::TUID,
        rate: u32,
        sandbox: Sandbox,
        live: bool,
    ) -> Result<(Plugin, Processor), String> {
        let sandbox = if self.helper.is_none() {
            Sandbox::InApp
        } else {
            sandbox
        };
        let block = live.then_some(self.block);
        match sandbox {
            Sandbox::InApp => {
                let module = self.module(bundle)?;
                let (p, x) =
                    daw_plugins::instantiate(module, cid, rate as f64, daw_audio_io::MAX_BLOCK)?;
                Ok((Plugin::Local(p), Processor::Local(x)))
            }
            Sandbox::Shared => {
                let host = self.shared_host()?;
                let (p, x) =
                    host.instantiate(bundle, cid, rate as f64, daw_audio_io::MAX_BLOCK, block)?;
                Ok((Plugin::Remote(p), Processor::Remote(x)))
            }
            Sandbox::Own => {
                let exe = self.helper.as_ref().ok_or("no plugin helper here")?;
                let host = Host::spawn(exe)?;
                let (p, x) =
                    host.instantiate(bundle, cid, rate as f64, daw_audio_io::MAX_BLOCK, block)?;
                Ok((Plugin::Remote(p), Processor::Remote(x)))
            }
        }
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
            id,
            key: daw_project::new_id(),
            name: effect.kind().name().to_string(),
            bypass: false,
            latency,
            sandbox: Sandbox::InApp,
            crashed: None,
            last_state: None,
            show_params: true,
            preset_name: String::new(),
            presets: None,
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
        let sandbox = self.list.sandbox_of(&cid);
        let (plugin, processor) = self.sides(bundle, &cid, rate, sandbox, true)?;
        let id = self.next_id;
        self.next_id += 1;
        self.pending.push((id, Box::new(processor)));
        Ok(FxSlot {
            id,
            key: daw_project::new_id(),
            name: name.to_string(),
            bypass: false,
            latency: plugin.latency(),
            sandbox: if plugin.is_remote() {
                sandbox
            } else {
                Sandbox::InApp
            },
            crashed: None,
            last_state: None,
            show_params: false,
            preset_name: String::new(),
            presets: None,
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
            if !e.response.is_empty() {
                // A file that can't be read is kept in the project; one of
                // the rooms made here plays meanwhile.
                let read = self.read_response(&e.response).ok();
                effect.set_response(e.response.clone(), read);
            }
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
        if let Some(s) = &e.state {
            slot.load(&PluginState {
                component: s.component.clone(),
                controller: s.controller.clone(),
            })?;
        }
        Ok(slot)
    }

    /// The response `reference` names, read (once) from the rooms that
    /// come with the app or a sound file.
    fn read_response(&mut self, reference: &str) -> Result<Arc<daw_effects::Response>, String> {
        if let Some(r) = self.responses.get(reference) {
            return Ok(r.clone());
        }
        let r = match daw_effects::library::find(reference) {
            Some(room) => room.response()?,
            None if reference.starts_with(daw_effects::library::PREFIX) => {
                return Err("that recorded room comes with a newer version of the app".into());
            }
            None => {
                let path = match &self.folder {
                    Some(dir)
                        if daw_project::ProjectFolder { dir: dir.clone() }.has_asset(reference) =>
                    {
                        daw_project::ProjectFolder { dir: dir.clone() }.asset(reference)
                    }
                    _ => PathBuf::from(reference),
                };
                daw_effects::Response::from_file(&path)?
            }
        };
        let r = Arc::new(r);
        self.responses.insert(reference.to_string(), r.clone());
        Ok(r)
    }

    /// Knows the response read from `old` by its new name.
    pub fn remember_as(&mut self, old: &str, new: &str) {
        if let Some(r) = self.responses.get(old).cloned() {
            self.responses.insert(new.to_string(), r);
        }
    }

    /// Points a built-in reverb at the response `reference` names (empty
    /// for its own rooms), making its audio side again. A file that can't
    /// be read is still kept, and the error returned.
    pub fn set_response(
        &mut self,
        slot: &mut FxSlot,
        reference: &str,
        rate: u32,
    ) -> Result<(), String> {
        let SlotKind::Builtin(b) = &slot.kind else {
            return Ok(());
        };
        let read = if reference.is_empty() {
            Ok(None)
        } else {
            self.read_response(reference).map(Some)
        };
        let (response, result) = match read {
            Ok(r) => (r, Ok(())),
            Err(e) => (None, Err(e)),
        };
        b.set_response(reference.to_string(), response);
        self.reload(slot, rate)?;
        result
    }

    /// Makes `slot`'s effect again for a new engine or sample rate, or
    /// after a crash, keeping its settings (the last saved ones, after
    /// a crash) and where the user now wants it to run.
    pub fn reload(&mut self, slot: &mut FxSlot, rate: u32) -> Result<(), String> {
        let (bundle, cid) = match &slot.kind {
            SlotKind::Builtin(b) => {
                let processor = b.processor(rate);
                slot.latency = processor.latency();
                self.pending.push((slot.id, Box::new(processor)));
                return Ok(());
            }
            SlotKind::Plugin { bundle, cid, .. } => (bundle.clone(), *cid),
        };
        let state = match &slot.kind {
            SlotKind::Plugin { plugin, .. } => match plugin.save_state() {
                Ok(s) => Some(s),
                Err(_) => slot.last_state.clone(),
            },
            SlotKind::Builtin(_) => unreachable!(),
        };
        let had_editor = slot.plugin().is_some_and(|p| p.editor_open());
        if let Some(p) = slot.plugin_mut() {
            p.close_editor();
        }
        let sandbox = self.list.sandbox_of(&cid);
        let (fresh, processor) = self.sides(&bundle, &cid, rate, sandbox, true)?;
        slot.sandbox = if fresh.is_remote() {
            sandbox
        } else {
            Sandbox::InApp
        };
        slot.latency = fresh.latency();
        let SlotKind::Plugin { plugin, .. } = &mut slot.kind else {
            unreachable!()
        };
        *plugin = fresh;
        slot.crashed = None;
        if let Some(s) = &state {
            slot.load(s)?;
        }
        self.pending.push((slot.id, Box::new(processor)));
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
        let state = plugin.save_state().ok().or_else(|| slot.last_state.clone());
        let sandbox = self.list.sandbox_of(cid);
        let (plugin, processor) = self.sides(bundle, cid, rate, sandbox, false)?;
        if let Some(s) = &state {
            plugin.load_state(s)?;
        }
        Ok((Some(plugin), Box::new(processor)))
    }

    /// The credits of the recorded rooms that come with the app.
    pub fn credits_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_credits;
        egui::Window::new("Sound credits")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(
                    "The Room reverb's recorded spaces were recorded by these people, who share \
                     them under the licences shown.",
                );
                ui.separator();
                let rooms = daw_effects::library::ROOMS;
                if rooms.is_empty() {
                    ui.label(
                        RichText::new("No recorded spaces come with this version yet.").weak(),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for room in rooms {
                            ui.strong(room.name);
                            ui.label(room.credit());
                            ui.hyperlink_to(room.licence.name(), room.licence.url());
                            ui.add_space(6.0);
                        }
                    });
            });
        self.show_credits = open;
    }

    /// The window listing installed plugins.
    pub fn manager_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_manager;
        egui::Window::new("Plugins")
            .open(&mut open)
            .default_width(700.0)
            .max_width(820.0)
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
                let mut sandboxes = Vec::new();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        egui::Grid::new("plugin-list")
                            .num_columns(5)
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
                                    let mut sb = p.sandbox;
                                    egui::ComboBox::from_id_salt((
                                        "sandbox",
                                        &p.bundle,
                                        p.class.cid,
                                    ))
                                    .selected_text(sb.label())
                                    .width(170.0)
                                    .show_ui(ui, |ui| {
                                        for choice in Sandbox::ALL {
                                            ui.selectable_value(&mut sb, choice, choice.label());
                                        }
                                    })
                                    .response
                                    .accessible_name(format!("Where {} runs", p.class.name))
                                    .on_hover_text(
                                        "In a helper process a crash can't take the app \
                                             down, at the cost of one block of extra delay. \
                                             Applies to effects added from now on, or when \
                                             an effect is restarted.",
                                    );
                                    if sb != p.sandbox {
                                        sandboxes.push((p.class.cid, sb));
                                    }
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
                for (cid, sb) in sandboxes {
                    self.list.set_sandbox(&cid, sb);
                    let _ = self.save();
                }
                if self.helper.is_none() {
                    ui.label(
                        RichText::new("No helper process here: every plugin runs in the app.")
                            .small()
                            .weak(),
                    );
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
        let mut restart = None;
        let mut picks = Vec::new();
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
                        ui.label(RichText::new(&s.name).strong())
                            .on_hover_text(format!("Runs {}", s.runs()));
                        if s.latency > 0 {
                            ui.label(RichText::new(format!("{} smp", s.latency)).weak())
                                .on_hover_text(
                                    "Delay this effect adds (counting its helper process, \
                                     if any); other tracks are kept in time",
                                );
                        }
                        let late = s.late();
                        if late > 0 {
                            ui.label(RichText::new(format!("late ×{late}")).weak())
                                .on_hover_text(
                                    "Blocks its helper process was late with, played as \
                                     silence. A bigger audio buffer gives it more time.",
                                );
                        }
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
                        if s.is_plugin() {
                            ui.menu_button("Presets", |ui| {
                                if let Some(e) = presets_menu(ui, s, &self.presets_dir) {
                                    *message = Some(format!("{}: {e}", s.name));
                                }
                            })
                            .response
                            .accessible_name(format!("Presets for {}", s.name));
                        }
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
                    if let Some(why) = &s.crashed {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Stopped: {why}. Its audio passes through."))
                                    .color(Color32::LIGHT_RED),
                            );
                            if ui
                                .button("Restart")
                                .on_hover_text(
                                    "Start the plugin again with its last saved settings",
                                )
                                .clicked()
                            {
                                restart = Some(i);
                            }
                        });
                    }
                    if s.show_params
                        && s.crashed.is_none()
                        && let Some(pick) = params(ui, s)
                    {
                        picks.push((i, pick));
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
        for (i, pick) in picks {
            let reference = match pick {
                RoomPick::Made(_) => String::new(),
                RoomPick::Recorded(room) => daw_effects::library::reference(room),
                RoomPick::File => {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title("Choose a room's impulse response")
                        .add_filter("Sound files", &["wav", "flac", "aif", "aiff", "mp3", "ogg"])
                        .pick_file()
                    else {
                        continue;
                    };
                    path.to_string_lossy().into_owned()
                }
            };
            if let Err(e) = self.set_response(&mut slots[i], &reference, rate) {
                *message = Some(format!("Couldn't use that response: {e}"));
            }
            changed = true;
        }
        if let Some((a, b)) = swap {
            slots.swap(a, b);
            changed = true;
        }
        if let Some(i) = restart {
            match self.reload(&mut slots[i], rate) {
                Ok(()) => changed = true,
                Err(e) => *message = Some(format!("{}: {e}", slots[i].name)),
            }
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
fn params(ui: &mut egui::Ui, s: &mut FxSlot) -> Option<RoomPick> {
    match &s.kind {
        SlotKind::Builtin(b) => builtin_params(ui, s.id, b),
        SlotKind::Plugin { plugin, .. } => {
            plugin_params(ui, s.id, plugin);
            None
        }
    }
}

/// The room list of a convolution reverb: the rooms made here, the
/// recorded rooms that come with the app, and a file of the user's own.
fn room_picker(ui: &mut egui::Ui, id: u64, b: &daw_effects::Effect) -> Option<RoomPick> {
    use daw_effects::library;
    let reference = b.response_ref();
    let made = daw_effects::Kind::Convolution.params()[0];
    let now = if reference.is_empty() {
        made.text(b.get(0))
    } else if let Some(room) = library::find(&reference) {
        room.name.to_string()
    } else {
        Path::new(&reference)
            .file_name()
            .map_or(reference.clone(), |n| n.to_string_lossy().into_owned())
    };
    let mut pick = None;
    egui::ComboBox::from_id_salt(("room", id))
        .selected_text(now)
        .width(200.0)
        .show_ui(ui, |ui| {
            ui.label(RichText::new("Made here").small().weak());
            for (i, name) in made.choices.iter().enumerate() {
                let on = reference.is_empty() && b.get(0) as usize == i;
                if ui.selectable_label(on, *name).clicked() {
                    pick = Some(RoomPick::Made(i));
                }
            }
            if !library::ROOMS.is_empty() {
                ui.separator();
                ui.label(RichText::new("Recorded spaces").small().weak());
                for room in library::ROOMS {
                    let on = library::find(&reference).is_some_and(|r| r.id == room.id);
                    if ui
                        .selectable_label(on, room.name)
                        .on_hover_text(room.credit())
                        .clicked()
                    {
                        pick = Some(RoomPick::Recorded(room));
                    }
                }
            }
            ui.separator();
            if ui
                .selectable_label(false, "Your own file…")
                .on_hover_text("A room's impulse response: WAV with 1, 2 or 4 (B-format) channels")
                .clicked()
            {
                pick = Some(RoomPick::File);
            }
        })
        .response
        .accessible_name("Room");
    if let Some(RoomPick::Made(i)) = pick {
        b.set(0, i as f64);
    }
    pick
}

/// The line under a convolution reverb's room: who recorded it, or
/// which file it is.
fn room_note(b: &daw_effects::Effect) -> Option<RichText> {
    let reference = b.response_ref();
    if reference.is_empty() {
        return None;
    }
    if b.response_missing() {
        return Some(
            RichText::new(
                "Its response file can't be read here, so a room made here plays instead. \
                 The project keeps the file's name.",
            )
            .small()
            .color(Color32::LIGHT_RED),
        );
    }
    let line = match daw_effects::library::find(&reference) {
        Some(room) => room.credit(),
        None => "From your own file, kept with the project.".into(),
    };
    Some(RichText::new(line).small().weak())
}

fn builtin_params(ui: &mut egui::Ui, id: u64, b: &daw_effects::Effect) -> Option<RoomPick> {
    let kind = b.kind();
    let mut pick = None;
    egui::Grid::new(("builtin", id))
        .num_columns(2)
        .show(ui, |ui| {
            for (i, p) in kind.params().iter().enumerate() {
                if kind == daw_effects::Kind::Convolution && i == 0 {
                    ui.label(p.name);
                    pick = room_picker(ui, id, b);
                    ui.end_row();
                    continue;
                }
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
    if let Some(note) = room_note(b) {
        ui.add(egui::Label::new(note).wrap());
    }
    pick
}

/// The presets menu for one plugin effect: save the settings as a new
/// preset, load or delete the user's own, load the plugin's own, or
/// pick one of its programs. Returns an error to show.
fn presets_menu(ui: &mut egui::Ui, s: &mut FxSlot, dir: &Path) -> Option<String> {
    let cid = *s.cid()?;
    let mut error = None;
    if s.presets.is_none() {
        let programs = s.plugin().map(Plugin::programs).unwrap_or_default();
        s.presets = Some((presets::list(dir, &cid), programs));
    }
    let (list, programs) = s.presets.clone().unwrap_or_default();
    ui.set_min_width(240.0);
    let mut save = false;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut s.preset_name)
                .hint_text("New preset name")
                .desired_width(150.0),
        )
        .labelled_by(ui.label("Save as").id);
        save = ui.button("Save").clicked();
    });
    if save {
        let state = s.plugin()?.save_state();
        match state.and_then(|state| presets::save(dir, &cid, &s.preset_name, &state)) {
            Ok(_) => {
                s.preset_name.clear();
                s.presets = None;
            }
            Err(e) => error = Some(e),
        }
    }
    let own: Vec<&Preset> = list.iter().filter(|p| p.own).collect();
    let native: Vec<&Preset> = list.iter().filter(|p| !p.own).collect();
    let mut load = None;
    let mut delete = None;
    if !own.is_empty() {
        ui.separator();
        ui.label(RichText::new("Your presets").weak());
        for p in own {
            ui.horizontal(|ui| {
                if ui.button(&p.name).clicked() {
                    load = Some(p.clone());
                }
                if ui
                    .small_button("✖")
                    .on_hover_text("Delete this preset")
                    .accessible_name(format!("Delete preset {}", p.name))
                    .clicked()
                {
                    delete = Some(p.clone());
                }
            });
        }
    }
    if !native.is_empty() {
        ui.separator();
        ui.label(RichText::new("The plugin's presets").weak());
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for p in native {
                    if ui
                        .button(&p.name)
                        .on_hover_text(p.path.display().to_string())
                        .clicked()
                    {
                        load = Some(p.clone());
                    }
                }
            });
    }
    let mut pick = None;
    for l in &programs {
        if l.programs.is_empty() {
            continue;
        }
        ui.separator();
        ui.label(RichText::new(&l.name).weak());
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .id_salt(("programs", l.id))
            .show(ui, |ui| {
                for (i, name) in l.programs.iter().enumerate() {
                    if ui.button(name).clicked() {
                        pick = Some((l.clone(), i));
                    }
                }
            });
    }
    if list.is_empty() && programs.iter().all(|l| l.programs.is_empty()) {
        ui.separator();
        ui.label(RichText::new("No presets yet.").weak());
    }
    if let Some(p) = load {
        match presets::load(&p.path, &cid).and_then(|state| s.load(&state)) {
            Ok(()) => ui.close(),
            Err(e) => error = Some(e),
        }
    }
    if let Some(p) = delete {
        match presets::delete(&p) {
            Ok(()) => s.presets = None,
            Err(e) => error = Some(e),
        }
    }
    if let Some((l, i)) = pick {
        match s.plugin()?.select_program(&l, i) {
            Ok(()) => ui.close(),
            Err(e) => error = Some(e),
        }
    }
    error
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

    use super::{Available, Effects, FxSlot, Sandbox, SlotKind, presets};
    use crate::App;
    use crate::freeze::test_plugin;
    use daw_test_plugin::PROCESSOR_CID;

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
    fn a_reverb_response_file_comes_into_the_project_and_back() {
        let dir = scratch("ir");
        let mut h = harness();
        let rate = h.state().rate;
        h.state_mut().add_file(&tone(&dir, rate));
        until(&mut h, |a| !a.tracks.is_empty());
        // A made-up room's response, as a file of the user's own.
        let ir = dir.join("my-room.wav");
        let left: Vec<f32> = (0..rate as usize / 2)
            .map(|i| ((i * 7919 % 997) as f32 / 997.0 - 0.5) * (-(i as f32) / 2000.0).exp())
            .collect();
        daw_effects::Response {
            rate,
            right: left.iter().map(|x| -x).collect(),
            left,
        }
        .to_wav(&ir)
        .unwrap();

        let app = h.state_mut();
        let mut slot = app.effects.create_builtin(Kind::Convolution, rate);
        builtin_of(&slot).set(2, 100.0);
        app.effects
            .set_response(&mut slot, &ir.to_string_lossy(), rate)
            .unwrap();
        app.tracks[0].fx.push(slot);
        app.rebuild();
        h.run();
        let frames = rate as usize;
        let heard = render(h.state_mut(), frames);

        // Saved: the file is copied in, and the project names the copy.
        let song = dir.join("Song");
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &song).unwrap();
        h.run();
        let (_, bytes) = ProjectFolder::open(&song).unwrap();
        let doc = ProjectDoc::from_bytes(&bytes, 1).unwrap();
        let e = &doc.get().tracks[0].effects[0];
        let folder = ProjectFolder { dir: song.clone() };
        assert!(folder.has_asset(&e.response), "{}", e.response);
        assert!(doc.assets_in_use().contains(&e.response));
        drop(h);

        // Reopened elsewhere, without the original file: the same sound.
        std::fs::remove_file(&ir).unwrap();
        let mut h = harness();
        h.state_mut().open_project(song.clone());
        until(&mut h, |a| !a.proj.loading());
        h.run();
        let b = builtin(h.state(), 0);
        assert!(!b.response_missing());
        let again = render(h.state_mut(), frames);
        let worst = (heard.iter().zip(&again)).fold(0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-3, "{worst}");

        // Back to a room made here, and undone.
        let app = h.state_mut();
        let mut slot = app.tracks[0].fx.remove(0);
        app.effects.set_response(&mut slot, "", rate).unwrap();
        app.tracks[0].fx.push(slot);
        h.run();
        assert_eq!(builtin(h.state(), 0).response_ref(), "");
        let ctx = h.ctx.clone();
        h.state_mut().undo(&ctx);
        h.run();
        assert_eq!(builtin(h.state(), 0).response_ref(), e.response);
        assert!(!builtin(h.state(), 0).response_missing());
    }

    #[test]
    fn a_recorded_room_comes_with_the_app_and_the_project_names_it() {
        let dir = scratch("room");
        let mut h = harness();
        let rate = h.state().rate;
        h.state_mut().add_file(&tone(&dir, rate));
        until(&mut h, |a| !a.tracks.is_empty());
        let room = daw_effects::library::find("room:cathedral-york-minster").unwrap();
        let reference = daw_effects::library::reference(room);

        let app = h.state_mut();
        let mut slot = app.effects.create_builtin(Kind::Convolution, rate);
        builtin_of(&slot).set(2, 100.0);
        app.effects
            .set_response(&mut slot, &reference, rate)
            .unwrap();
        app.tracks[0].fx.push(slot);
        app.rebuild();
        h.run();
        let frames = rate as usize * 2;
        let heard = render(h.state_mut(), frames);
        // The cathedral rings on after the second of tone ends.
        assert!(peak(&heard[rate as usize * 3..]) > 1e-3);

        // Saved by name only: nothing is copied into the project.
        let song = dir.join("Song");
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &song).unwrap();
        h.run();
        let (_, bytes) = ProjectFolder::open(&song).unwrap();
        let doc = ProjectDoc::from_bytes(&bytes, 1).unwrap();
        let e = &doc.get().tracks[0].effects[0];
        assert_eq!(e.response, reference);
        assert!(!doc.assets_in_use().contains(&e.response));
        assert!(!ProjectFolder { dir: song.clone() }.has_asset(&e.response));
        drop(h);

        // Reopened: the same room, heard the same.
        let mut h = harness();
        h.state_mut().open_project(song.clone());
        until(&mut h, |a| !a.proj.loading());
        h.run();
        assert!(!builtin(h.state(), 0).response_missing());
        let again = render(h.state_mut(), frames);
        let worst = (heard.iter().zip(&again)).fold(0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-3, "{worst}");
    }

    #[test]
    fn a_missing_response_file_is_kept_by_name() {
        let mut h = harness();
        let rate = h.state().rate;
        let e = daw_project::Effect {
            id: "e1".into(),
            name: "Room reverb".into(),
            builtin: "convolution".into(),
            response: "feedbeef.wav".into(),
            ..Default::default()
        };
        let slot = h.state_mut().effects.restore(&e, rate).unwrap();
        assert!(builtin_of(&slot).response_missing());
        assert_eq!(slot.value().response, "feedbeef.wav");
    }

    fn builtin_of(s: &super::FxSlot) -> &daw_effects::Effect {
        match &s.kind {
            SlotKind::Builtin(b) => b,
            SlotKind::Plugin { .. } => panic!("a plugin"),
        }
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

    fn effects(name: &str) -> Effects {
        let dir = scratch(name);
        let mut fx = Effects::new(dir.join("plugins.txt"));
        fx.helper = Some(test_plugin::helper());
        fx
    }

    fn plugin_of(s: &FxSlot) -> &daw_plugins::Plugin {
        s.plugin().expect("a plugin")
    }

    /// Runs blocks through `slot`'s audio side (waiting in `pending`)
    /// until its saved state carries `gain`: a parameter change reaches
    /// the plugin's state with the next block, and a live helper
    /// processor answers a little later.
    fn run_block(fx: &mut Effects, slot: &FxSlot, gain: f64) {
        let i = fx
            .pending
            .iter()
            .position(|(id, _)| *id == slot.id)
            .unwrap();
        let start = std::time::Instant::now();
        loop {
            let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
            fx.pending[i].1.process(&mut l, &mut r);
            let state = plugin_of(slot).save_state().unwrap();
            if state.component == gain.to_le_bytes() {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the setting never reached the plugin's state"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn available(bundle: &Path) -> Available {
        let class = daw_plugins::scan::scan_here(bundle)
            .unwrap()
            .into_iter()
            .find(|c| c.cid == PROCESSOR_CID)
            .unwrap();
        Available {
            bundle: bundle.to_owned(),
            class,
            enabled: true,
            sandbox: Sandbox::Shared,
        }
    }

    #[test]
    fn effects_run_where_the_user_chose_and_a_crash_can_be_restarted_from() {
        let mut fx = effects("crash");
        let good = available(&test_plugin::bundle());
        let bad = available(&test_plugin::broken(
            "DawTestCrashProcess",
            "crash-on-process",
        ));

        // The default is the shared helper; the user picks the rest.
        let mut g = fx.create(&good, 48_000).unwrap();
        assert_eq!(g.sandbox, Sandbox::Shared);
        assert!(plugin_of(&g).is_remote());
        fx.list.set_sandbox(&PROCESSOR_CID, Sandbox::InApp);
        let mut in_app = fx.create(&good, 48_000).unwrap();
        assert_eq!(in_app.sandbox, Sandbox::InApp);
        assert!(!plugin_of(&in_app).is_remote());
        fx.list.set_sandbox(&PROCESSOR_CID, Sandbox::Shared);
        // A built-in effect always runs in the app.
        let eq = fx.create_builtin(Kind::Eq, 48_000);
        assert_eq!(eq.sandbox, Sandbox::InApp);

        // The helper's delay counts in the latency the engine makes up for.
        assert_eq!(g.latency, daw_test_plugin::LATENCY + fx.block as u32);
        assert_eq!(in_app.latency, daw_test_plugin::LATENCY);
        assert!(!g.poll() && !in_app.poll());

        // Settings are saved, and kept for after a crash.
        plugin_of(&g).set_param(0, 0.25);
        run_block(&mut fx, &g, 0.25);
        let saved = g.state().expect("state is saved");
        assert_eq!(saved.component, 0.25f64.to_le_bytes());

        // A crash in the shared helper takes the good plugin's helper
        // too; both report it, their audio passes through, and the
        // settings last saved are still there.
        let mut b = fx.create(&bad, 48_000).unwrap();
        let (_p, mut render) = fx.fresh(&b, 48_000).unwrap();
        let (mut l, mut r) = (vec![0.5; 64], vec![0.5; 64]);
        render.process(&mut l, &mut r);
        assert!(l.iter().all(|&x| x == 0.5));
        assert!(b.poll(), "the graph is rebuilt so audio passes through");
        assert!(b.crashed.as_deref().unwrap().contains("crashed"));
        assert!(g.poll());
        assert!(g.crashed.is_some());
        assert!(!g.poll(), "reported once");
        assert_eq!(g.state(), Some(saved.clone()));
        assert!(in_app.crashed.is_none());

        // Restarting makes a new helper and brings the settings back.
        fx.reload(&mut g, 48_000).unwrap();
        assert!(g.crashed.is_none());
        assert_eq!(plugin_of(&g).param(0), Some(0.25));
        assert_eq!(g.state(), Some(saved));
        assert!(fx.pending.iter().any(|(id, _)| *id == g.id));
    }

    #[test]
    fn presets_are_saved_beside_the_plugin_list_and_load_back() {
        let mut fx = effects("presets");
        let good = available(&test_plugin::bundle());
        let mut slot = fx.create(&good, 48_000).unwrap();
        plugin_of(&slot).set_param(0, 0.5);
        run_block(&mut fx, &slot, 0.5);
        let state = plugin_of(&slot).save_state().unwrap();
        let cid = *slot.cid().unwrap();
        let preset = presets::save(&fx.presets_dir, &cid, "Half", &state).unwrap();
        assert!(preset.path.starts_with(fx.list_path.parent().unwrap()));
        plugin_of(&slot).set_param(0, 1.0);
        let back = presets::load(&preset.path, &cid).unwrap();
        slot.load(&back).unwrap();
        assert_eq!(plugin_of(&slot).param(0), Some(0.5));
        let listed = presets::list(&fx.presets_dir, &cid);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Half");
        assert!(listed[0].own);
    }
}
