// SPDX-License-Identifier: GPL-3.0-or-later
//! One plugin instance, split into its two sides.
//!
//! [`instantiate`] returns a [`LocalPlugin`], kept by the UI (parameters,
//! state, editor), and a [`LocalProcessor`], handed to the engine as a
//! track insert and run on the audio thread. Both hold the same plugin
//! objects; the plugin is shut down when the last of the two is dropped,
//! which is always off the audio thread (the engine retires old graphs to
//! the UI thread).
//!
//! Parameter changes from the UI and from the plugin's own editor reach
//! the processor through a lock-free queue and are given to it with its
//! next block, as VST 3 asks.
//!
//! [`Plugin`] and [`Processor`] are the two sides of a plugin wherever
//! it runs: in this process, or in a helper process ([`crate::host`])
//! that a crash takes down instead of the app.
//!
//! **Buses.** A plugin can have more than one audio input and output.
//! The main ones carry the track's audio; a [`Layout`] switches on, as
//! well, the first side input (for a sidechain: another track's audio)
//! and any extra outputs, which are mixed into the track. Each bus is
//! asked for in stereo (mono where the plugin has it mono); a plugin
//! that wants otherwise keeps its own arrangement and the host adapts.
//!
//! **Restarts.** A plugin whose delay changes while it plays asks the
//! host to restart it. [`LocalPlugin::poll`] does so on the UI thread
//! while the audio side plays silence for the moment it takes (a lock
//! the audio thread only ever tries, never waits on), then reads the new
//! delay, which the app makes up for by rebuilding its graph. A plugin
//! whose buses changed, or that asks to be loaded again, is reported
//! for the app to make again with its settings.

#![allow(non_snake_case)]

use std::ffi::CStr;
use std::path::Path;
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, TryLockError};

use rtrb::{Consumer, RingBuffer};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};

use daw_engine::graph::Insert;

use crate::clap::{ClapModule, ClapPlugin, ClapProcessor};
use crate::com::{ComponentHandler, MemStream, ParamChanges, host_app, read_wstr};
use crate::host::{RemotePlugin, RemoteProcessor};
use crate::module::{ClassInfo, Module, Result};
use crate::proto::{ProgramList, Status};

fn check(r: tresult, what: &str) -> Result<()> {
    if r == kResultOk {
        Ok(())
    } else {
        Err(format!("{what} failed ({r})"))
    }
}

/// The plugin objects, shut down when the last side lets go.
struct Core {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// The controller is a separate object we created.
    separate: bool,
    connections: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    /// Held by the audio side for each block, and by the UI side while
    /// it restarts the plugin. The audio side only tries it: when the UI
    /// side has it, the block is silent.
    gate: Mutex<()>,
    // Dropped last: the module must outlive its objects.
    _module: Arc<Module>,
}

impl Drop for Core {
    fn drop(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            if let Some((a, b)) = self.connections.take() {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(ptr::null_mut());
                if self.separate {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

/// A parameter as the plugin describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub id: u32,
    pub title: String,
    pub units: String,
    pub automatable: bool,
    pub bypass: bool,
    /// Setting it picks one of the plugin's own programs (presets).
    pub program_change: bool,
    /// Steps for a stepped parameter, 0 for a continuous one.
    pub steps: i32,
}

/// One of a plugin's audio buses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bus {
    pub name: String,
    pub channels: u32,
    /// A side bus (a sidechain input, an extra output) rather than a
    /// main one.
    pub aux: bool,
}

/// A plugin's audio buses, as set up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buses {
    pub inputs: Vec<Bus>,
    pub outputs: Vec<Bus>,
}

impl Buses {
    /// The input a sidechain goes to: the first side input after the
    /// main one.
    pub fn sidechain(&self) -> Option<usize> {
        (self.inputs.iter().enumerate())
            .skip(1)
            .find(|(_, b)| b.aux && b.channels > 0)
            .map(|(i, _)| i)
    }

    /// The outputs after the main one, by index.
    pub fn extra_outputs(&self) -> impl Iterator<Item = (usize, &Bus)> {
        (self.outputs.iter().enumerate())
            .skip(1)
            .filter(|(_, b)| b.channels > 0)
    }
}

/// Which buses to switch on besides the main input and output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The sidechain input, if the plugin has one.
    pub sidechain: bool,
    /// Extra outputs mixed into the track, by bus index (the main
    /// output, 0, always is).
    pub outputs: Vec<u32>,
}

