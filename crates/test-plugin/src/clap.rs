// SPDX-License-Identifier: GPL-3.0-or-later
//! The test effect in the CLAP format, for the CLAP host's tests.
//!
//! The same library is copied to a `.clap` file, which exports
//! `clap_entry` beside the VST 3 entry point. One plugin, [`CLAP_ID`]:
//!
//! - Inputs: the main stereo input and a stereo sidechain. Outputs: the
//!   main stereo output and a stereo extra output.
//! - Main output: the main input delayed by [`LATENCY`] frames (twice
//!   that with "Lookahead" on) times "Gain", plus the sidechain.
//!   Extra output: half the sidechain.
//! - Parameters: [`GAIN_PARAM`] "Gain", 0 to 2; [`LEVEL_PARAM`] "Level",
//!   which the plugin sets itself to each block's input peak and sends
//!   back as an output event, as a plugin's own window would; and
//!   [`LOOKAHEAD_PARAM`] "Lookahead", off or on, which changes its delay
//!   and so asks the host to restart it.
//! - State: the gain as 8 little-endian bytes, then lookahead as 1 byte.
//!
//! Written for this project from the public CLAP headers.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::latency::*;
use clap_sys::ext::params::*;
use clap_sys::ext::state::*;
use clap_sys::factory::plugin_factory::*;
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::*;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

use crate::LATENCY;

pub const CLAP_ID: &str = "org.daw.test-gain";
pub const CLAP_NAME: &str = "DAW Test Gain (CLAP)";
pub const GAIN_PARAM: u32 = 0;
pub const LEVEL_PARAM: u32 = 1;
pub const LOOKAHEAD_PARAM: u32 = 2;

const ID: &CStr = c"org.daw.test-gain";

struct Features([*const c_char; 4]);
// Pointers to static strings.
unsafe impl Sync for Features {}

static FEATURES: Features = Features([
    c"audio-effect".as_ptr(),
    c"utility".as_ptr(),
    c"stereo".as_ptr(),
    ptr::null(),
]);

static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr(),
    name: c"DAW Test Gain (CLAP)".as_ptr(),
    vendor: c"DAW project".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"0.0.0".as_ptr(),
    description: c"A test effect".as_ptr(),
    features: &FEATURES.0 as *const _ as *const *const c_char,
};

/// Room for the longest delay.
const RING: usize = 4 * LATENCY as usize;

#[repr(C)]
struct Gain {
    clap: clap_plugin,
    host: *const clap_host,
    gain: AtomicU64,
    lookahead: AtomicBool,
    /// The delay since activation.
    latency: u32,
    ring: [[f32; RING]; 2],
    pos: usize,
    level: AtomicU64,
}

fn me<'a>(p: *const clap_plugin) -> &'a mut Gain {
    unsafe { &mut *(p as *mut Gain) }
}

impl Gain {
    fn gain(&self) -> f64 {
        f64::from_bits(self.gain.load(Ordering::Relaxed))
    }
    fn set(&self, id: u32, v: f64) {
        match id {
            GAIN_PARAM => self
                .gain
                .store(v.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed),
            LOOKAHEAD_PARAM => {
                let on = v >= 0.5;
                if self.lookahead.swap(on, Ordering::Relaxed) != on {
                    // The delay changes once restarted.
                    let host = unsafe { &*self.host };
                    if let Some(f) = host.request_restart {
                        unsafe { f(self.host) };
                    }
                }
            }
            _ => {}
        }
    }
    fn value(&self, id: u32) -> Option<f64> {
        match id {
            GAIN_PARAM => Some(self.gain()),
            LEVEL_PARAM => Some(f64::from_bits(self.level.load(Ordering::Relaxed))),
            LOOKAHEAD_PARAM => Some(self.lookahead.load(Ordering::Relaxed) as u8 as f64),
            _ => None,
        }
    }
}

