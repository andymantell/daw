// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugins in the open CLAP format (roadmap 3.2), beside VST 3.
//!
//! Built directly on the `clap-sys` bindings (MIT/Apache-2.0) to the
//! CLAP headers (MIT). A `.clap` file is loaded as a [`ClapModule`]; its
//! plugins are listed as [`ClassInfo`]s like VST 3 classes, so scanning,
//! the plugin list, presets and projects treat both formats alike.
//!
//! **Ids.** A CLAP plugin is known by a text id ("com.example.reverb")
//! where a VST 3 class has 16 bytes. The app keeps 16 bytes for every
//! plugin, so a CLAP plugin's are made from its id ([`cid_of`]): the
//! same id always gives the same bytes, on every computer, and the
//! module finds the plugin again by them.
//!
//! **Two sides.** As with VST 3, [`instantiate_with`] gives a
//! [`ClapPlugin`] for the UI thread and a [`ClapProcessor`] for the
//! audio thread. The thread that makes the plugin is its main thread, as
//! CLAP calls it. Parameter changes from the app go to the processor
//! through a lock-free queue and reach the plugin as events with its
//! next block; changes the plugin makes itself (from its own window)
//! come back the same way. CLAP parameters have their own ranges; the
//! app sees them as 0 to 1, as it does VST 3 parameters.
//!
//! **Restarts.** A plugin that asks to be restarted (its delay changed)
//! is deactivated and activated again on the UI thread while the audio
//! side plays silence, as for VST 3. A plugin whose audio ports changed
//! is reported for the app to make again.
//!
//! **Windows.** On Windows the plugin's own window is put inside one of
//! ours ([`crate::clap_editor_win`]). Elsewhere, and on Windows for a
//! plugin that only offers it, the plugin opens a floating window of its
//! own.

#![allow(non_upper_case_globals)]

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::ThreadId;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::gui::*;
use clap_sys::ext::latency::*;
use clap_sys::ext::params::*;
use clap_sys::ext::state::*;
use clap_sys::ext::thread_check::*;
use clap_sys::factory::plugin_factory::*;
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::*;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{CLAP_VERSION, clap_version_is_compatible};
use rtrb::{Consumer, Producer, RingBuffer};
use vst3::Steinberg::TUID;

use daw_engine::graph::Insert;

use crate::module::{ClassInfo, Result};
use crate::plugin::{Bus, BusBufs, Buses, Layout, Param, PluginState};
use crate::proto::Status;

/// The file extension of a CLAP plugin.
pub const EXT: &str = "clap";

/// True for a path that names a CLAP plugin (by its extension).
pub fn is_clap(path: &Path) -> bool {
    path.extension()
        .is_some_and(|x| x.eq_ignore_ascii_case(EXT))
}

/// The 16 bytes the app knows the CLAP plugin `id` by: a 128-bit FNV-1a
/// hash of `clap:` and the id, so it can't be taken for a VST 3 class.
pub fn cid_of(id: &str) -> TUID {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013B;
    let mut h = OFFSET;
    for b in b"clap:".iter().chain(id.as_bytes()) {
        h ^= *b as u128;
        h = h.wrapping_mul(PRIME);
    }
    h.to_be_bytes().map(|b| b as _)
}

/// The binary to load: inside the bundle on macOS, the file itself
/// elsewhere.
pub fn binary_path(path: &Path) -> PathBuf {
    if cfg!(target_os = "macos") && path.is_dir() {
        path.join("Contents")
            .join("MacOS")
            .join(path.file_stem().unwrap_or_default())
    } else {
        path.to_owned()
    }
}