/// The UI side of a plugin running in this process.
pub struct LocalPlugin {
    // Declared first so it is dropped first: the editor must be closed
    // before its plugin.
    #[cfg(windows)]
    editor: Option<crate::editor_win::EditorWindow>,
    core: Arc<Core>,
    handler: ComWrapper<ComponentHandler>,
    buses: Buses,
}

/// Edits queued between the UI and the audio thread.
const EDITS: usize = 1024;

/// Creates and starts the plugin class `cid` from `module`, ready to
/// process blocks of up to `max_block` frames at `rate`, with its main
/// buses only.
pub fn instantiate(
    module: Arc<Module>,
    cid: &TUID,
    rate: f64,
    max_block: usize,
) -> Result<(LocalPlugin, LocalProcessor)> {
    instantiate_with(module, cid, rate, max_block, &Layout::default())
}

/// As [`instantiate`], also switching on the buses `layout` asks for.
pub fn instantiate_with(
    module: Arc<Module>,
    cid: &TUID,
    rate: f64,
    max_block: usize,
    layout: &Layout,
) -> Result<(LocalPlugin, LocalProcessor)> {
    let host = host_app();
    let component: ComPtr<IComponent> = module
        .create(cid)
        .ok_or("the plugin could not be created")?;
    check(
        unsafe { component.initialize(host.as_ptr()) },
        "component initialize",
    )?;
    let processor: ComPtr<IAudioProcessor> = component
        .cast()
        .ok_or("this is not an audio plugin (no IAudioProcessor)")?;

    // The controller is either the component itself or a separate class.
    let (controller, separate) = match component.cast::<IEditController>() {
        Some(c) => (Some(c), false),
        None => {
            let mut ccid: TUID = [0; 16];
            if unsafe { component.getControllerClassId(&mut ccid) } == kResultOk {
                let c: Option<ComPtr<IEditController>> = module.create(&ccid);
                if let Some(c) = &c {
                    check(
                        unsafe { c.initialize(host.as_ptr()) },
                        "controller initialize",
                    )?;
                }
                (c, true)
            } else {
                (None, false)
            }
        }
    };

    let (tx, rx) = RingBuffer::new(EDITS);
    let handler = ComWrapper::new(ComponentHandler {
        edits: Mutex::new(tx),
        restart: Default::default(),
        edited: Default::default(),
    });
    let mut connections = None;
    if let Some(c) = &controller {
        let h = handler.to_com_ptr::<IComponentHandler>().unwrap();
        unsafe { c.setComponentHandler(h.as_ptr()) };
        if separate
            && let (Some(a), Some(b)) = (
                component.cast::<IConnectionPoint>(),
                c.cast::<IConnectionPoint>(),
            )
        {
            unsafe {
                a.connect(b.as_ptr());
                b.connect(a.as_ptr());
            }
            connections = Some((a, b));
        }
    }

    fn read_buses(component: &ComPtr<IComponent>, dir: BusDirections) -> Vec<Bus> {
        let n =
            unsafe { component.getBusCount(MediaTypes_::kAudio as MediaType, dir as BusDirection) };
        (0..n)
            .map(|i| {
                let mut info: BusInfo = unsafe { std::mem::zeroed() };
                unsafe {
                    component.getBusInfo(
                        MediaTypes_::kAudio as MediaType,
                        dir as BusDirection,
                        i,
                        &mut info,
                    )
                };
                Bus {
                    name: read_wstr(&info.name),
                    channels: info.channelCount.max(0) as u32,
                    aux: info.busType == BusTypes_::kAux as BusType,
                }
            })
            .collect()
    }
    let before = Buses {
        inputs: read_buses(&component, BusDirections_::kInput),
        outputs: read_buses(&component, BusDirections_::kOutput),
    };
    if before.outputs.first().is_none_or(|b| b.channels == 0) {
        return Err("the plugin has no audio output".into());
    }

    let core = Arc::new(Core {
        component,
        processor,
        controller,
        separate,
        connections,
        gate: Mutex::new(()),
        _module: module,
    });
    let mut plugin = LocalPlugin {
        #[cfg(windows)]
        editor: None,
        core: core.clone(),
        handler,
        buses: before,
    };
    plugin.sync_controller();
    let (sidechain, active_out) = unsafe {
        let mut setup = ProcessSetup {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: max_block as i32,
            sampleRate: rate,
        };
        check(
            core.processor.setupProcessing(&mut setup),
            "setupProcessing",
        )?;
        // Stereo wherever the plugin will take it (mono buses stay
        // mono). A plugin that refuses keeps an arrangement of its own,
        // read back below; the host mixes whatever it gets to stereo.
        let want = |list: &[Bus]| -> Vec<SpeakerArrangement> {
            (list.iter())
                .map(|b| {
                    if b.channels == 1 {
                        SpeakerArr::kMono
                    } else {
                        SpeakerArr::kStereo
                    }
                })
                .collect()
        };
        let (mut ins, mut outs) = (want(&plugin.buses.inputs), want(&plugin.buses.outputs));
        core.processor.setBusArrangements(
            if ins.is_empty() {
                ptr::null_mut()
            } else {
                ins.as_mut_ptr()
            },
            ins.len() as i32,
            outs.as_mut_ptr(),
            outs.len() as i32,
        );
        plugin.buses = Buses {
            inputs: read_buses(&core.component, BusDirections_::kInput),
            outputs: read_buses(&core.component, BusDirections_::kOutput),
        };
        let sidechain = plugin.buses.sidechain().filter(|_| layout.sidechain);
        let mut active_in: Vec<usize> = Vec::new();
        if !plugin.buses.inputs.is_empty() {
            active_in.push(0);
        }
        active_in.extend(sidechain);
        let mut active_out = vec![0];
        for &o in &layout.outputs {
            let o = o as usize;
            if o > 0 && o < plugin.buses.outputs.len() && !active_out.contains(&o) {
                active_out.push(o);
            }
        }
        for (dir, list) in [
            (BusDirections_::kInput, &active_in),
            (BusDirections_::kOutput, &active_out),
        ] {
            for &i in list {
                core.component.activateBus(
                    MediaTypes_::kAudio as MediaType,
                    dir as BusDirection,
                    i as i32,
                    1,
                );
            }
        }
        check(core.component.setActive(1), "setActive")?;
        // Some plugins return kNotImplemented here, which is allowed.
        core.processor.setProcessing(1);
        (sidechain, active_out)
    };
    let channels = |list: &[Bus]| list.iter().map(|b| b.channels as usize).collect::<Vec<_>>();
    let mut processor = LocalProcessor::new(
        core,
        rx,
        &channels(&plugin.buses.inputs),
        &channels(&plugin.buses.outputs),
        max_block,
    );
    processor.key = sidechain;
    processor.mix = active_out[1..].to_vec();
    Ok((plugin, processor))
}

