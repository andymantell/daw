// SPDX-License-Identifier: GPL-3.0-or-later
//! Loading a VST 3 module and driving one plugin instance.

#![allow(non_snake_case)]

use std::ffi::{CStr, c_void};
use std::path::{Path, PathBuf};
use std::ptr;

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper, Interface};

use crate::com::{
    ComponentHandler, EventList, MemStream, ParamChanges, host_app, read_cstr, read_wstr,
};

pub type Result<T> = std::result::Result<T, String>;

fn check(r: tresult, what: &str) -> Result<()> {
    if r == kResultOk {
        Ok(())
    } else {
        Err(format!("{what} failed ({r})"))
    }
}

/// The binary inside a `.vst3` bundle for this platform, or the path
/// itself for the older single-file layout.
pub fn binary_path(bundle: &Path) -> PathBuf {
    if !bundle.is_dir() {
        return bundle.to_owned();
    }
    let stem = bundle.file_stem().unwrap_or_default();
    let contents = bundle.join("Contents");
    if cfg!(windows) {
        contents
            .join("x86_64-win")
            .join(bundle.file_name().unwrap_or_default())
    } else if cfg!(target_os = "macos") {
        contents.join("MacOS").join(stem)
    } else {
        contents
            .join(format!("{}-linux", std::env::consts::ARCH))
            .join(stem)
            .with_extension("so")
    }
}

pub struct ClassInfo {
    pub cid: TUID,
    pub name: String,
    pub category: String,
}

/// A loaded module. Dropping it calls the module's exit function.
pub struct Module {
    // Field order matters: the factory must be released before the
    // library is unloaded.
    pub factory: ComPtr<IPluginFactory>,
    lib: libloading::Library,
}

impl Module {
    pub fn load(bundle: &Path) -> Result<Module> {
        let bin = binary_path(bundle);
        let lib = unsafe { libloading::Library::new(&bin) }
            .map_err(|e| format!("could not load {}: {e}", bin.display()))?;
        unsafe {
            #[cfg(windows)]
            if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0")
                && !init()
            {
                return Err("InitDll returned false".into());
            }
            #[cfg(target_os = "linux")]
            if let Ok(entry) =
                lib.get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"ModuleEntry\0")
                && !entry(ptr::null_mut())
            {
                return Err("ModuleEntry returned false".into());
            }
            let get = lib
                .get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0")
                .map_err(|e| format!("no GetPluginFactory: {e}"))?;
            let factory = ComPtr::from_raw(get()).ok_or("GetPluginFactory returned null")?;
            Ok(Module { factory, lib })
        }
    }

    pub fn classes(&self) -> Vec<ClassInfo> {
        let n = unsafe { self.factory.countClasses() };
        (0..n)
            .filter_map(|i| {
                let mut info: PClassInfo = unsafe { std::mem::zeroed() };
                (unsafe { self.factory.getClassInfo(i, &mut info) } == kResultOk).then(|| {
                    ClassInfo {
                        cid: info.cid,
                        name: read_cstr(&info.name),
                        category: read_cstr(&info.category),
                    }
                })
            })
            .collect()
    }

    fn create<I: Interface>(&self, cid: &TUID) -> Option<ComPtr<I>> {
        let mut obj: *mut c_void = ptr::null_mut();
        let iid = I::IID;
        let r = unsafe {
            self.factory
                .createInstance(cid.as_ptr(), iid.as_ptr() as FIDString, &mut obj)
        };
        if r != kResultOk {
            return None;
        }
        unsafe { ComPtr::from_raw(obj as *mut I) }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            #[cfg(windows)]
            if let Ok(exit) = self
                .lib
                .get::<unsafe extern "system" fn() -> bool>(b"ExitDll\0")
            {
                exit();
            }
            #[cfg(target_os = "linux")]
            if let Ok(exit) = self
                .lib
                .get::<unsafe extern "system" fn() -> bool>(b"ModuleExit\0")
            {
                exit();
            }
        }
    }
}

pub struct Bus {
    pub channels: i32,
}

pub struct Param {
    pub id: ParamID,
    pub title: String,
    pub automatable: bool,
    pub bypass: bool,
}

/// One plugin instance, initialised and ready to process.
pub struct Instance {
    pub component: ComPtr<IComponent>,
    pub processor: ComPtr<IAudioProcessor>,
    pub controller: Option<ComPtr<IEditController>>,
    /// True when the controller is a separate object we created.
    separate_controller: bool,
    connections: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    pub handler: ComWrapper<ComponentHandler>,
    pub inputs: Vec<Bus>,
    pub outputs: Vec<Bus>,
    pub has_event_input: bool,
    pub block: usize,
    active: bool,
}

