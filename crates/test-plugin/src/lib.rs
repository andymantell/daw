// SPDX-License-Identifier: GPL-3.0-or-later
//! A minimal VST 3 effect used to test our plugin host in CI.
//!
//! Stereo in, stereo out. One parameter (id 0, "Gain", normalised value =
//! linear gain). The output is delayed by [`LATENCY`] samples and the
//! plugin reports that latency, so the host's latency handling can be
//! checked. The component's state is the gain as 8 little-endian bytes.
//!
//! A second class, [`SYNTH_NAME`], is a test instrument: no audio input,
//! one note input. Each note-on writes a single sample of +velocity×gain
//! at the event's exact sample offset and each note-off writes
//! −velocity×gain, so the host's note timing can be checked to the sample.
//!
//! A third, [`ROUTER_NAME`] (see [`router`]), has a sidechain input, an
//! extra output, a delay that changes while it plays, and declares that
//! it replaces an older version of itself.
//!
//! Cargo features build broken variants (crash or hang on load or while
//! processing, refuse processing setup) for testing how the host copes.
//!
//! Written for this project from the public VST 3 interface definitions.

#![allow(non_snake_case)]
// VST 3 enum types are signed on some platforms and unsigned on others,
// so casts that are no-ops here are needed elsewhere.
#![allow(clippy::unnecessary_cast)]

use std::ffi::{c_char, c_void};
use std::ptr;
use std::slice;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};

pub mod router;
pub use router::{
    OLD_LOOKAHEAD_PARAM, OLD_ROUTER_CID, ROUTER_CID, ROUTER_LOOKAHEAD, ROUTER_NAME, sdk_string,
};

/// Reported (and real) processing latency in samples.
pub const LATENCY: u32 = 64;
pub const NAME: &str = "DAW Test Gain";
pub const PROCESSOR_CID: TUID = uid(0x5D4A_1C01, 0x9E3B_4F6A, 0x8C21_7B0D, 0x3E5F_B001);
pub const CONTROLLER_CID: TUID = uid(0x5D4A_1C01, 0x9E3B_4F6A, 0x8C21_7B0D, 0x3E5F_B002);
pub const SYNTH_NAME: &str = "DAW Test Synth";
pub const SYNTH_CID: TUID = uid(0x5D4A_1C01, 0x9E3B_4F6A, 0x8C21_7B0D, 0x3E5F_B003);

fn write_cstr(dst: &mut [c_char], s: &str) {
    let n = s.len().min(dst.len().saturating_sub(1));
    for (d, b) in dst.iter_mut().zip(s.bytes().take(n)) {
        *d = b as c_char;
    }
    if let Some(end) = dst.get_mut(n) {
        *end = 0;
    }
}

pub(crate) fn write_wstr(dst: &mut [TChar], s: &str) {
    let max = dst.len().saturating_sub(1);
    let mut n = 0;
    for (d, u) in dst.iter_mut().zip(s.encode_utf16()).take(max) {
        *d = u as TChar;
        n += 1;
    }
    if let Some(end) = dst.get_mut(n) {
        *end = 0;
    }
}

pub(crate) unsafe fn read_exact(stream: *mut IBStream, buf: &mut [u8]) -> bool {
    let Some(stream) = (unsafe { ComRef::from_raw(stream) }) else {
        return false;
    };
    let mut got = 0;
    let r = unsafe { stream.read(buf.as_mut_ptr().cast(), buf.len() as i32, &mut got) };
    r == kResultOk && got as usize == buf.len()
}

pub(crate) unsafe fn write_all(stream: *mut IBStream, buf: &[u8]) -> bool {
    let Some(stream) = (unsafe { ComRef::from_raw(stream) }) else {
        return false;
    };
    let mut put = 0;
    let r = unsafe { stream.write(buf.as_ptr() as *mut c_void, buf.len() as i32, &mut put) };
    r == kResultOk && put as usize == buf.len()
}

// ---------------------------------------------------------------------------
// Processor

struct Processor {
    gain: AtomicU64,
    /// Per-channel delay lines of LATENCY samples. Only touched from
    /// process(), so the lock is never contended.
    delay: Mutex<[[f32; LATENCY as usize]; 2]>,
    pos: AtomicU64,
}