impl LocalPlugin {
    /// Frames of delay the plugin adds.
    pub fn latency(&self) -> u32 {
        unsafe { self.core.processor.getLatencySamples() }
    }

    /// The restart flags the plugin has raised since last asked (zero
    /// for none): it raises them when its delay, buses or parameters
    /// change.
    pub fn take_restart_flags(&self) -> i32 {
        self.handler.restart.swap(0, Ordering::Relaxed)
    }

    /// The plugin's audio buses, as set up.
    pub fn buses(&self) -> &Buses {
        &self.buses
    }

    /// Stops and starts the plugin's processing, as VST 3 asks for once
    /// its delay has changed, while the audio side plays silence.
    pub fn reactivate(&self) {
        let _quiet = self.core.gate.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            self.core.processor.setProcessing(0);
            self.core.component.setActive(0);
            self.core.component.setActive(1);
            self.core.processor.setProcessing(1);
        }
    }

    /// The id the plugin now uses for parameter `old` of the class
    /// `old_cid` it replaces (an older version of itself), if it says.
    pub fn compatible_param(&self, old_cid: &TUID, old: u32) -> Option<u32> {
        let c = self.core.controller.as_ref()?;
        let remap = c.cast::<IRemapParamID>()?;
        let mut new = 0;
        (unsafe { remap.getCompatibleParamID(old_cid, old, &mut new) } == kResultTrue)
            .then_some(new)
    }

    /// True once after the user changed something in the plugin's editor.
    pub fn take_edited(&self) -> bool {
        self.handler.edited.swap(false, Ordering::Relaxed)
    }

    /// Pushes the component's state to a separate controller.
    fn sync_controller(&self) {
        if let (true, Some(c)) = (self.core.separate, &self.core.controller) {
            let s = ComWrapper::new(MemStream::default());
            let p = s.to_com_ptr::<IBStream>().unwrap();
            if unsafe { self.core.component.getState(p.as_ptr()) } == kResultOk {
                let r = ComWrapper::new(MemStream::with_bytes(s.bytes()));
                let rp = r.to_com_ptr::<IBStream>().unwrap();
                unsafe { c.setComponentState(rp.as_ptr()) };
            }
        }
    }

    pub fn params(&self) -> Vec<Param> {
        let Some(c) = &self.core.controller else {
            return Vec::new();
        };
        let n = unsafe { c.getParameterCount() };
        (0..n)
            .filter_map(|i| {
                let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
                (unsafe { c.getParameterInfo(i, &mut info) } == kResultOk).then(|| Param {
                    id: info.id,
                    title: read_wstr(&info.title),
                    units: read_wstr(&info.units),
                    automatable: info.flags & ParameterInfo_::ParameterFlags_::kCanAutomate as i32
                        != 0,
                    bypass: info.flags & ParameterInfo_::ParameterFlags_::kIsBypass as i32 != 0,
                    program_change: info.flags
                        & ParameterInfo_::ParameterFlags_::kIsProgramChange as i32
                        != 0,
                    steps: info.stepCount,
                })
            })
            .collect()
    }

    /// A parameter's normalised value (0 to 1), as the controller has it.
    pub fn param(&self, id: u32) -> Option<f64> {
        let c = self.core.controller.as_ref()?;
        Some(unsafe { c.getParamNormalized(id) })
    }

    /// How the plugin would display a normalised value of `id`.
    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        let c = self.core.controller.as_ref()?;
        let mut s: String128 = [0; 128];
        (unsafe { c.getParamStringByValue(id, value, &mut s) } == kResultOk).then(|| read_wstr(&s))
    }

    /// Sets a parameter from the host's own controls: the controller is
    /// told now, the processor with its next block.
    pub fn set_param(&self, id: u32, value: f64) {
        if let Some(c) = &self.core.controller {
            unsafe { c.setParamNormalized(id, value) };
        }
        if let Ok(mut q) = self.handler.edits.lock() {
            let _ = q.push((id, value));
        }
    }

    /// The plugin's state: the component's, then the controller's if it
    /// is separate. Safe while processing.
    pub fn save_state(&self) -> Result<PluginState> {
        let s = ComWrapper::new(MemStream::default());
        let p = s.to_com_ptr::<IBStream>().unwrap();
        check(
            unsafe { self.core.component.getState(p.as_ptr()) },
            "saving the plugin's state",
        )?;
        let controller = self
            .core
            .controller
            .as_ref()
            .filter(|_| self.core.separate)
            .and_then(|c| {
                let s = ComWrapper::new(MemStream::default());
                let p = s.to_com_ptr::<IBStream>().unwrap();
                (unsafe { c.getState(p.as_ptr()) } == kResultOk).then(|| s.bytes())
            });
        Ok(PluginState {
            component: s.bytes(),
            controller,
        })
    }

    pub fn load_state(&self, state: &PluginState) -> Result<()> {
        let s = ComWrapper::new(MemStream::with_bytes(state.component.clone()));
        let p = s.to_com_ptr::<IBStream>().unwrap();
        check(
            unsafe { self.core.component.setState(p.as_ptr()) },
            "restoring the plugin's state",
        )?;
        self.sync_controller();
        if let (Some(bytes), Some(c)) = (&state.controller, &self.core.controller) {
            let s = ComWrapper::new(MemStream::with_bytes(bytes.clone()));
            let p = s.to_com_ptr::<IBStream>().unwrap();
            unsafe { c.setState(p.as_ptr()) };
        }
        Ok(())
    }

    /// The plugin's own program (preset) lists, if it offers any.
    pub fn programs(&self) -> Vec<ProgramList> {
        let Some(c) = &self.core.controller else {
            return Vec::new();
        };
        let Some(u) = c.cast::<IUnitInfo>() else {
            return Vec::new();
        };
        let n = unsafe { u.getProgramListCount() };
        (0..n)
            .filter_map(|i| {
                let mut info: ProgramListInfo = unsafe { std::mem::zeroed() };
                if unsafe { u.getProgramListInfo(i, &mut info) } != kResultOk {
                    return None;
                }
                let programs = (0..info.programCount)
                    .map(|j| {
                        let mut name: String128 = [0; 128];
                        unsafe { u.getProgramName(info.id, j, &mut name) };
                        read_wstr(&name)
                    })
                    .collect();
                Some(ProgramList {
                    id: info.id,
                    name: read_wstr(&info.name),
                    programs,
                })
            })
            .collect()
    }

    /// What the plugin has flagged since last asked, and its latency.
    /// A plugin whose delay changed is restarted here first.
    pub fn poll(&mut self) -> Status {
        #[cfg(windows)]
        if self.editor.as_ref().is_some_and(|e| !e.is_open()) {
            self.editor = None;
        }
        let flags = self.take_restart_flags();
        if flags & RestartFlags_::kLatencyChanged != 0 {
            self.reactivate();
        }
        Status {
            latency: self.latency(),
            restart: flags != 0,
            reload: flags & (RestartFlags_::kIoChanged | RestartFlags_::kReloadComponent) != 0,
            edited: self.take_edited(),
            editor_open: self.editor_open(),
        }
    }

    /// The plugin's editor view, if it has one. Open and close it on the
    /// UI thread.
    pub fn create_editor(&self) -> Option<ComPtr<IPlugView>> {
        let c = self.core.controller.as_ref()?;
        let editor: &CStr = c"editor";
        unsafe { ComPtr::from_raw(c.createView(editor.as_ptr())) }
    }

    /// Opens the plugin's own editor in a window titled `title`, or
    /// brings it to the front.
    pub fn open_editor(&mut self, title: &str) -> Result<()> {
        #[cfg(windows)]
        {
            if let Some(e) = &self.editor {
                e.focus();
                return Ok(());
            }
            let view = self
                .create_editor()
                .ok_or("this plugin has no editor of its own")?;
            self.editor = Some(crate::editor_win::EditorWindow::open(view, title)?);
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = title;
            Err("plugin windows are not supported on this platform yet".into())
        }
    }

    pub fn editor_open(&self) -> bool {
        #[cfg(windows)]
        {
            self.editor.as_ref().is_some_and(|e| e.is_open())
        }
        #[cfg(not(windows))]
        false
    }

    pub fn close_editor(&mut self) {
        #[cfg(windows)]
        {
            self.editor = None;
        }
    }
}

