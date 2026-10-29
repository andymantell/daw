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

#![allow(non_snake_case)]

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use rtrb::{Consumer, RingBuffer};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};

use daw_engine::graph::Insert;

use crate::com::{ComponentHandler, MemStream, ParamChanges, host_app, read_wstr};
use crate::host::{RemotePlugin, RemoteProcessor};
use crate::module::{Module, Result};
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

/// The UI side of a plugin running in this process.
pub struct LocalPlugin {
    // Declared first so it is dropped first: the editor must be closed
    // before its plugin.
    #[cfg(windows)]
    editor: Option<crate::editor_win::EditorWindow>,
    core: Arc<Core>,
    handler: ComWrapper<ComponentHandler>,
}

/// Edits queued between the UI and the audio thread.
const EDITS: usize = 1024;

/// Creates and starts the plugin class `cid` from `module`, ready to
/// process blocks of up to `max_block` frames at `rate`.
pub fn instantiate(
    module: Arc<Module>,
    cid: &TUID,
    rate: f64,
    max_block: usize,
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
        restart_requested: Default::default(),
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

    let buses = |dir: BusDirections| -> Vec<usize> {
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
                info.channelCount.max(0) as usize
            })
            .collect()
    };
    let inputs = buses(BusDirections_::kInput);
    let outputs = buses(BusDirections_::kOutput);
    if outputs.first().is_none_or(|&c| c == 0) {
        return Err("the plugin has no audio output".into());
    }

    let core = Arc::new(Core {
        component,
        processor,
        controller,
        separate,
        connections,
        _module: module,
    });
    let plugin = LocalPlugin {
        #[cfg(windows)]
        editor: None,
        core: core.clone(),
        handler,
    };
    plugin.sync_controller();
    unsafe {
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
        // Main buses only; side-chains get silence.
        for (dir, list) in [
            (BusDirections_::kInput, &inputs),
            (BusDirections_::kOutput, &outputs),
        ] {
            if !list.is_empty() {
                core.component.activateBus(
                    MediaTypes_::kAudio as MediaType,
                    dir as BusDirection,
                    0,
                    1,
                );
            }
        }
        check(core.component.setActive(1), "setActive")?;
        // Some plugins return kNotImplemented here, which is allowed.
        core.processor.setProcessing(1);
    }
    let processor = LocalProcessor::new(core, rx, &inputs, &outputs, max_block);
    Ok((plugin, processor))
}

impl LocalPlugin {
    /// Frames of delay the plugin adds.
    pub fn latency(&self) -> u32 {
        unsafe { self.core.processor.getLatencySamples() }
    }