/// Text from a C string the plugin owns; empty for none.
fn text(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

/// Text from a fixed-size C string buffer.
fn text_in(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What a plugin says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub features: Vec<String>,
}

impl Descriptor {
    fn read(d: &clap_plugin_descriptor) -> Descriptor {
        let mut features = Vec::new();
        if !d.features.is_null() {
            for i in 0.. {
                let f = unsafe { *d.features.add(i) };
                if f.is_null() {
                    break;
                }
                features.push(text(f));
            }
        }
        Descriptor {
            id: text(d.id),
            name: text(d.name),
            vendor: text(d.vendor),
            version: text(d.version),
            features,
        }
    }

    /// As a class in the plugin list. CLAP's features become the
    /// categories the list sorts by: "Instrument" for an instrument,
    /// otherwise "Fx", then the plugin's own features.
    pub fn class(&self) -> ClassInfo {
        let instrument = self.features.iter().any(|f| f == "instrument");
        let mut cats = vec![if instrument { "Instrument" } else { "Fx" }.to_string()];
        cats.extend(
            (self.features.iter())
                .filter(|f| !matches!(f.as_str(), "instrument" | "audio-effect"))
                .cloned(),
        );
        ClassInfo {
            cid: cid_of(&self.id),
            name: self.name.clone(),
            category: "Audio Module Class".into(),
            sub_categories: cats.join("|"),
            vendor: self.vendor.clone(),
            replaces: Vec::new(),
        }
    }
}

/// A loaded `.clap` file. Dropping it calls the plugin's `deinit`.
pub struct ClapModule {
    pub path: PathBuf,
    entry: *const clap_plugin_entry,
    factory: *const clap_plugin_factory,
    // Dropped last: unloads the library.
    _lib: libloading::Library,
}

// The entry and factory are made to be used from any thread.
unsafe impl Send for ClapModule {}
unsafe impl Sync for ClapModule {}

impl ClapModule {
    pub fn load(path: &Path) -> Result<ClapModule> {
        let bin = binary_path(path);
        if !bin.is_file() {
            return Err("could not load: no binary for this platform in the bundle".into());
        }
        let lib = unsafe { libloading::Library::new(&bin) }
            .map_err(|e| format!("could not load: {e}"))?;
        let entry = unsafe { lib.get::<*const clap_plugin_entry>(b"clap_entry\0") }
            .map_err(|e| format!("no clap_entry: {e}"))?;
        let entry: *const clap_plugin_entry = *entry;
        let e = unsafe { entry.as_ref() }.ok_or("clap_entry is null")?;
        if !clap_version_is_compatible(e.clap_version) {
            let v = e.clap_version;
            return Err(format!(
                "made for CLAP {}.{}, which this host can't load",
                v.major, v.minor
            ));
        }
        let (Some(init), Some(get_factory)) = (e.init, e.get_factory) else {
            return Err("clap_entry is incomplete".into());
        };
        let c_path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "the plugin's path can't be passed to it")?;
        if !unsafe { init(c_path.as_ptr()) } {
            return Err("the plugin's init failed".into());
        }
        let factory =
            unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) } as *const clap_plugin_factory;
        let module = ClapModule {
            path: path.to_owned(),
            entry,
            factory,
            _lib: lib,
        };
        if factory.is_null() {
            return Err("the plugin has no plugin factory".into());
        }
        Ok(module)
    }

    fn factory(&self) -> &clap_plugin_factory {
        unsafe { &*self.factory }
    }

    /// Every plugin the file offers.
    pub fn descriptors(&self) -> Vec<Descriptor> {
        let f = self.factory();
        let (Some(count), Some(get)) = (f.get_plugin_count, f.get_plugin_descriptor) else {
            return Vec::new();
        };
        let n = unsafe { count(self.factory) };
        (0..n)
            .filter_map(|i| unsafe { get(self.factory, i).as_ref() })
            .map(Descriptor::read)
            .collect()
    }

    pub fn classes(&self) -> Vec<ClassInfo> {
        self.descriptors().iter().map(Descriptor::class).collect()
    }

    /// The plugin id whose class id is `cid`.
    pub fn id_of(&self, cid: &TUID) -> Option<String> {
        self.descriptors()
            .into_iter()
            .find(|d| cid_of(&d.id) == *cid)
            .map(|d| d.id)
    }
}

impl Drop for ClapModule {
    fn drop(&mut self) {
        if let Some(deinit) = unsafe { (*self.entry).deinit } {
            unsafe { deinit() };
        }
    }
}

// ---------------------------------------------------------------------------
// The host side the plugin talks to

thread_local! {
    /// Set while this thread is processing audio for a plugin.
    static AUDIO: Cell<bool> = const { Cell::new(false) };
}

/// Marks this thread as processing audio for as long as it lives.
struct AudioThread(bool);

impl AudioThread {
    fn enter() -> AudioThread {
        AudioThread(AUDIO.replace(true))
    }
}

impl Drop for AudioThread {
    fn drop(&mut self) {
        AUDIO.set(self.0);
    }
}

/// What the plugin has asked of the host, from any thread.
#[derive(Default)]
struct Flags {
    restart: AtomicBool,
    callback: AtomicBool,
    /// Parameter rescan flags asked for.
    rescan: AtomicU32,
    ports: AtomicBool,
    flush: AtomicBool,
    edited: AtomicBool,
    gui_closed: AtomicBool,
    resize: Mutex<Option<(u32, u32)>>,
}

/// The `clap_host` handed to one plugin, with what it has asked for.
/// The plugin's callbacks get a pointer to `clap`, the first field.
#[repr(C)]
struct HostBox {
    clap: clap_host,
    main: ThreadId,
    flags: Flags,
}

const HOST_VERSION: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(s) => s,
        Err(_) => c"0",
    };

impl HostBox {
    fn new() -> Box<HostBox> {
        Box::new(HostBox {
            clap: clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: c"DAW".as_ptr(),
                vendor: c"DAW project".as_ptr(),
                url: c"https://github.com/andymantell/daw".as_ptr(),
                version: HOST_VERSION.as_ptr(),
                get_extension: Some(host_get_extension),
                request_restart: Some(host_request_restart),
                request_process: Some(host_request_process),
                request_callback: Some(host_request_callback),
            },
            main: std::thread::current().id(),
            flags: Flags::default(),
        })
    }
}

/// The host a plugin's callback is about.
fn host_of<'a>(host: *const clap_host) -> Option<&'a HostBox> {
    unsafe { (host as *const HostBox).as_ref() }
}

fn flags_of<'a>(host: *const clap_host) -> Option<&'a Flags> {
    host_of(host).map(|h| &h.flags)
}