/// A plugin's saved state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginState {
    pub component: Vec<u8>,
    pub controller: Option<Vec<u8>>,
}

/// One bus's buffers, allocated up front.
pub(crate) struct BusBufs {
    pub(crate) channels: Vec<Vec<f32>>,
    pub(crate) ptrs: Vec<*mut f32>,
}

impl BusBufs {
    pub(crate) fn new(channels: usize, max_block: usize) -> BusBufs {
        let mut channels: Vec<Vec<f32>> = (0..channels).map(|_| vec![0.0; max_block]).collect();
        let ptrs = channels.iter_mut().map(|c| c.as_mut_ptr()).collect();
        BusBufs { channels, ptrs }
    }

    /// Left and right into the bus's first `n` frames, or their mix for
    /// a mono bus; channels past the second are silent.
    pub(crate) fn fill(&mut self, left: &[f32], right: &[f32], n: usize) {
        match self.channels.as_mut_slice() {
            [] => {}
            [m] => {
                for ((o, l), r) in m[..n].iter_mut().zip(left).zip(right) {
                    *o = 0.5 * (l + r);
                }
            }
            [a, b, rest @ ..] => {
                a[..n].copy_from_slice(&left[..n]);
                b[..n].copy_from_slice(&right[..n]);
                for c in rest {
                    c[..n].fill(0.0);
                }
            }
        }
    }