impl Class for Processor {
    type Interfaces = (IComponent, IAudioProcessor);
}

impl Processor {
    fn gain(&self) -> f64 {
        f64::from_bits(self.gain.load(Ordering::Relaxed))
    }
}

impl IPluginBaseTrait for Processor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

fn fill_bus(bus: &mut BusInfo, dir: BusDirection, name: &str) {
    bus.mediaType = MediaTypes_::kAudio as MediaType;
    bus.direction = dir;
    bus.channelCount = 2;
    write_wstr(&mut bus.name, name);
    bus.busType = BusTypes_::kMain as BusType;
    bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
}

impl IComponentTrait for Processor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        unsafe { *class_id = CONTROLLER_CID };
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, _dir: BusDirection) -> i32 {
        (media == MediaTypes_::kAudio as MediaType) as i32
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if media != MediaTypes_::kAudio as MediaType || index != 0 {
            return kInvalidArgument;
        }
        let name = if dir == BusDirections_::kInput as BusDirection {
            "Input"
        } else {
            "Output"
        };
        fill_bus(unsafe { &mut *bus }, dir, name);
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        if let Ok(mut d) = self.delay.lock() {
            *d = [[0.0; LATENCY as usize]; 2];
        }
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let mut b = [0u8; 8];
        if !unsafe { read_exact(state, &mut b) } {
            return kResultFalse;
        }
        self.gain
            .store(f64::from_le_bytes(b).to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        if unsafe { write_all(state, &self.gain().to_le_bytes()) } {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IAudioProcessorTrait for Processor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        let ok = num_ins == 1
            && num_outs == 1
            && unsafe { *inputs == SpeakerArr::kStereo && *outputs == SpeakerArr::kStereo };
        if ok { kResultTrue } else { kResultFalse }
    }
    unsafe fn getBusArrangement(
        &self,
        _dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 {
            kResultOk
        } else {
            kNotImplemented
        }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        LATENCY
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        if cfg!(feature = "refuse-setup") {
            return kResultFalse;
        }
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        if cfg!(feature = "crash-on-process") {
            std::process::abort();
        }
        if cfg!(feature = "hang-on-process") {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
        let data = unsafe { &*data };

        // Apply the last queued gain value for this block.
        if let Some(changes) = unsafe { ComRef::from_raw(data.inputParameterChanges) } {
            for i in 0..unsafe { changes.getParameterCount() } {
                let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else {
                    continue;
                };
                let n = unsafe { q.getPointCount() };
                if unsafe { q.getParameterId() } == 0 && n > 0 {
                    let (mut offset, mut value) = (0, 0.0);
                    if unsafe { q.getPoint(n - 1, &mut offset, &mut value) } == kResultOk {
                        self.gain.store(value.to_bits(), Ordering::Relaxed);
                    }
                }
            }
        }

        if data.numInputs < 1 || data.numOutputs < 1 || data.numSamples <= 0 {
            return kResultOk;
        }
        let n = data.numSamples as usize;
        let gain = self.gain() as f32;
        let (inb, outb) = unsafe { (&*data.inputs, &*data.outputs) };
        if inb.numChannels < 2 || outb.numChannels < 2 {
            return kResultOk;
        }
        let ins = unsafe { slice::from_raw_parts(inb.__field0.channelBuffers32, 2) };
        let outs = unsafe { slice::from_raw_parts(outb.__field0.channelBuffers32, 2) };
        let Ok(mut delay) = self.delay.lock() else {
            return kResultFalse;
        };
        let start = self.pos.load(Ordering::Relaxed) as usize;
        for ch in 0..2 {
            let input = unsafe { slice::from_raw_parts(ins[ch], n) };
            let output = unsafe { slice::from_raw_parts_mut(outs[ch], n) };
            let line = &mut delay[ch];
            for (p, (o, &x)) in (start..).zip(output.iter_mut().zip(input)) {
                let slot = p % LATENCY as usize;
                *o = line[slot];
                line[slot] = x * gain;
            }
        }
        self.pos.store((start + n) as u64, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

// ---------------------------------------------------------------------------
// Controller

struct Controller {
    gain: Mutex<f64>,
}

impl Class for Controller {
    type Interfaces = (IEditController,);
}

impl IPluginBaseTrait for Controller {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for Controller {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let mut b = [0u8; 8];
        if !unsafe { read_exact(state, &mut b) } {
            return kResultFalse;
        }
        *self.gain.lock().unwrap() = f64::from_le_bytes(b);
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> i32 {
        1
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.id = 0;
        write_wstr(&mut info.title, "Gain");
        write_wstr(&mut info.shortTitle, "Gain");
        write_wstr(&mut info.units, "");
        info.stepCount = 0;
        info.defaultNormalizedValue = 1.0;
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as i32;
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: u32, v: f64, s: *mut String128) -> tresult {
        if id != 0 {
            return kInvalidArgument;
        }
        write_wstr(unsafe { &mut *s }, &format!("{v:.3}"));
        kResultOk
    }
    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, _id: u32, v: f64) -> f64 {
        v
    }
    unsafe fn plainParamToNormalized(&self, _id: u32, v: f64) -> f64 {
        v
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        if id == 0 {
            *self.gain.lock().unwrap()
        } else {
            0.0
        }
    }
    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        if id != 0 {
            return kInvalidArgument;
        }
        *self.gain.lock().unwrap() = v;
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _h: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

// ---------------------------------------------------------------------------
// Test instrument

struct Synth {
    gain: AtomicU64,
}

impl Class for Synth {
    type Interfaces = (IComponent, IAudioProcessor);
}

impl IPluginBaseTrait for Synth {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Synth {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        unsafe { *class_id = CONTROLLER_CID };
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        let input = dir == BusDirections_::kInput as BusDirection;
        if media == MediaTypes_::kEvent as MediaType {
            input as i32
        } else {
            !input as i32
        }
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        let input = dir == BusDirections_::kInput as BusDirection;
        let bus = unsafe { &mut *bus };
        if index != 0 {
            return kInvalidArgument;
        }
        if media == MediaTypes_::kEvent as MediaType && input {
            bus.mediaType = media;
            bus.direction = dir;
            bus.channelCount = 16;
            write_wstr(&mut bus.name, "Notes");
            bus.busType = BusTypes_::kMain as BusType;
            bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
            kResultOk
        } else if media == MediaTypes_::kAudio as MediaType && !input {
            fill_bus(bus, dir, "Output");
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let mut b = [0u8; 8];
        if !unsafe { read_exact(state, &mut b) } {
            return kResultFalse;
        }
        self.gain
            .store(f64::from_le_bytes(b).to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let g = f64::from_bits(self.gain.load(Ordering::Relaxed));
        if unsafe { write_all(state, &g.to_le_bytes()) } {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IAudioProcessorTrait for Synth {
    unsafe fn setBusArrangements(
        &self,
        _inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        let ok = num_ins == 0 && num_outs == 1 && unsafe { *outputs == SpeakerArr::kStereo };
        if ok { kResultTrue } else { kResultFalse }
    }
    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0 || dir != BusDirections_::kOutput as BusDirection {
            return kInvalidArgument;
        }
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 {
            kResultOk
        } else {
            kNotImplemented
        }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = unsafe { &*data };
        if data.numOutputs < 1 || data.numSamples <= 0 {
            return kResultOk;
        }
        let n = data.numSamples as usize;
        let outb = unsafe { &*data.outputs };
        if outb.numChannels < 2 {
            return kResultOk;
        }
        let outs = unsafe { slice::from_raw_parts(outb.__field0.channelBuffers32, 2) };
        let gain = f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32;
        for &o in outs {
            unsafe { slice::from_raw_parts_mut(o, n) }.fill(0.0);
        }
        let Some(events) = (unsafe { ComRef::from_raw(data.inputEvents) }) else {
            return kResultOk;
        };
        for i in 0..unsafe { events.getEventCount() } {
            let mut e: Event = unsafe { std::mem::zeroed() };
            if unsafe { events.getEvent(i, &mut e) } != kResultOk {
                continue;
            }
            let at = e.sampleOffset;
            if at < 0 || at as usize >= n {
                continue;
            }
            let v = match e.r#type as u32 {
                t if t == Event_::EventTypes_::kNoteOnEvent as u32 => unsafe {
                    e.__field0.noteOn.velocity
                },
                t if t == Event_::EventTypes_::kNoteOffEvent as u32 => {
                    -unsafe { e.__field0.noteOff.velocity }
                }
                _ => continue,
            };
            for &o in outs {
                unsafe { *o.add(at as usize) += v * gain };
            }
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

// ---------------------------------------------------------------------------
// Factory and module entry points

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = unsafe { &mut *info };
        write_cstr(&mut info.vendor, "DAW project (test fixture)");
        write_cstr(&mut info.url, "");
        write_cstr(&mut info.email, "");
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as i32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 {
        5
    }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let info = unsafe { &mut *info };
        let Some((cid, category, name, _)) = class(index) else {
            return kInvalidArgument;
        };
        info.cid = cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        write_cstr(&mut info.category, category);
        write_cstr(&mut info.name, name);
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let cid = unsafe { *(cid as *const TUID) };
        let instance = if cid == PROCESSOR_CID {
            ComWrapper::new(Processor {
                gain: AtomicU64::new(1.0f64.to_bits()),
                delay: Mutex::new([[0.0; LATENCY as usize]; 2]),
                pos: AtomicU64::new(0),
            })
            .to_com_ptr::<FUnknown>()
        } else if cid == SYNTH_CID {
            ComWrapper::new(Synth {
                gain: AtomicU64::new(1.0f64.to_bits()),
            })
            .to_com_ptr::<FUnknown>()
        } else if cid == ROUTER_CID {
            ComWrapper::new(router::Router::new()).to_com_ptr::<FUnknown>()
        } else if cid == router::COMPAT_CID {
            ComWrapper::new(router::Compat).to_com_ptr::<FUnknown>()
        } else if cid == CONTROLLER_CID {
            ComWrapper::new(Controller {
                gain: Mutex::new(1.0),
            })
            .to_com_ptr::<FUnknown>()
        } else {
            None
        };
        let Some(instance) = instance else {
            unsafe { *obj = ptr::null_mut() };
            return kInvalidArgument;
        };
        let p = instance.as_ptr();
        unsafe { ((*(*p).vtbl).queryInterface)(p, iid as *mut TUID, obj) }
    }
}

/// Class `index`: ID, category, name and sub-categories.
fn class(index: i32) -> Option<(TUID, &'static str, &'static str, &'static str)> {
    Some(match index {
        0 => (PROCESSOR_CID, "Audio Module Class", NAME, "Fx"),
        1 => (CONTROLLER_CID, "Component Controller Class", NAME, ""),
        2 => (
            SYNTH_CID,
            "Audio Module Class",
            SYNTH_NAME,
            "Instrument|Synth",
        ),
        3 => (ROUTER_CID, "Audio Module Class", ROUTER_NAME, "Fx|Tools"),
        4 => (
            router::COMPAT_CID,
            "Plugin Compatibility Class",
            "DAW Test Compatibility",
            "",
        ),
        _ => return None,
    })
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let info = unsafe { &mut *info };
        let Some((cid, category, name, sub)) = class(index) else {
            return kInvalidArgument;
        };
        info.cid = cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        write_cstr(&mut info.category, category);
        write_cstr(&mut info.name, name);
        info.classFlags = 0;
        write_cstr(&mut info.subCategories, sub);
        write_cstr(&mut info.vendor, "");
        write_cstr(&mut info.version, "0.0.0");
        write_cstr(&mut info.sdkVersion, "VST 3.7");
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "system" fn BundleEntry(_bundle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "system" fn BundleExit() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
extern "system" fn ModuleEntry(_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
extern "system" fn ModuleExit() -> bool {
    true
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    if cfg!(feature = "crash-on-load") {
        std::process::abort();
    }
    if cfg!(feature = "hang-on-load") {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map_or(ptr::null_mut(), |p| p.into_raw())
}
