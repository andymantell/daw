// SPDX-License-Identifier: GPL-3.0-or-later
//! [`ROUTER_NAME`]: a test effect with more than one bus, a delay that
//! changes while it plays, and an older version of itself it replaces.
//!
//! Inputs: the main stereo input, and a stereo side input (a sidechain).
//! Outputs: the main stereo output, and a stereo extra output. The main
//! output is the main input and the extra output is the side input, each
//! delayed by the plugin's latency, so a host's bus handling can be
//! heard: a side bus that is not switched on gives silence.
//!
//! One parameter, "Look-ahead" (id 0, two steps): off adds no delay,
//! on adds [`ROUTER_LOOKAHEAD`] frames. Changing it from the controller
//! asks the host to restart the plugin; the new delay applies, and is
//! reported, once the host has stopped and started it again, as VST 3
//! asks. The component is its own controller.
//!
//! It declares that it replaces [`OLD_ROUTER_CID`], through a
//! compatibility class, and maps that version's parameter 7 to its own
//! parameter 0.

use std::ffi::c_char;
use std::ptr;
use std::slice;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, uid};

use crate::{read_exact, write_all, write_wstr};

pub const ROUTER_NAME: &str = "DAW Test Router";
pub const ROUTER_CID: TUID = uid(0x5D4A_1C01, 0x9E3B_4F6A, 0x8C21_7B0D, 0x3E5F_B004);
/// The class an "older version" of the router had.
pub const OLD_ROUTER_CID: TUID = uid(0x5D4A_1C01, 0x9E3B_4F6A, 0x8C21_7B0D, 0x3E5F_B0FF);
pub const COMPAT_CID: TUID = uid(0x5D4A_1C01, 0x9E3B_4F6A, 0x8C21_7B0D, 0x3E5F_B005);
/// Frames of delay with "Look-ahead" on.
pub const ROUTER_LOOKAHEAD: u32 = 32;
/// The older version's id for "Look-ahead".
pub const OLD_LOOKAHEAD_PARAM: u32 = 7;

const LINE: usize = ROUTER_LOOKAHEAD as usize;

/// A class ID as the VST 3 SDK writes it in text: on Windows, where
/// IDs are laid out as COM GUIDs, the first three fields are read as
/// little-endian numbers; elsewhere the bytes in order.
pub fn sdk_string(cid: &TUID) -> String {
    let b: Vec<u8> = cid.iter().map(|&c| c as u8).collect();
    if cfg!(windows) {
        format!(
            "{:08X}{:04X}{:04X}{}",
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
            b[8..]
                .iter()
                .map(|x| format!("{x:02X}"))
                .collect::<String>()
        )
    } else {
        b.iter().map(|x| format!("{x:02X}")).collect()
    }
}

pub(crate) struct Router {
    /// "Look-ahead" as last set, applied when next activated.
    wanted: AtomicBool,
    /// The delay in use now, in frames.
    latency: AtomicU32,
    /// Which buses the host switched on: input bits from 0, output bits
    /// from 8.
    active: AtomicU32,
    /// The host's component handler, while it has set one. The host
    /// keeps it alive until it sets another.
    handler: AtomicUsize,
    /// Delay lines for the two outputs' channels, and where they are.
    lines: Mutex<([[f32; LINE]; 4], usize)>,
}

impl Router {
    pub(crate) fn new() -> Router {
        Router {
            wanted: AtomicBool::new(false),
            latency: AtomicU32::new(0),
            active: AtomicU32::new(0),
            handler: AtomicUsize::new(0),
            lines: Mutex::new(([[0.0; LINE]; 4], 0)),
        }
    }
}

impl Class for Router {
    type Interfaces = (IComponent, IAudioProcessor, IEditController, IRemapParamID);
}

