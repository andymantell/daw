// SPDX-License-Identifier: GPL-3.0-or-later
//! The test instrument in the CLAP format, [`CLAP_SYNTH_ID`], beside
//! the test effect in the same `.clap` file.
//!
//! - No audio input; one note input taking CLAP notes and MIDI.
//!   Outputs: the main stereo output and a stereo extra output, "Drums".
//! - Each note-on writes one sample of +velocity×gain at its frame, and
//!   each note-off −velocity×gain, on both channels, as the VST 3 test
//!   instrument does. Notes on channel 10 (9 from 0) go to "Drums".
//! - MIDI controller 7 sets the gain (value / 127) from that frame on;
//!   a MIDI note-on or -off plays like a CLAP one.
//!
//! Written for this project from the public CLAP headers.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;

use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::note_ports::*;
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::*;
use clap_sys::version::CLAP_VERSION;

use crate::clap::{Features, channel, write_name};

pub const CLAP_SYNTH_ID: &str = "org.daw.test-synth";
pub const CLAP_SYNTH_NAME: &str = "DAW Test Synth (CLAP)";

pub(crate) const ID: &CStr = c"org.daw.test-synth";

static FEATURES: Features = Features([
    c"instrument".as_ptr(),
    c"synthesizer".as_ptr(),
    c"stereo".as_ptr(),
    ptr::null(),
]);

pub(crate) static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr(),
    name: c"DAW Test Synth (CLAP)".as_ptr(),
    vendor: c"DAW project".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"0.0.0".as_ptr(),
    description: c"A test instrument".as_ptr(),
    features: &FEATURES.0 as *const _ as *const *const c_char,
};

#[repr(C)]
struct Synth {
    clap: clap_plugin,
    gain: f32,
}

fn me<'a>(p: *const clap_plugin) -> &'a mut Synth {
    unsafe { &mut *(p as *mut Synth) }
}

unsafe extern "C" fn init(_p: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn destroy(p: *const clap_plugin) {
    drop(unsafe { Box::from_raw(p as *mut Synth) });
}

unsafe extern "C" fn activate(_p: *const clap_plugin, _rate: f64, _min: u32, _max: u32) -> bool {
    true
}

unsafe extern "C" fn deactivate(_p: *const clap_plugin) {}

unsafe extern "C" fn start_processing(_p: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn stop_processing(_p: *const clap_plugin) {}

unsafe extern "C" fn reset(_p: *const clap_plugin) {}

unsafe extern "C" fn process(
    p: *const clap_plugin,
    proc_: *const clap_process,
) -> clap_process_status {
    let s = me(p);
    let pr = unsafe { &*proc_ };
    let n = pr.frames_count as usize;
    if pr.audio_outputs_count < 1 {
        return CLAP_PROCESS_ERROR;
    }
    let outs =
        unsafe { std::slice::from_raw_parts(pr.audio_outputs, pr.audio_outputs_count as usize) };
    for b in outs {
        for c in 0..2 {
            if let Some(ch) = unsafe { channel(b, c, n) } {
                ch.fill(0.0);
            }
        }
    }
    let Some(ev) = (unsafe { pr.in_events.as_ref() }) else {
        return CLAP_PROCESS_CONTINUE;
    };
    let (Some(size), Some(get)) = (ev.size, ev.get) else {
        return CLAP_PROCESS_CONTINUE;
    };
    for i in 0..unsafe { size(pr.in_events) } {
        let h = unsafe { get(pr.in_events, i) };
        let Some(hh) = (unsafe { h.as_ref() }) else {
            continue;
        };
        if hh.space_id != CLAP_CORE_EVENT_SPACE_ID || hh.time as usize >= n {
            continue;
        }
        let (v, ch) = match hh.type_ {
            CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
                let e = unsafe { &*(h as *const clap_event_note) };
                let v = e.velocity as f32;
                let v = if hh.type_ == CLAP_EVENT_NOTE_ON {
                    v
                } else {
                    -v
                };
                (v, e.channel)
            }
            CLAP_EVENT_MIDI => {
                let e = unsafe { &*(h as *const clap_event_midi) };
                let [status, d1, d2] = e.data;
                let ch = (status & 15) as i16;
                match status & 0xf0 {
                    0xb0 if d1 == 7 => {
                        s.gain = d2 as f32 / 127.0;
                        continue;
                    }
                    0x90 if d2 > 0 => (d2 as f32 / 127.0, ch),
                    0x80 | 0x90 => (-(d2 as f32) / 127.0, ch),
                    _ => continue,
                }
            }
            _ => continue,
        };
        let bus = if ch == 9 && outs.len() > 1 { 1 } else { 0 };
        for c in 0..2 {
            if let Some(o) = unsafe { channel(&outs[bus], c, n) } {
                o[hh.time as usize] += v * s.gain;
            }
        }
    }
    CLAP_PROCESS_CONTINUE
}

static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(ports_count),
    get: Some(ports_get),
};

unsafe extern "C" fn ports_count(_p: *const clap_plugin, input: bool) -> u32 {
    if input { 0 } else { 2 }
}

unsafe extern "C" fn ports_get(
    _p: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if input || index > 1 {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = index;
    write_name(&mut info.name, if index == 0 { "Main" } else { "Drums" });
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

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(notes_count),
    get: Some(notes_get),
};

unsafe extern "C" fn notes_count(_p: *const clap_plugin, input: bool) -> u32 {
    input as u32
}

unsafe extern "C" fn notes_get(
    _p: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    if !input || index != 0 {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    write_name(&mut info.name, "Notes");
    true
}

unsafe extern "C" fn get_extension(_p: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &NOTE_PORTS as *const _ as *const c_void
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn on_main_thread(_p: *const clap_plugin) {}

pub(crate) fn create(_host: *const clap_host) -> *const clap_plugin {
    let s = Box::new(Synth {
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
        gain: 1.0,
    });
    Box::into_raw(s) as *const clap_plugin
}