unsafe extern "C" fn init(_p: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn destroy(p: *const clap_plugin) {
    drop(unsafe { Box::from_raw(p as *mut Gain) });
}

unsafe extern "C" fn activate(p: *const clap_plugin, _rate: f64, _min: u32, _max: u32) -> bool {
    let g = me(p);
    g.latency = if g.lookahead.load(Ordering::Relaxed) {
        2 * LATENCY
    } else {
        LATENCY
    };
    g.ring = [[0.0; RING]; 2];
    g.pos = 0;
    true
}

unsafe extern "C" fn deactivate(_p: *const clap_plugin) {}

unsafe extern "C" fn start_processing(_p: *const clap_plugin) -> bool {
    !cfg!(feature = "refuse-setup")
}

unsafe extern "C" fn stop_processing(_p: *const clap_plugin) {}

unsafe extern "C" fn reset(_p: *const clap_plugin) {}

/// Channel `c` of `buf`, if it has one.
unsafe fn channel<'a>(buf: &clap_audio_buffer, c: usize, n: usize) -> Option<&'a mut [f32]> {
    if buf.data32.is_null() || c >= buf.channel_count as usize {
        return None;
    }
    let ch = unsafe { *buf.data32.add(c) };
    (!ch.is_null()).then(|| unsafe { std::slice::from_raw_parts_mut(ch, n) })
}

unsafe fn handle_events(g: &Gain, events: *const clap_input_events) {
    let Some(ev) = (unsafe { events.as_ref() }) else {
        return;
    };
    let (Some(size), Some(get)) = (ev.size, ev.get) else {
        return;
    };
    for i in 0..unsafe { size(events) } {
        let h = unsafe { get(events, i) };
        let Some(hh) = (unsafe { h.as_ref() }) else {
            continue;
        };
        if hh.space_id == CLAP_CORE_EVENT_SPACE_ID && hh.type_ == CLAP_EVENT_PARAM_VALUE {
            let pv = unsafe { &*(h as *const clap_event_param_value) };
            g.set(pv.param_id, pv.value);
        }
    }
}

unsafe extern "C" fn process(
    p: *const clap_plugin,
    proc_: *const clap_process,
) -> clap_process_status {
    if cfg!(feature = "crash-on-process") {
        std::process::abort();
    }
    if cfg!(feature = "hang-on-process") {
        loop {
            std::thread::park();
        }
    }
    let g = me(p);
    let pr = unsafe { &*proc_ };
    let n = pr.frames_count as usize;
    unsafe { handle_events(g, pr.in_events) };
    if pr.audio_inputs_count < 1 || pr.audio_outputs_count < 1 {
        return CLAP_PROCESS_ERROR;
    }
    let ins =
        unsafe { std::slice::from_raw_parts(pr.audio_inputs, pr.audio_inputs_count as usize) };
    let outs =
        unsafe { std::slice::from_raw_parts(pr.audio_outputs, pr.audio_outputs_count as usize) };
    let gain = g.gain() as f32;
    let d = g.latency as usize;
    let mut peak = 0.0f32;
    for c in 0..2 {
        let (Some(input), Some(out)) = (unsafe { channel(&ins[0], c, n) }, unsafe {
            channel(&outs[0], c, n)
        }) else {
            return CLAP_PROCESS_ERROR;
        };
        let key = ins.get(1).and_then(|b| unsafe { channel(b, c, n) });
        let mut pos = g.pos;
        for i in 0..n {
            let x = input[i];
            peak = peak.max(x.abs());
            g.ring[c][pos] = x;
            let back = (pos + RING - d) % RING;
            let k = key.as_ref().map_or(0.0, |k| k[i]);
            out[i] = g.ring[c][back] * gain + k;
            pos = (pos + 1) % RING;
        }
        if let Some(extra) = outs.get(1).and_then(|b| unsafe { channel(b, c, n) }) {
            for (i, o) in extra.iter_mut().enumerate() {
                *o = key.as_ref().map_or(0.0, |k| 0.5 * k[i]);
            }
        }
    }
    g.pos = (g.pos + n) % RING;
    g.level.store((peak as f64).to_bits(), Ordering::Relaxed);
    if let Some(out) = unsafe { pr.out_events.as_ref() }
        && let Some(push) = out.try_push
    {
        let ev = clap_event_param_value {
            header: clap_event_header {
                size: size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: LEVEL_PARAM,
            cookie: ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: peak as f64,
        };
        unsafe { push(pr.out_events, &ev.header) };
    }
    CLAP_PROCESS_CONTINUE
}

// ---------------------------------------------------------------------------
// Extensions

static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(ports_count),
    get: Some(ports_get),
};

unsafe extern "C" fn ports_count(_p: *const clap_plugin, _input: bool) -> u32 {
    2
}

fn write_name(dst: &mut [c_char], s: &str) {
    for (d, b) in dst.iter_mut().zip(s.bytes().chain(Some(0))) {
        *d = b as c_char;
    }
}

unsafe extern "C" fn ports_get(
    _p: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if index > 1 {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = index;
    let name = match (index, input) {
        (0, _) => "Main",
        (_, true) => "Sidechain",
        (_, false) => "Aux",
    };
    write_name(&mut info.name, name);
    info.flags = if index == 0 {
        CLAP_AUDIO_PORT_IS_MAIN
    } else {
        0
    };
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = clap_sys::id::CLAP_INVALID_ID;
    true
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_info),
    get_value: Some(params_value),
    value_to_text: Some(params_text),
    text_to_value: None,
    flush: Some(params_flush),
};

unsafe extern "C" fn params_count(_p: *const clap_plugin) -> u32 {
    3
}

unsafe extern "C" fn params_info(
    _p: *const clap_plugin,
    i: u32,
    info: *mut clap_param_info,
) -> bool {
    let info = unsafe { &mut *info };
    let (name, flags, max, default) = match i {
        0 => ("Gain", CLAP_PARAM_IS_AUTOMATABLE, 2.0, 1.0),
        1 => ("Level", CLAP_PARAM_IS_READONLY, 1.0, 0.0),
        2 => ("Lookahead", CLAP_PARAM_IS_STEPPED, 1.0, 0.0),
        _ => return false,
    };
    info.id = i as clap_id;
    info.flags = flags;
    info.cookie = ptr::null_mut();
    write_name(&mut info.name, name);
    write_name(&mut info.module, "");
    info.min_value = 0.0;
    info.max_value = max;
    info.default_value = default;
    true
}

unsafe extern "C" fn params_value(p: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    match me(p).value(id) {
        Some(v) => {
            unsafe { *out = v };
            true
        }
        None => false,
    }
}

unsafe extern "C" fn params_text(
    _p: *const clap_plugin,
    id: clap_id,
    v: f64,
    buf: *mut c_char,
    cap: u32,
) -> bool {
    let s = match id {
        GAIN_PARAM => format!("{:.2}x", v),
        LEVEL_PARAM => format!("{v:.2}"),
        LOOKAHEAD_PARAM => if v >= 0.5 { "On" } else { "Off" }.to_string(),
        _ => return false,
    };
    let dst = unsafe { std::slice::from_raw_parts_mut(buf, cap as usize) };
    if dst.is_empty() {
        return false;
    }
    let n = s.len().min(dst.len() - 1);
    write_name(&mut dst[..=n], &s[..n]);
    true
}

unsafe extern "C" fn params_flush(
    p: *const clap_plugin,
    ins: *const clap_input_events,
    _out: *const clap_output_events,
) {
    unsafe { handle_events(me(p), ins) };
}

static STATE: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

unsafe extern "C" fn state_save(p: *const clap_plugin, s: *const clap_ostream) -> bool {
    let g = me(p);
    let mut bytes = g.gain().to_le_bytes().to_vec();
    bytes.push(g.lookahead.load(Ordering::Relaxed) as u8);
    let st = unsafe { &*s };
    let Some(write) = st.write else {
        return false;
    };
    let mut done = 0;
    while done < bytes.len() {
        let n = unsafe {
            write(
                s,
                bytes[done..].as_ptr() as *const c_void,
                (bytes.len() - done) as u64,
            )
        };
        if n <= 0 {
            return false;
        }
        done += n as usize;
    }
    true
}

unsafe extern "C" fn state_load(p: *const clap_plugin, s: *const clap_istream) -> bool {
    let st = unsafe { &*s };
    let Some(read) = st.read else {
        return false;
    };
    let mut bytes = [0u8; 9];
    let mut got = 0;
    while got < bytes.len() {
        let n = unsafe {
            read(
                s,
                bytes[got..].as_mut_ptr() as *mut c_void,
                (bytes.len() - got) as u64,
            )
        };
        if n <= 0 {
            break;
        }
        got += n as usize;
    }
    if got < 8 {
        return false;
    }
    let g = me(p);
    let gain = f64::from_le_bytes(bytes[..8].try_into().unwrap());
    g.gain.store(gain.to_bits(), Ordering::Relaxed);
    g.lookahead
        .store(got > 8 && bytes[8] != 0, Ordering::Relaxed);
    true
}

static LATENCY_EXT: clap_plugin_latency = clap_plugin_latency {
    get: Some(latency_get),
};

unsafe extern "C" fn latency_get(p: *const clap_plugin) -> u32 {
    me(p).latency
}

unsafe extern "C" fn get_extension(_p: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &LATENCY_EXT as *const _ as *const c_void
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn on_main_thread(_p: *const clap_plugin) {}

// ---------------------------------------------------------------------------
// The factory and entry

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

unsafe extern "C" fn factory_count(_f: *const clap_plugin_factory) -> u32 {
    1
}

unsafe extern "C" fn factory_descriptor(
    _f: *const clap_plugin_factory,
    i: u32,
) -> *const clap_plugin_descriptor {
    if i == 0 { &DESCRIPTOR } else { ptr::null() }
}

unsafe extern "C" fn factory_create(
    _f: *const clap_plugin_factory,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    if id.is_null() || unsafe { CStr::from_ptr(id) } != ID {
        return ptr::null();
    }
    let g = Box::new(Gain {
        clap: clap_plugin {
            desc: &DESCRIPTOR,
            plugin_data: ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start_processing),
            stop_processing: Some(stop_processing),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(get_extension),
            on_main_thread: Some(on_main_thread),
        },
        host,
        gain: AtomicU64::new(1.0f64.to_bits()),
        lookahead: AtomicBool::new(false),
        latency: LATENCY,
        ring: [[0.0; RING]; 2],
        pos: 0,
        level: AtomicU64::new(0),
    });
    Box::into_raw(g) as *const clap_plugin
}

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_factory(id: *const c_char) -> *const c_void {
    if cfg!(feature = "crash-on-load") {
        std::process::abort();
    }
    if cfg!(feature = "hang-on-load") {
        loop {
            std::thread::park();
        }
    }
    if !id.is_null() && unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as *const c_void
    } else {
        ptr::null()
    }
}

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_factory),
};