impl IPluginBaseTrait for Router {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

/// Bus `index` going `dir`: its name and whether it is the main one.
fn bus(dir: BusDirection, index: i32) -> Option<(&'static str, bool)> {
    let input = dir == BusDirections_::kInput as BusDirection;
    Some(match (input, index) {
        (true, 0) => ("Input", true),
        (true, 1) => ("Sidechain", false),
        (false, 0) => ("Output", true),
        (false, 1) => ("Extra output", false),
        _ => return None,
    })
}

impl IComponentTrait for Router {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, _dir: BusDirection) -> i32 {
        if media == MediaTypes_::kAudio as MediaType {
            2
        } else {
            0
        }
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        info: *mut BusInfo,
    ) -> tresult {
        let Some((name, main)) =
            bus(dir, index).filter(|_| media == MediaTypes_::kAudio as MediaType)
        else {
            return kInvalidArgument;
        };
        let info = unsafe { &mut *info };
        info.mediaType = media;
        info.direction = dir;
        info.channelCount = 2;
        write_wstr(&mut info.name, name);
        if main {
            info.busType = BusTypes_::kMain as BusType;
            info.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        } else {
            info.busType = BusTypes_::kAux as BusType;
            info.flags = 0;
        }
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, m: MediaType, d: BusDirection, i: i32, on: TBool) -> tresult {
        if m != MediaTypes_::kAudio as MediaType || bus(d, i).is_none() {
            return kInvalidArgument;
        }
        let shift = if d == BusDirections_::kInput as BusDirection {
            0
        } else {
            8
        };
        let bit = 1 << (shift + i);
        if on != 0 {
            self.active.fetch_or(bit, Ordering::Relaxed);
        } else {
            self.active.fetch_and(!bit, Ordering::Relaxed);
        }
        kResultOk
    }
    unsafe fn setActive(&self, state: TBool) -> tresult {
        if state != 0 {
            let l = if self.wanted.load(Ordering::Relaxed) {
                ROUTER_LOOKAHEAD
            } else {
                0
            };
            self.latency.store(l, Ordering::Relaxed);
            if let Ok(mut lines) = self.lines.lock() {
                *lines = ([[0.0; LINE]; 4], 0);
            }
        }
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let mut b = [0u8; 1];
        if !unsafe { read_exact(state, &mut b) } {
            return kResultFalse;
        }
        self.wanted.store(b[0] != 0, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let b = [self.wanted.load(Ordering::Relaxed) as u8];
        if unsafe { write_all(state, &b) } {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IAudioProcessorTrait for Router {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        let all_stereo = |p: *mut SpeakerArrangement, n: i32| {
            n == 2
                && unsafe { slice::from_raw_parts(p, 2) }
                    .iter()
                    .all(|&a| a == SpeakerArr::kStereo)
        };
        if all_stereo(inputs, num_ins) && all_stereo(outputs, num_outs) {
            kResultTrue
        } else {
            kResultFalse
        }
    }
    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if bus(dir, index).is_none() {
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
        self.latency.load(Ordering::Relaxed)
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = unsafe { &*data };
        if data.numInputs < 2 || data.numOutputs < 2 || data.numSamples <= 0 {
            return kResultOk;
        }
        let n = data.numSamples as usize;
        let active = self.active.load(Ordering::Relaxed);
        let latency = self.latency.load(Ordering::Relaxed) as usize;
        let ins = unsafe { slice::from_raw_parts(data.inputs, 2) };
        let outs = unsafe { slice::from_raw_parts(data.outputs, 2) };
        let Ok(mut guard) = self.lines.lock() else {
            return kResultFalse;
        };
        let (lines, pos) = &mut *guard;
        // (input bus, output bus) pairs: main to main, side to extra.
        for (k, (i, o)) in [(0usize, 0usize), (1, 1)].into_iter().enumerate() {
            let (inb, outb) = (&ins[i], &outs[o]);
            if inb.numChannels < 2 || outb.numChannels < 2 {
                continue;
            }
            let out = unsafe { slice::from_raw_parts(outb.__field0.channelBuffers32, 2) };
            let src = unsafe { slice::from_raw_parts(inb.__field0.channelBuffers32, 2) };
            let heard = active & (1 << i) != 0;
            for ch in 0..2 {
                let output = unsafe { slice::from_raw_parts_mut(out[ch], n) };
                let input = unsafe { slice::from_raw_parts(src[ch], n) };
                let line = &mut lines[2 * k + ch];
                for (j, o) in output.iter_mut().enumerate() {
                    let x = if heard { input[j] } else { 0.0 };
                    if latency == 0 {
                        *o = x;
                    } else {
                        let slot = (*pos + j) % latency;
                        *o = line[slot];
                        line[slot] = x;
                    }
                }
            }
        }
        if latency > 0 {
            *pos = (*pos + n) % latency;
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IEditControllerTrait for Router {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
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
        write_wstr(&mut info.title, "Look-ahead");
        write_wstr(&mut info.shortTitle, "Look");
        write_wstr(&mut info.units, "");
        info.stepCount = 1;
        info.defaultNormalizedValue = 0.0;
        info.unitId = 0;
        info.flags = 0;
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: u32, v: f64, s: *mut String128) -> tresult {
        if id != 0 {
            return kInvalidArgument;
        }
        write_wstr(unsafe { &mut *s }, if v >= 0.5 { "On" } else { "Off" });
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
        if id == 0 && self.wanted.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        }
    }
    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        if id != 0 {
            return kInvalidArgument;
        }
        let on = v >= 0.5;
        if self.wanted.swap(on, Ordering::Relaxed) != on {
            let h = self.handler.load(Ordering::Relaxed) as *mut IComponentHandler;
            if let Some(h) = unsafe { ComRef::from_raw(h) } {
                unsafe { h.restartComponent(RestartFlags_::kLatencyChanged) };
            }
        }
        kResultOk
    }
    unsafe fn setComponentHandler(&self, h: *mut IComponentHandler) -> tresult {
        self.handler.store(h as usize, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

impl IRemapParamIDTrait for Router {
    unsafe fn getCompatibleParamID(
        &self,
        old_uid: *const TUID,
        old: ParamID,
        new: *mut ParamID,
    ) -> tresult {
        if unsafe { *old_uid } == OLD_ROUTER_CID && old == OLD_LOOKAHEAD_PARAM {
            unsafe { *new = 0 };
            kResultTrue
        } else {
            kResultFalse
        }
    }
}

// ---------------------------------------------------------------------------

/// Says which older classes the router replaces, in the JSON5 the VST 3
/// SDK describes (comments and trailing commas included, to test the
/// host's reading of it).
pub(crate) struct Compat;

impl Class for Compat {
    type Interfaces = (IPluginCompatibility,);
}

impl IPluginCompatibilityTrait for Compat {
    unsafe fn getCompatibilityJSON(&self, stream: *mut IBStream) -> tresult {
        let json = format!(
            "[\n  {{\n    \"New\": \"{}\",\n    \"Old\": [\n      \"{}\", // the older router\n    ],\n  }},\n]\n",
            sdk_string(&ROUTER_CID),
            sdk_string(&OLD_ROUTER_CID),
        );
        if unsafe { write_all(stream, json.as_bytes()) } {
            kResultTrue
        } else {
            kResultFalse
        }
    }
}