unsafe extern "C" fn host_get_extension(
    _host: *const clap_host,
    id: *const c_char,
) -> *const c_void {
    if id.is_null() {
        return ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_THREAD_CHECK {
        &HOST_THREAD_CHECK as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &HOST_PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &HOST_STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &HOST_LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &HOST_AUDIO_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI {
        &HOST_GUI as *const _ as *const c_void
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    if let Some(f) = flags_of(host) {
        f.restart.store(true, Ordering::Relaxed);
    }
}

// The audio side always runs, so there is nothing to wake.
unsafe extern "C" fn host_request_process(_host: *const clap_host) {}

unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    if let Some(f) = flags_of(host) {
        f.callback.store(true, Ordering::Relaxed);
    }
}

static HOST_THREAD_CHECK: clap_host_thread_check = clap_host_thread_check {
    is_main_thread: Some(is_main_thread),
    is_audio_thread: Some(is_audio_thread),
};

unsafe extern "C" fn is_main_thread(host: *const clap_host) -> bool {
    host_of(host).is_some_and(|h| h.main == std::thread::current().id()) && !AUDIO.get()
}

unsafe extern "C" fn is_audio_thread(_host: *const clap_host) -> bool {
    AUDIO.get()
}

static HOST_PARAMS: clap_host_params = clap_host_params {
    rescan: Some(params_rescan),
    clear: Some(params_clear),
    request_flush: Some(params_request_flush),
};

unsafe extern "C" fn params_rescan(host: *const clap_host, flags: clap_param_rescan_flags) {
    if let Some(f) = flags_of(host) {
        f.rescan.fetch_or(flags, Ordering::Relaxed);
    }
}

// The app keeps no automation or modulation of its own for plugins yet.
unsafe extern "C" fn params_clear(_host: *const clap_host, _id: u32, _flags: u32) {}

unsafe extern "C" fn params_request_flush(host: *const clap_host) {
    if let Some(f) = flags_of(host) {
        f.flush.store(true, Ordering::Relaxed);
    }
}

static HOST_STATE: clap_host_state = clap_host_state {
    mark_dirty: Some(state_mark_dirty),
};

unsafe extern "C" fn state_mark_dirty(host: *const clap_host) {
    if let Some(f) = flags_of(host) {
        f.edited.store(true, Ordering::Relaxed);
    }
}

static HOST_LATENCY: clap_host_latency = clap_host_latency {
    changed: Some(latency_changed),
};

// Only called while the plugin is being activated, after which the app
// reads the latency anyway.
unsafe extern "C" fn latency_changed(_host: *const clap_host) {}

static HOST_AUDIO_PORTS: clap_host_audio_ports = clap_host_audio_ports {
    is_rescan_flag_supported: Some(ports_rescan_supported),
    rescan: Some(ports_rescan),
};

unsafe extern "C" fn ports_rescan_supported(_host: *const clap_host, _flag: u32) -> bool {
    true
}

unsafe extern "C" fn ports_rescan(host: *const clap_host, _flags: u32) {
    if let Some(f) = flags_of(host) {
        f.ports.store(true, Ordering::Relaxed);
    }
}

static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(gui_hints_changed),
    request_resize: Some(gui_request_resize),
    request_show: Some(gui_request_show),
    request_hide: Some(gui_request_hide),
    closed: Some(gui_closed),
};

unsafe extern "C" fn gui_hints_changed(_host: *const clap_host) {}

unsafe extern "C" fn gui_request_resize(host: *const clap_host, w: u32, h: u32) -> bool {
    let Some(f) = flags_of(host) else {
        return false;
    };
    if let Ok(mut r) = f.resize.lock() {
        *r = Some((w, h));
    }
    true
}

unsafe extern "C" fn gui_request_show(_host: *const clap_host) -> bool {
    false
}

unsafe extern "C" fn gui_request_hide(_host: *const clap_host) -> bool {
    false
}

unsafe extern "C" fn gui_closed(host: *const clap_host, _was_destroyed: bool) {
    if let Some(f) = flags_of(host) {
        f.gui_closed.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Streams for saving and loading state

struct Writer(Vec<u8>);

unsafe extern "C" fn stream_write(s: *const clap_ostream, buf: *const c_void, size: u64) -> i64 {
    let w = unsafe { &mut *((*s).ctx as *mut Writer) };
    if buf.is_null() {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, size as usize) };
    w.0.extend_from_slice(bytes);
    size as i64
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

unsafe extern "C" fn stream_read(s: *const clap_istream, buf: *mut c_void, size: u64) -> i64 {
    let r = unsafe { &mut *((*s).ctx as *mut Reader) };
    if buf.is_null() {
        return -1;
    }
    let n = (size as usize).min(r.bytes.len() - r.at);
    unsafe { ptr::copy_nonoverlapping(r.bytes[r.at..].as_ptr(), buf as *mut u8, n) };
    r.at += n;
    n as i64
}

// ---------------------------------------------------------------------------
// Events in and out of a block

/// Parameter changes for the plugin, as CLAP input events.
struct InEvents {
    events: Vec<clap_event_param_value>,
}

unsafe extern "C" fn in_size(list: *const clap_input_events) -> u32 {
    let e = unsafe { &*((*list).ctx as *const InEvents) };
    e.events.len() as u32
}

unsafe extern "C" fn in_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let e = unsafe { &*((*list).ctx as *const InEvents) };
    match e.events.get(index as usize) {
        Some(ev) => &ev.header,
        None => ptr::null(),
    }
}

fn param_event(id: u32, value: f64) -> clap_event_param_value {
    clap_event_param_value {
        header: clap_event_header {
            size: size_of::<clap_event_param_value>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: id,
        cookie: ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    }
}

/// Where the plugin's own parameter changes go.
struct OutEvents {
    back: Producer<(u32, f64)>,
    edited: bool,
}

unsafe extern "C" fn out_push(
    list: *const clap_output_events,
    ev: *const clap_event_header,
) -> bool {
    let o = unsafe { &mut *((*list).ctx as *mut OutEvents) };
    let Some(h) = (unsafe { ev.as_ref() }) else {
        return false;
    };
    if h.space_id == CLAP_CORE_EVENT_SPACE_ID && h.type_ == CLAP_EVENT_PARAM_VALUE {
        let p = unsafe { &*(ev as *const clap_event_param_value) };
        let _ = o.back.push((p.param_id, p.value));
        o.edited = true;
    }
    true
}

// ---------------------------------------------------------------------------
// The plugin itself

/// Calls one of the plugin's (or an extension's) functions, if it has it.
macro_rules! call {
    ($table:expr, $f:ident, $plugin:expr $(, $arg:expr)*) => {
        match unsafe { (*$table).$f } {
            Some(f) => Some(unsafe { f($plugin $(, $arg)*) }),
            None => None,
        }
    };
}

/// The plugin, shut down when the last side lets go.
struct Core {
    plugin: *const clap_plugin,
    params: *const clap_plugin_params,
    state: *const clap_plugin_state,
    latency: *const clap_plugin_latency,
    gui: *const clap_plugin_gui,
    ports: *const clap_plugin_audio_ports,
    rate: f64,
    max_block: u32,
    active: AtomicBool,
    /// Started processing since it was last activated.
    processing: AtomicBool,
    /// Held by the audio side for each block, and by the UI side while
    /// it restarts the plugin. The audio side only tries it: when the UI
    /// side has it, the block is silent.
    gate: Mutex<()>,
    host: Box<HostBox>,
    // Dropped last: the module must outlive its plugins.
    _module: Arc<ClapModule>,
}

// CLAP says which thread may call what; the two sides keep to it.
unsafe impl Send for Core {}
unsafe impl Sync for Core {}

impl Core {
    fn ext<T>(plugin: *const clap_plugin, id: &CStr) -> *const T {
        call!(plugin, get_extension, plugin, id.as_ptr()).unwrap_or(ptr::null()) as *const T
    }

    fn activate(&self) -> Result<()> {
        let ok = call!(
            self.plugin,
            activate,
            self.plugin,
            self.rate,
            1,
            self.max_block
        );
        if ok != Some(true) {
            return Err("the plugin could not be started".into());
        }
        self.active.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Stops processing, as if on the audio thread, which is not running
    /// it now (the gate is held or the audio side is gone).
    fn stop_processing(&self) {
        if self.processing.swap(false, Ordering::Relaxed) {
            let _audio = AudioThread::enter();
            call!(self.plugin, stop_processing, self.plugin);
        }
    }

    fn deactivate(&self) {
        self.stop_processing();
        if self.active.swap(false, Ordering::Relaxed) {
            call!(self.plugin, deactivate, self.plugin);
        }
    }

    fn latency(&self) -> u32 {
        if self.latency.is_null() || !self.active.load(Ordering::Relaxed) {
            return 0;
        }
        call!(self.latency, get, self.plugin).unwrap_or(0)
    }

    fn ports(&self, input: bool) -> Vec<(Bus, bool)> {
        if self.ports.is_null() {
            return Vec::new();
        }
        let n = call!(self.ports, count, self.plugin, input).unwrap_or(0);
        (0..n)
            .filter_map(|i| {
                let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
                (call!(self.ports, get, self.plugin, i, input, &mut info) == Some(true)).then(
                    || {
                        let main = info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0;
                        let bus = Bus {
                            name: text_in(&info.name),
                            // A sane cap on what we allocate for.
                            channels: info.channel_count.min(64),
                            aux: !main,
                        };
                        (bus, main)
                    },
                )
            })
            .collect()
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.deactivate();
        call!(self.plugin, destroy, self.plugin);
    }
}

/// A parameter as the plugin describes it, with its range.
#[derive(Clone, Debug)]
struct Info {
    id: u32,
    flags: u32,
    min: f64,
    max: f64,
    name: String,
}

impl Info {
    fn norm(&self, plain: f64) -> f64 {
        if self.max > self.min {
            ((plain - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    fn plain(&self, norm: f64) -> f64 {
        let v = self.min + norm.clamp(0.0, 1.0) * (self.max - self.min);
        if self.flags & CLAP_PARAM_IS_STEPPED != 0 {
            v.round()
        } else {
            v
        }
    }
}

/// Edits queued between the UI and the audio thread.
const EDITS: usize = 1024;

/// The UI side of a CLAP plugin running in this process.
pub struct ClapPlugin {
    // Declared first so it is dropped first: the plugin's window is
    // taken down before the plugin.
    gui: Option<Gui>,
    core: Arc<Core>,
    edits: Mutex<Producer<(u32, f64)>>,
    back: Mutex<Consumer<(u32, f64)>>,
    /// Plain values set or reported since the plugin was last asked,
    /// which it may not show yet.
    values: Mutex<HashMap<u32, f64>>,
    infos: Mutex<Vec<Info>>,
    buses: Buses,
}

/// The plugin's own window, while it is open.
enum Gui {
    #[cfg(windows)]
    Embedded(crate::clap_editor_win::EditorWindow),
    Floating,
}

/// Makes the plugin `id` from `module` and starts it, ready to process
/// blocks of up to `max_block` frames at `rate`, hearing the inputs and
/// outputs `layout` asks for.
pub fn instantiate_with(
    module: Arc<ClapModule>,
    cid: &TUID,
    rate: f64,
    max_block: usize,
    layout: &Layout,
) -> Result<(ClapPlugin, ClapProcessor)> {
    let id = module.id_of(cid).ok_or("the plugin is not in this file")?;
    let c_id = CString::new(id).map_err(|_| "the plugin's id is not valid")?;
    let host = HostBox::new();
    let f = module.factory();
    let create = f
        .create_plugin
        .ok_or("the plugin factory can't make plugins")?;
    let plugin = unsafe { create(module.factory, &host.clap, c_id.as_ptr()) };
    if plugin.is_null() {
        return Err("the plugin could not be created".into());
    }
    if call!(plugin, init, plugin) != Some(true) {
        call!(plugin, destroy, plugin);
        return Err("the plugin could not be set up".into());
    }
    let core = Arc::new(Core {
        plugin,
        params: Core::ext(plugin, CLAP_EXT_PARAMS),
        state: Core::ext(plugin, CLAP_EXT_STATE),
        latency: Core::ext(plugin, CLAP_EXT_LATENCY),
        gui: Core::ext(plugin, CLAP_EXT_GUI),
        ports: Core::ext(plugin, CLAP_EXT_AUDIO_PORTS),
        rate,
        max_block: max_block as u32,
        active: AtomicBool::new(false),
        processing: AtomicBool::new(false),
        gate: Mutex::new(()),
        host,
        _module: module,
    });
    let inputs = core.ports(true);
    let outputs = core.ports(false);
    core.activate()?;
    let buses = Buses {
        inputs: inputs.iter().map(|(b, _)| b.clone()).collect(),
        outputs: outputs.iter().map(|(b, _)| b.clone()).collect(),
    };
    let (edits_tx, edits_rx) = RingBuffer::new(EDITS);
    let (back_tx, back_rx) = RingBuffer::new(EDITS);
    let mut processor = ClapProcessor::new(core.clone(), edits_rx, back_tx, &buses, max_block);
    processor.key = if layout.sidechain {
        buses.sidechain()
    } else {
        None
    };
    processor.mix = (layout.outputs.iter())
        .map(|&i| i as usize)
        .filter(|&i| buses.extra_outputs().any(|(j, _)| j == i))
        .collect();
    let plugin = ClapPlugin {
        gui: None,
        core,
        edits: Mutex::new(edits_tx),
        back: Mutex::new(back_rx),
        values: Mutex::new(HashMap::new()),
        infos: Mutex::new(Vec::new()),
        buses,
    };
    plugin.rescan_params();
    Ok((plugin, processor))
}

impl ClapPlugin {
    /// Frames of delay the plugin adds.
    pub fn latency(&self) -> u32 {
        self.core.latency()
    }

    pub fn buses(&self) -> &Buses {
        &self.buses
    }

    fn rescan_params(&self) {
        let core = &self.core;
        let mut infos = Vec::new();
        if !core.params.is_null() {
            let n = call!(core.params, count, core.plugin).unwrap_or(0);
            for i in 0..n {
                let mut info: clap_param_info = unsafe { std::mem::zeroed() };
                if call!(core.params, get_info, core.plugin, i, &mut info) == Some(true) {
                    infos.push(Info {
                        id: info.id,
                        flags: info.flags,
                        min: info.min_value,
                        max: info.max_value,
                        name: text_in(&info.name),
                    });
                }
            }
        }
        *self.infos.lock().unwrap_or_else(|e| e.into_inner()) = infos;
    }

    fn info(&self, id: u32) -> Option<Info> {
        let infos = self.infos.lock().unwrap_or_else(|e| e.into_inner());
        infos.iter().find(|i| i.id == id).cloned()
    }

    /// Takes in the changes the plugin made itself; true if there were.
    fn take_back(&self) -> bool {
        let mut back = self.back.lock().unwrap_or_else(|e| e.into_inner());
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        let mut any = false;
        while let Ok((id, v)) = back.pop() {
            values.insert(id, v);
            any = true;
        }
        any
    }

    pub fn params(&self) -> Vec<Param> {
        let infos = self.infos.lock().unwrap_or_else(|e| e.into_inner());
        (infos.iter())
            .filter(|i| i.flags & CLAP_PARAM_IS_HIDDEN == 0)
            .map(|i| Param {
                id: i.id,
                title: i.name.clone(),
                units: String::new(),
                automatable: i.flags & CLAP_PARAM_IS_AUTOMATABLE != 0,
                bypass: i.flags & CLAP_PARAM_IS_BYPASS != 0,
                program_change: false,
                steps: if i.flags & CLAP_PARAM_IS_STEPPED != 0 {
                    (i.max - i.min).round().max(0.0) as i32
                } else {
                    0
                },
            })
            .collect()
    }

    fn plain_value(&self, id: u32) -> Option<f64> {
        self.take_back();
        if let Some(v) = self.values.lock().ok()?.get(&id) {
            return Some(*v);
        }
        let core = &self.core;
        if core.params.is_null() {
            return None;
        }
        let mut v = 0.0;
        (call!(core.params, get_value, core.plugin, id, &mut v) == Some(true)).then_some(v)
    }

    /// A parameter's value, 0 to 1 across its range.
    pub fn param(&self, id: u32) -> Option<f64> {
        let info = self.info(id)?;
        Some(info.norm(self.plain_value(id)?))
    }

    /// How the plugin would display `value` (0 to 1) of `id`.
    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        let info = self.info(id)?;
        let core = &self.core;
        if core.params.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 256];
        let ok = call!(
            core.params,
            value_to_text,
            core.plugin,
            id,
            info.plain(value),
            buf.as_mut_ptr(),
            buf.len() as u32
        );
        (ok == Some(true)).then(|| text_in(&buf))
    }

    /// Sets a parameter (0 to 1) from the host's own controls; the
    /// plugin hears it with its next block.
    pub fn set_param(&self, id: u32, value: f64) {
        let Some(info) = self.info(id) else {
            return;
        };
        let plain = info.plain(value);
        if let Ok(mut v) = self.values.lock() {
            v.insert(id, plain);
        }
        if let Ok(mut q) = self.edits.lock() {
            let _ = q.push((id, plain));
        }
    }

    pub fn save_state(&self) -> Result<PluginState> {
        let core = &self.core;
        if core.state.is_null() {
            return Ok(PluginState::default());
        }
        let mut w = Writer(Vec::new());
        let stream = clap_ostream {
            ctx: &mut w as *mut Writer as *mut c_void,
            write: Some(stream_write),
        };
        if call!(core.state, save, core.plugin, &stream) != Some(true) {
            return Err("saving the plugin's state failed".into());
        }
        Ok(PluginState {
            component: w.0,
            controller: None,
        })
    }

    pub fn load_state(&self, state: &PluginState) -> Result<()> {
        let core = &self.core;
        if core.state.is_null() {
            return Ok(());
        }
        let mut r = Reader {
            bytes: &state.component,
            at: 0,
        };
        let stream = clap_istream {
            ctx: &mut r as *mut Reader as *mut c_void,
            read: Some(stream_read),
        };
        if call!(core.state, load, core.plugin, &stream) != Some(true) {
            return Err("restoring the plugin's state failed".into());
        }
        self.take_back();
        if let Ok(mut v) = self.values.lock() {
            v.clear();
        }
        Ok(())
    }

    /// Deactivates and activates the plugin again, while the audio side
    /// plays silence.
    pub fn reactivate(&self) -> Result<()> {
        let _quiet = self.core.gate.lock().unwrap_or_else(|e| e.into_inner());
        self.core.deactivate();
        self.core.activate()
    }

    /// Hands the plugin queued parameter changes while it isn't
    /// processing, as it asked.
    fn flush(&self) {
        let core = &self.core;
        if core.params.is_null() {
            return;
        }
        let _quiet = core.gate.lock().unwrap_or_else(|e| e.into_inner());
        if core.processing.load(Ordering::Relaxed) {
            // The next block carries them.
            return;
        }
        // The app's own changes wait for the next block, queued; this
        // only lets the plugin hand back its own.
        let mut ins = InEvents { events: Vec::new() };
        let in_list = clap_input_events {
            ctx: &mut ins as *mut InEvents as *mut c_void,
            size: Some(in_size),
            get: Some(in_get),
        };
        let (tx, mut rx) = RingBuffer::new(EDITS);
        let mut outs = OutEvents {
            back: tx,
            edited: false,
        };
        let out_list = clap_output_events {
            ctx: &mut outs as *mut OutEvents as *mut c_void,
            try_push: Some(out_push),
        };
        call!(core.params, flush, core.plugin, &in_list, &out_list);
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        while let Ok((id, v)) = rx.pop() {
            values.insert(id, v);
        }
        if outs.edited {
            core.host.flags.edited.store(true, Ordering::Relaxed);
        }
    }

    /// What the plugin has asked for since last asked, and its latency.
    /// A plugin that asked to be restarted is restarted here first.
    pub fn poll(&mut self) -> Status {
        let core = self.core.clone();
        let flags = &core.host.flags;
        if flags.gui_closed.swap(false, Ordering::Relaxed) {
            self.close_editor();
        }
        #[cfg(windows)]
        if let Some(Gui::Embedded(w)) = &self.gui
            && !w.is_open()
        {
            self.close_editor();
        }
        if flags.callback.swap(false, Ordering::Relaxed) {
            call!(self.core.plugin, on_main_thread, self.core.plugin);
        }
        let rescan = flags.rescan.swap(0, Ordering::Relaxed);
        if rescan & (CLAP_PARAM_RESCAN_INFO | CLAP_PARAM_RESCAN_ALL) != 0 {
            self.rescan_params();
        }
        if rescan & (CLAP_PARAM_RESCAN_VALUES | CLAP_PARAM_RESCAN_ALL) != 0 {
            self.take_back();
            if let Ok(mut v) = self.values.lock() {
                v.clear();
            }
        }
        if flags.flush.swap(false, Ordering::Relaxed) {
            self.flush();
        }
        let restart = flags.restart.swap(false, Ordering::Relaxed);
        if restart {
            let _ = self.reactivate();
        }
        let resize = flags.resize.lock().ok().and_then(|mut r| r.take());
        #[cfg(windows)]
        if let (Some((w, h)), Some(Gui::Embedded(win))) = (resize, &self.gui) {
            win.resize(w, h);
        }
        #[cfg(not(windows))]
        let _ = resize;
        let edited = self.take_back() | flags.edited.swap(false, Ordering::Relaxed);
        Status {
            latency: self.latency(),
            restart,
            reload: flags.ports.swap(false, Ordering::Relaxed),
            edited,
            editor_open: self.editor_open(),
        }
    }

    fn gui_supports(&self, api: &CStr, floating: bool) -> bool {
        let core = &self.core;
        !core.gui.is_null()
            && call!(
                core.gui,
                is_api_supported,
                core.plugin,
                api.as_ptr(),
                floating
            ) == Some(true)
    }

    /// Opens the plugin's own window, titled `title`, or brings it to
    /// the front.
    pub fn open_editor(&mut self, title: &str) -> Result<()> {
        let core = self.core.clone();
        if let Some(g) = &self.gui {
            match g {
                #[cfg(windows)]
                Gui::Embedded(w) => w.focus(),
                Gui::Floating => {
                    call!(core.gui, show, core.plugin);
                }
            }
            return Ok(());
        }
        if core.gui.is_null() {
            return Err("this plugin has no editor of its own".into());
        }
        #[cfg(windows)]
        if self.gui_supports(CLAP_WINDOW_API_WIN32, false) {
            if call!(
                core.gui,
                create,
                core.plugin,
                CLAP_WINDOW_API_WIN32.as_ptr(),
                false
            ) != Some(true)
            {
                return Err("the plugin could not open its editor".into());
            }
            // The plugin and its window extension live as long as `core`.
            let opened =
                unsafe { crate::clap_editor_win::EditorWindow::open(core.plugin, core.gui, title) };
            match opened {
                Ok(w) => {
                    self.gui = Some(Gui::Embedded(w));
                    return Ok(());
                }
                Err(e) => {
                    call!(core.gui, destroy, core.plugin);
                    return Err(e);
                }
            }
        }
        let api = if cfg!(windows) {
            CLAP_WINDOW_API_WIN32
        } else if cfg!(target_os = "macos") {
            CLAP_WINDOW_API_COCOA
        } else {
            CLAP_WINDOW_API_X11
        };
        if !self.gui_supports(api, true) {
            return Err("plugin windows are not supported on this platform yet".into());
        }
        if call!(core.gui, create, core.plugin, api.as_ptr(), true) != Some(true) {
            return Err("the plugin could not open its editor".into());
        }
        if let Ok(t) = CString::new(title) {
            call!(core.gui, suggest_title, core.plugin, t.as_ptr());
        }
        core.host.flags.gui_closed.store(false, Ordering::Relaxed);
        if call!(core.gui, show, core.plugin) != Some(true) {
            call!(core.gui, destroy, core.plugin);
            return Err("the plugin could not show its editor".into());
        }
        self.gui = Some(Gui::Floating);
        Ok(())
    }

    pub fn editor_open(&self) -> bool {
        match &self.gui {
            None => false,
            #[cfg(windows)]
            Some(Gui::Embedded(w)) => w.is_open(),
            Some(Gui::Floating) => true,
        }
    }

    pub fn close_editor(&mut self) {
        // Our window, if there is one, goes at the end of this scope,
        // once the plugin has let go of it.
        let Some(_window) = self.gui.take() else {
            return;
        };
        let core = &self.core;
        call!(core.gui, hide, core.plugin);
        call!(core.gui, destroy, core.plugin);
    }
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        self.close_editor();
    }
}

/// The audio side of a CLAP plugin running in this process: an
/// [`Insert`] for the engine.
pub struct ClapProcessor {
    core: Arc<Core>,
    edits: Consumer<(u32, f64)>,
    ins: Box<InEvents>,
    in_list: clap_input_events,
    outs: Box<OutEvents>,
    out_list: clap_output_events,
    inputs: Vec<BusBufs>,
    outputs: Vec<BusBufs>,
    in_bufs: Vec<clap_audio_buffer>,
    out_bufs: Vec<clap_audio_buffer>,
    max_block: usize,
    steady: i64,
    /// The input a sidechain goes to, when switched on.
    key: Option<usize>,
    /// Extra outputs mixed into the track.
    mix: Vec<usize>,
}

// The raw pointers point into buffers this struct owns and never resizes.
unsafe impl Send for ClapProcessor {}

impl ClapProcessor {
    fn new(
        core: Arc<Core>,
        edits: Consumer<(u32, f64)>,
        back: Producer<(u32, f64)>,
        buses: &Buses,
        max_block: usize,
    ) -> ClapProcessor {
        let mut ins = Box::new(InEvents {
            events: Vec::with_capacity(EDITS),
        });
        let in_list = clap_input_events {
            ctx: &mut *ins as *mut InEvents as *mut c_void,
            size: Some(in_size),
            get: Some(in_get),
        };
        let mut outs = Box::new(OutEvents {
            back,
            edited: false,
        });
        let out_list = clap_output_events {
            ctx: &mut *outs as *mut OutEvents as *mut c_void,
            try_push: Some(out_push),
        };
        let bufs = |list: &[Bus]| -> Vec<BusBufs> {
            (list.iter())
                .map(|b| BusBufs::new(b.channels as usize, max_block))
                .collect()
        };
        let mut inputs = bufs(&buses.inputs);
        let mut outputs = bufs(&buses.outputs);
        let clap = |b: &mut BusBufs| clap_audio_buffer {
            data32: b.ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: b.ptrs.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let in_bufs = inputs.iter_mut().map(clap).collect();
        let out_bufs = outputs.iter_mut().map(clap).collect();
        ClapProcessor {
            core,
            edits,
            ins,
            in_list,
            outs,
            out_list,
            inputs,
            outputs,
            in_bufs,
            out_bufs,
            max_block,
            steady: 0,
            key: None,
            mix: Vec::new(),
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let n = left.len().min(right.len()).min(self.max_block);
        let core = &self.core;
        if !core.active.load(Ordering::Relaxed) {
            return;
        }
        if !core.processing.load(Ordering::Relaxed) {
            if call!(core.plugin, start_processing, core.plugin) != Some(true) {
                // Leave the audio as it was: a failing effect is skipped.
                return;
            }
            core.processing.store(true, Ordering::Relaxed);
        }
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
        for b in &mut self.in_bufs {
            b.constant_mask = 0;
        }

        let ins = &mut self.ins.events;
        ins.clear();
        while ins.len() < ins.capacity() {
            match self.edits.pop() {
                Ok((id, v)) => ins.push(param_event(id, v)),
                Err(_) => break,
            }
        }
        self.outs.edited = false;

        let process = clap_process {
            steady_time: self.steady,
            frames_count: n as u32,
            transport: ptr::null(),
            audio_inputs: self.in_bufs.as_ptr(),
            audio_outputs: self.out_bufs.as_mut_ptr(),
            audio_inputs_count: self.in_bufs.len() as u32,
            audio_outputs_count: self.out_bufs.len() as u32,
            in_events: &self.in_list,
            out_events: &self.out_list,
        };
        let status = call!(core.plugin, process, core.plugin, &process);
        self.steady += n as i64;
        if self.outs.edited {
            core.host.flags.edited.store(true, Ordering::Relaxed);
        }
        if matches!(status, None | Some(CLAP_PROCESS_ERROR)) {
            return;
        }
        // A plugin with no audio out (it only listens) leaves the track
        // as it was.
        let Some(main) = self.outputs.first() else {
            return;
        };
        main.out(left, right, n, false);
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
        let _audio = AudioThread::enter();
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

impl Insert for ClapProcessor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.blocks(left, right, None);
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        self.blocks(left, right, Some(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_become_the_same_class_id_every_time() {
        let a = cid_of("org.example.gain");
        assert_eq!(a, cid_of("org.example.gain"));
        assert_ne!(a, cid_of("org.example.gain2"));
        assert_ne!(a, [0; 16]);
    }

    #[test]
    fn features_become_categories() {
        let d = Descriptor {
            id: "x".into(),
            name: "X".into(),
            vendor: "V".into(),
            version: "1".into(),
            features: vec!["audio-effect".into(), "reverb".into(), "stereo".into()],
        };
        let c = d.class();
        assert!(c.is_audio_plugin());
        assert!(!c.is_instrument());
        assert_eq!(c.sub_categories, "Fx|reverb|stereo");
        let synth = Descriptor {
            features: vec!["instrument".into(), "synthesizer".into()],
            ..d
        };
        assert!(synth.class().is_instrument());
    }

    #[test]
    fn values_map_across_the_range() {
        let i = Info {
            id: 0,
            flags: 0,
            min: -24.0,
            max: 24.0,
            name: String::new(),
        };
        assert_eq!(i.plain(0.5), 0.0);
        assert_eq!(i.norm(12.0), 0.75);
        let stepped = Info {
            flags: CLAP_PARAM_IS_STEPPED,
            min: 0.0,
            max: 4.0,
            ..i
        };
        assert_eq!(stepped.plain(0.6), 2.0);
    }
}