impl Instance {
    pub fn new(module: &Module, cid: &TUID, rate: f64, block: usize) -> Result<Instance> {
        let host = host_app();
        let component: ComPtr<IComponent> = module
            .create(cid)
            .ok_or("factory could not create the component")?;
        check(
            unsafe { component.initialize(host.as_ptr()) },
            "component initialize",
        )?;
        let processor: ComPtr<IAudioProcessor> =
            component.cast().ok_or("component has no IAudioProcessor")?;

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

        let handler = ComWrapper::new(ComponentHandler::default());
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

        let buses = |dir: BusDirections| -> Vec<Bus> {
            let n = unsafe {
                component.getBusCount(MediaTypes_::kAudio as MediaType, dir as BusDirection)
            };
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
                        channels: info.channelCount.max(0),
                    }
                })
                .collect()
        };
        let inputs = buses(BusDirections_::kInput);
        let outputs = buses(BusDirections_::kOutput);
        let has_event_input = unsafe {
            component.getBusCount(
                MediaTypes_::kEvent as MediaType,
                BusDirections_::kInput as BusDirection,
            )
        } > 0;

        let mut inst = Instance {
            component,
            processor,
            controller,
            separate_controller: separate,
            connections,
            handler,
            inputs,
            outputs,
            has_event_input,
            block,
            active: false,
        };
        inst.sync_controller();
        inst.activate(rate)?;
        Ok(inst)
    }

    fn activate(&mut self, rate: f64) -> Result<()> {
        unsafe {
            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as i32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
                maxSamplesPerBlock: self.block as i32,
                sampleRate: rate,
            };
            check(
                self.processor.setupProcessing(&mut setup),
                "setupProcessing",
            )?;
            // Main buses only.
            for (dir, list) in [
                (BusDirections_::kInput, &self.inputs),
                (BusDirections_::kOutput, &self.outputs),
            ] {
                if !list.is_empty() {
                    self.component.activateBus(
                        MediaTypes_::kAudio as MediaType,
                        dir as BusDirection,
                        0,
                        1,
                    );
                }
            }
            if self.has_event_input {
                self.component.activateBus(
                    MediaTypes_::kEvent as MediaType,
                    BusDirections_::kInput as BusDirection,
                    0,
                    1,
                );
            }
            check(self.component.setActive(1), "setActive")?;
            // Some plugins return kNotImplemented here; that's allowed.
            self.processor.setProcessing(1);
        }
        self.active = true;
        Ok(())
    }

    /// Pushes the component's state to a separate controller.
    pub fn sync_controller(&self) {
        if let (true, Some(c)) = (self.separate_controller, &self.controller) {
            let s = ComWrapper::new(MemStream::default());
            let p = s.to_com_ptr::<IBStream>().unwrap();
            if unsafe { self.component.getState(p.as_ptr()) } == kResultOk {
                let r = ComWrapper::new(MemStream::with_bytes(s.bytes()));
                let rp = r.to_com_ptr::<IBStream>().unwrap();
                unsafe { c.setComponentState(rp.as_ptr()) };
            }
        }
    }

    pub fn latency(&self) -> u32 {
        unsafe { self.processor.getLatencySamples() }
    }

    pub fn params(&self) -> Vec<Param> {
        let Some(c) = &self.controller else {
            return Vec::new();
        };
        let n = unsafe { c.getParameterCount() };
        (0..n)
            .filter_map(|i| {
                let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
                (unsafe { c.getParameterInfo(i, &mut info) } == kResultOk).then(|| Param {
                    id: info.id,
                    title: read_wstr(&info.title),
                    automatable: info.flags & ParameterInfo_::ParameterFlags_::kCanAutomate as i32
                        != 0,
                    bypass: info.flags & ParameterInfo_::ParameterFlags_::kIsBypass as i32 != 0,
                })
            })
            .collect()
    }

    /// Runs one block. `input[ch]` is copied to every input channel of the
    /// main bus (cycled); returns the main output bus, one Vec per channel.
    pub fn process(
        &self,
        input: &[Vec<f32>],
        changes: Option<&ComWrapper<ParamChanges>>,
    ) -> Result<Vec<Vec<f32>>> {
        self.process_with_notes(input, changes, None)
    }

    /// As [`Instance::process`], also sending note events to instruments.
    pub fn process_with_notes(
        &self,
        input: &[Vec<f32>],
        changes: Option<&ComWrapper<ParamChanges>>,
        notes: Option<&ComWrapper<EventList>>,
    ) -> Result<Vec<Vec<f32>>> {
        let n = self.block;
        let mut in_bufs: Vec<Vec<Vec<f32>>> = self
            .inputs
            .iter()
            .enumerate()
            .map(|(b, bus)| {
                (0..bus.channels as usize)
                    .map(|ch| {
                        if b == 0 && !input.is_empty() {
                            input[ch % input.len()].clone()
                        } else {
                            vec![0.0; n]
                        }
                    })
                    .collect()
            })
            .collect();
        let mut out_bufs: Vec<Vec<Vec<f32>>> = self
            .outputs
            .iter()
            .map(|bus| vec![vec![0.0f32; n]; bus.channels as usize])
            .collect();
        let mut in_ptrs: Vec<Vec<*mut f32>> = in_bufs
            .iter_mut()
            .map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        let mut out_ptrs: Vec<Vec<*mut f32>> = out_bufs
            .iter_mut()
            .map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        let bus = |ptrs: &mut Vec<*mut f32>| AudioBusBuffers {
            numChannels: ptrs.len() as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: ptrs.as_mut_ptr(),
            },
        };
        let mut ins: Vec<AudioBusBuffers> = in_ptrs.iter_mut().map(bus).collect();
        let mut outs: Vec<AudioBusBuffers> = out_ptrs.iter_mut().map(bus).collect();

        let changes_com = changes.map(|c| c.to_com_ptr::<IParameterChanges>().unwrap());
        let notes_com = notes.map(|e| e.to_com_ptr::<IEventList>().unwrap());
        let out_changes = ComWrapper::new(ParamChanges::default());
        let out_changes_ptr = out_changes.to_com_ptr::<IParameterChanges>().unwrap();

        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: n as i32,
            numInputs: ins.len() as i32,
            numOutputs: outs.len() as i32,
            inputs: if ins.is_empty() {
                ptr::null_mut()
            } else {
                ins.as_mut_ptr()
            },
            outputs: if outs.is_empty() {
                ptr::null_mut()
            } else {
                outs.as_mut_ptr()
            },
            inputParameterChanges: changes_com.as_ref().map_or(ptr::null_mut(), |c| c.as_ptr()),
            outputParameterChanges: out_changes_ptr.as_ptr(),
            inputEvents: notes_com.as_ref().map_or(ptr::null_mut(), |e| e.as_ptr()),
            outputEvents: ptr::null_mut(),
            processContext: ptr::null_mut(),
        };
        check(unsafe { self.processor.process(&mut data) }, "process")?;
        Ok(out_bufs.into_iter().next().unwrap_or_default())
    }

    pub fn save_state(&self) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
        let s = ComWrapper::new(MemStream::default());
        let p = s.to_com_ptr::<IBStream>().unwrap();
        check(
            unsafe { self.component.getState(p.as_ptr()) },
            "component getState",
        )?;
        let ctl = self
            .controller
            .as_ref()
            .filter(|_| self.separate_controller)
            .and_then(|c| {
                let s = ComWrapper::new(MemStream::default());
                let p = s.to_com_ptr::<IBStream>().unwrap();
                (unsafe { c.getState(p.as_ptr()) } == kResultOk).then(|| s.bytes())
            });
        Ok((s.bytes(), ctl))
    }

    pub fn load_state(&self, component: &[u8], controller: Option<&[u8]>) -> Result<()> {
        let s = ComWrapper::new(MemStream::with_bytes(component.to_vec()));
        let p = s.to_com_ptr::<IBStream>().unwrap();
        check(
            unsafe { self.component.setState(p.as_ptr()) },
            "component setState",
        )?;
        self.sync_controller();
        if let (Some(bytes), Some(c)) = (controller, &self.controller) {
            let s = ComWrapper::new(MemStream::with_bytes(bytes.to_vec()));
            let p = s.to_com_ptr::<IBStream>().unwrap();
            unsafe { c.setState(p.as_ptr()) };
        }
        Ok(())
    }

    pub fn create_editor(&self) -> Option<ComPtr<IPlugView>> {
        let c = self.controller.as_ref()?;
        let editor: &CStr = c"editor";
        unsafe { ComPtr::from_raw(c.createView(editor.as_ptr())) }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            if self.active {
                self.processor.setProcessing(0);
                self.component.setActive(0);
            }
            if let Some((a, b)) = self.connections.take() {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(ptr::null_mut());
                if self.separate_controller {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}