    pub(crate) fn silence(&mut self, n: usize) {
        for c in &mut self.channels {
            c[..n].fill(0.0);
        }
    }

    /// The bus's first `n` frames into left and right (a mono bus into
    /// both), replacing what is there or, with `add`, mixed in.
    pub(crate) fn out(&self, left: &mut [f32], right: &mut [f32], n: usize, add: bool) {
        let Some(a) = self.channels.first() else {
            return;
        };
        let b = self.channels.get(1).unwrap_or(a);
        if add {
            for (o, x) in left[..n].iter_mut().zip(&a[..n]) {
                *o += x;
            }
            for (o, x) in right[..n].iter_mut().zip(&b[..n]) {
                *o += x;
            }
        } else {
            left[..n].copy_from_slice(&a[..n]);
            right[..n].copy_from_slice(&b[..n]);
        }
    }
}

/// The audio side of a plugin running in this process: an [`Insert`]
/// for the engine.
pub struct LocalProcessor {
    core: Arc<Core>,
    edits: Consumer<(ParamID, ParamValue)>,
    changes: ComWrapper<ParamChanges>,
    changes_ptr: ComPtr<IParameterChanges>,
    out_changes: ComWrapper<ParamChanges>,
    out_changes_ptr: ComPtr<IParameterChanges>,
    inputs: Vec<BusBufs>,
    outputs: Vec<BusBufs>,
    in_buses: Vec<AudioBusBuffers>,
    out_buses: Vec<AudioBusBuffers>,
    max_block: usize,
    /// The input bus a sidechain goes to, when switched on.
    key: Option<usize>,
    /// Extra output buses mixed into the track.
    mix: Vec<usize>,
}