    /// True once after the plugin asked the host to restart it, which
    /// it does when its latency or parameters change.
    pub fn take_restart_request(&self) -> bool {
        self.handler
            .restart_requested
            .swap(false, Ordering::Relaxed)
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
    pub fn poll(&mut self) -> Status {
        #[cfg(windows)]
        if self.editor.as_ref().is_some_and(|e| !e.is_open()) {
            self.editor = None;
        }
        Status {
            latency: self.latency(),
            restart: self.take_restart_request(),
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
struct BusBufs {
    channels: Vec<Vec<f32>>,
    ptrs: Vec<*mut f32>,
}

impl BusBufs {
    fn new(channels: usize, max_block: usize) -> BusBufs {
        let mut channels: Vec<Vec<f32>> = (0..channels).map(|_| vec![0.0; max_block]).collect();
        let ptrs = channels.iter_mut().map(|c| c.as_mut_ptr()).collect();
        BusBufs { channels, ptrs }
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
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len()).min(self.max_block);
        // Main input: left and right, or their mix for a mono input.
        if let Some(main) = self.inputs.first_mut() {
            match main.channels.as_mut_slice() {
                [] => {}
                [m] => {
                    for ((o, l), r) in m[..n].iter_mut().zip(&*left).zip(&*right) {
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
        for bus in self.inputs.iter_mut().skip(1) {
            for c in &mut bus.channels {
                c[..n].fill(0.0);
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
        let main = &self.outputs[0].channels;
        let r_src = main.get(1).unwrap_or(&main[0]);
        left[..n].copy_from_slice(&main[0][..n]);
        right[..n].copy_from_slice(&r_src[..n]);
    }
}

impl Insert for LocalProcessor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Blocks longer than the plugin was set up for go in pieces.
        let n = left.len().min(right.len());
        let mut done = 0;
        while done < n {
            let end = (done + self.max_block).min(n);
            let (l, r) = (&mut left[done..end], &mut right[done..end]);
            self.run(l, r);
            done = end;
        }
    }
}

// ---------------------------------------------------------------------------

/// The UI side of a plugin, wherever it runs.
pub enum Plugin {
    Local(LocalPlugin),
    Remote(RemotePlugin),
}

impl Plugin {
    /// Frames of delay the plugin adds, counting any the helper process
    /// adds to hand audio to and from it.
    pub fn latency(&self) -> u32 {
        match self {
            Plugin::Local(p) => p.latency(),
            Plugin::Remote(p) => p.latency(),
        }
    }

    /// Anything the plugin has flagged since last asked.
    pub fn poll(&mut self) -> Status {
        match self {
            Plugin::Local(p) => p.poll(),
            Plugin::Remote(p) => p.poll(),
        }
    }

    /// Why the plugin can no longer be used: its helper process crashed
    /// or stopped answering. A plugin in this process never fails this
    /// way (it takes the app with it instead).
    pub fn failure(&self) -> Option<String> {
        match self {
            Plugin::Local(_) => None,
            Plugin::Remote(p) => p.failure(),
        }
    }

    /// True if the plugin runs in a helper process.
    pub fn is_remote(&self) -> bool {
        matches!(self, Plugin::Remote(_))
    }

    pub fn params(&self) -> Vec<Param> {
        match self {
            Plugin::Local(p) => p.params(),
            Plugin::Remote(p) => p.params(),
        }
    }

    pub fn param(&self, id: u32) -> Option<f64> {
        match self {
            Plugin::Local(p) => p.param(id),
            Plugin::Remote(p) => p.param(id),
        }
    }

    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        match self {
            Plugin::Local(p) => p.param_text(id, value),
            Plugin::Remote(p) => p.param_text(id, value),
        }
    }

    pub fn set_param(&self, id: u32, value: f64) {
        match self {
            Plugin::Local(p) => p.set_param(id, value),
            Plugin::Remote(p) => p.set_param(id, value),
        }
    }

    pub fn save_state(&self) -> Result<PluginState> {
        match self {
            Plugin::Local(p) => p.save_state(),
            Plugin::Remote(p) => p.save_state(),
        }
    }

    pub fn load_state(&self, state: &PluginState) -> Result<()> {
        match self {
            Plugin::Local(p) => p.load_state(state),
            Plugin::Remote(p) => p.load_state(state),
        }
    }

    pub fn programs(&self) -> Vec<ProgramList> {
        match self {
            Plugin::Local(p) => p.programs(),
            Plugin::Remote(p) => p.programs(),
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
        }
    }

    pub fn editor_open(&self) -> bool {
        match self {
            Plugin::Local(p) => p.editor_open(),
            Plugin::Remote(p) => p.editor_open(),
        }
    }

    pub fn close_editor(&mut self) {
        match self {
            Plugin::Local(p) => p.close_editor(),
            Plugin::Remote(p) => p.close_editor(),
        }
    }
}

/// The audio side of a plugin, wherever it runs: an [`Insert`] for the
/// engine.
pub enum Processor {
    Local(LocalProcessor),
    Remote(RemoteProcessor),
}

impl Insert for Processor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self {
            Processor::Local(p) => p.process(left, right),
            Processor::Remote(p) => p.process(left, right),
        }
    }
}