// The raw pointers point into buffers this struct owns and never resizes.
unsafe impl Send for LocalProcessor {}

impl LocalProcessor {
    fn new(
        core: Arc<Core>,
        edits: Consumer<(ParamID, ParamValue)>,
        inputs: &[usize],
        outputs: &[usize],
        max_block: usize,
    ) -> LocalProcessor {
        let changes = ComWrapper::new(ParamChanges::new());
        let changes_ptr = changes.to_com_ptr::<IParameterChanges>().unwrap();
        let out_changes = ComWrapper::new(ParamChanges::new());
        let out_changes_ptr = out_changes.to_com_ptr::<IParameterChanges>().unwrap();
        let mut inputs: Vec<BusBufs> = inputs.iter().map(|&c| BusBufs::new(c, max_block)).collect();
        let mut outputs: Vec<BusBufs> = outputs
            .iter()
            .map(|&c| BusBufs::new(c, max_block))
            .collect();
        let bus = |b: &mut BusBufs| AudioBusBuffers {
            numChannels: b.ptrs.len() as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: b.ptrs.as_mut_ptr(),
            },
        };
        let in_buses = inputs.iter_mut().map(bus).collect();
        let out_buses = outputs.iter_mut().map(bus).collect();
        LocalProcessor {
            core,
            edits,
            changes,
            changes_ptr,
            out_changes,
            out_changes_ptr,
            inputs,
            outputs,
            in_buses,
            out_buses,
            max_block,
            key: None,
            mix: Vec::new(),
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let n = left.len().min(right.len()).min(self.max_block);
        if let Some(main) = self.inputs.first_mut() {
            main.fill(left, right, n);
        }
        for (i, bus) in self.inputs.iter_mut().enumerate().skip(1) {
            match key {
                Some((kl, kr)) if Some(i) == self.key && kl.len() >= n && kr.len() >= n => {
                    bus.fill(kl, kr, n)
                }
                _ => bus.silence(n),
            }
        }

        self.changes.clear();
        self.out_changes.clear();
        while let Ok((id, v)) = self.edits.pop() {
            if !self.changes.add(id, 0, v) {
                break;
            }
        }

        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: n as i32,
            numInputs: self.in_buses.len() as i32,
            numOutputs: self.out_buses.len() as i32,
            inputs: if self.in_buses.is_empty() {
                ptr::null_mut()
            } else {
                self.in_buses.as_mut_ptr()
            },
            outputs: self.out_buses.as_mut_ptr(),
            inputParameterChanges: if self.changes.is_empty() {
                ptr::null_mut()
            } else {
                self.changes_ptr.as_ptr()
            },
            outputParameterChanges: self.out_changes_ptr.as_ptr(),
            inputEvents: ptr::null_mut(),
            outputEvents: ptr::null_mut(),
            processContext: ptr::null_mut(),
        };
        if unsafe { self.core.processor.process(&mut data) } != kResultOk {
            // Leave the audio as it was: a failing effect is skipped.
            return;
        }
        self.outputs[0].out(left, right, n, false);
        for &i in &self.mix {
            if let Some(bus) = self.outputs.get(i) {
                bus.out(left, right, n, true);
            }
        }
    }

    fn blocks(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        // While the UI side restarts the plugin, the block is silent.
        let core = self.core.clone();
        let _running = match core.gate.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => {
                left.fill(0.0);
                right.fill(0.0);
                return;
            }
        };
        // Blocks longer than the plugin was set up for go in pieces.
        let n = left.len().min(right.len());
        let mut done = 0;
        while done < n {
            let end = (done + self.max_block).min(n);
            let (l, r) = (&mut left[done..end], &mut right[done..end]);
            let k = key.map(|(a, b)| {
                (
                    a.get(done..end).unwrap_or(&[]),
                    b.get(done..end).unwrap_or(&[]),
                )
            });
            self.run(l, r, k);
            done = end;
        }
    }
}

impl Insert for LocalProcessor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.blocks(left, right, None);
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        self.blocks(left, right, Some(key));
    }
}

// ---------------------------------------------------------------------------

/// The UI side of a plugin, wherever it runs.
pub enum Plugin {
    Local(LocalPlugin),
    Remote(RemotePlugin),
    /// A plugin in the CLAP format, in this process.
    Clap(ClapPlugin),
}

impl Plugin {
    /// Frames of delay the plugin adds, counting any the helper process
    /// adds to hand audio to and from it.
    pub fn latency(&self) -> u32 {
        match self {
            Plugin::Local(p) => p.latency(),
            Plugin::Remote(p) => p.latency(),
            Plugin::Clap(p) => p.latency(),
        }
    }

    /// Anything the plugin has flagged since last asked.
    pub fn poll(&mut self) -> Status {
        match self {
            Plugin::Local(p) => p.poll(),
            Plugin::Remote(p) => p.poll(),
            Plugin::Clap(p) => p.poll(),
        }
    }

    /// Why the plugin can no longer be used: its helper process crashed
    /// or stopped answering. A plugin in this process never fails this
    /// way (it takes the app with it instead).
    pub fn failure(&self) -> Option<String> {
        match self {
            Plugin::Local(_) | Plugin::Clap(_) => None,
            Plugin::Remote(p) => p.failure(),
        }
    }

    /// True if the plugin runs in a helper process.
    pub fn is_remote(&self) -> bool {
        matches!(self, Plugin::Remote(_))
    }

    /// The plugin's audio buses, as set up.
    pub fn buses(&self) -> Buses {
        match self {
            Plugin::Local(p) => p.buses().clone(),
            Plugin::Remote(p) => p.buses().clone(),
            Plugin::Clap(p) => p.buses().clone(),
        }
    }

    /// The id the plugin now uses for parameter `old` of the older
    /// class `old_cid` it replaces, if it says (for automation saved
    /// against the older version).
    pub fn compatible_param(&self, old_cid: &TUID, old: u32) -> Option<u32> {
        match self {
            Plugin::Local(p) => p.compatible_param(old_cid, old),
            Plugin::Remote(p) => p.compatible_param(old_cid, old),
            // CLAP has no way to say this.
            Plugin::Clap(_) => None,
        }
    }

    pub fn params(&self) -> Vec<Param> {
        match self {
            Plugin::Local(p) => p.params(),
            Plugin::Remote(p) => p.params(),
            Plugin::Clap(p) => p.params(),
        }
    }

    pub fn param(&self, id: u32) -> Option<f64> {
        match self {
            Plugin::Local(p) => p.param(id),
            Plugin::Remote(p) => p.param(id),
            Plugin::Clap(p) => p.param(id),
        }
    }

    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        match self {
            Plugin::Local(p) => p.param_text(id, value),
            Plugin::Remote(p) => p.param_text(id, value),
            Plugin::Clap(p) => p.param_text(id, value),
        }
    }

    pub fn set_param(&self, id: u32, value: f64) {
        match self {
            Plugin::Local(p) => p.set_param(id, value),
            Plugin::Remote(p) => p.set_param(id, value),
            Plugin::Clap(p) => p.set_param(id, value),
        }
    }

    pub fn save_state(&self) -> Result<PluginState> {
        match self {
            Plugin::Local(p) => p.save_state(),
            Plugin::Remote(p) => p.save_state(),
            Plugin::Clap(p) => p.save_state(),
        }
    }

    pub fn load_state(&self, state: &PluginState) -> Result<()> {
        match self {
            Plugin::Local(p) => p.load_state(state),
            Plugin::Remote(p) => p.load_state(state),
            Plugin::Clap(p) => p.load_state(state),
        }
    }

    pub fn programs(&self) -> Vec<ProgramList> {
        match self {
            Plugin::Local(p) => p.programs(),
            Plugin::Remote(p) => p.programs(),
            // A CLAP plugin's own presets are not looked for yet.
            Plugin::Clap(_) => Vec::new(),
        }
    }

    /// Picks program `index` of the plugin's own list `list`, through
    /// its program-change parameter.
    pub fn select_program(&self, list: &ProgramList, index: usize) -> Result<()> {
        let count = list.programs.len();
        if index >= count {
            return Err("no such program".into());
        }
        let p = self
            .params()
            .into_iter()
            .find(|p| p.program_change)
            .ok_or("this plugin's programs can't be picked from here")?;
        let steps = if p.steps > 0 {
            p.steps as usize
        } else {
            count.saturating_sub(1)
        };
        self.set_param(p.id, index as f64 / steps.max(1) as f64);
        Ok(())
    }

    pub fn open_editor(&mut self, title: &str) -> Result<()> {
        match self {
            Plugin::Local(p) => p.open_editor(title),
            Plugin::Remote(p) => p.open_editor(title),
            Plugin::Clap(p) => p.open_editor(title),
        }
    }

    pub fn editor_open(&self) -> bool {
        match self {
            Plugin::Local(p) => p.editor_open(),
            Plugin::Remote(p) => p.editor_open(),
            Plugin::Clap(p) => p.editor_open(),
        }
    }

    pub fn close_editor(&mut self) {
        match self {
            Plugin::Local(p) => p.close_editor(),
            Plugin::Remote(p) => p.close_editor(),
            Plugin::Clap(p) => p.close_editor(),
        }
    }
}

/// The audio side of a plugin, wherever it runs: an [`Insert`] for the
/// engine.
pub enum Processor {
    Local(LocalProcessor),
    Remote(RemoteProcessor),
    Clap(ClapProcessor),
}

impl Insert for Processor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self {
            Processor::Local(p) => p.process(left, right),
            Processor::Remote(p) => p.process(left, right),
            Processor::Clap(p) => p.process(left, right),
        }
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        match self {
            Processor::Local(p) => p.process_keyed(left, right, key),
            Processor::Remote(p) => p.process_keyed(left, right, key),
            Processor::Clap(p) => p.process_keyed(left, right, key),
        }
    }
}

/// A loaded plugin file, in either format.
#[derive(Clone)]
pub enum Library {
    Vst3(Arc<Module>),
    Clap(Arc<ClapModule>),
}

impl Library {
    /// Loads the plugin file at `path`: a CLAP plugin if its extension
    /// says so, otherwise a VST 3 bundle.
    pub fn load(path: &Path) -> Result<Library> {
        Ok(if crate::clap::is_clap(path) {
            Library::Clap(Arc::new(ClapModule::load(path)?))
        } else {
            Library::Vst3(Arc::new(Module::load(path)?))
        })
    }

    /// The classes (plugins and helpers) the file offers.
    pub fn classes(&self) -> Vec<ClassInfo> {
        match self {
            Library::Vst3(m) => m.classes(),
            Library::Clap(m) => m.classes(),
        }
    }

    /// Makes and starts class `cid`, in this process, ready to process
    /// blocks of up to `max_block` frames at `rate`, with the buses
    /// `layout` asks for.
    pub fn instantiate(
        &self,
        cid: &TUID,
        rate: f64,
        max_block: usize,
        layout: &Layout,
    ) -> Result<(Plugin, Processor)> {
        Ok(match self {
            Library::Vst3(m) => {
                let (p, x) = instantiate_with(m.clone(), cid, rate, max_block, layout)?;
                (Plugin::Local(p), Processor::Local(x))
            }
            Library::Clap(m) => {
                let (p, x) =
                    crate::clap::instantiate_with(m.clone(), cid, rate, max_block, layout)?;
                (Plugin::Clap(p), Processor::Clap(x))
            }
        })
    }
}
