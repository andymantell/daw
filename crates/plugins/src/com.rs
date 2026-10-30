// SPDX-License-Identifier: GPL-3.0-or-later
//! Host-side COM objects that a VST 3 plugin calls back into.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use rtrb::Producer;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};

pub const HOST_NAME: &str = "DAW";

pub fn write_wstr(dst: &mut [TChar], s: &str) {
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

pub fn read_wstr(src: &[TChar]) -> String {
    let end = src.iter().position(|&c| c == 0).unwrap_or(src.len());
    String::from_utf16_lossy(&src[..end].iter().map(|&c| c as u16).collect::<Vec<_>>())
}

pub fn read_cstr(src: &[std::ffi::c_char]) -> String {
    let end = src.iter().position(|&c| c == 0).unwrap_or(src.len());
    String::from_utf8_lossy(&src[..end].iter().map(|&c| c as u8).collect::<Vec<_>>()).into_owned()
}

// ---------------------------------------------------------------------------

pub struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        write_wstr(unsafe { &mut *name }, HOST_NAME);
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        // Host messages and attribute lists are not offered yet; plugins
        // that ask are told no, which the interface allows.
        unsafe { *obj = std::ptr::null_mut() };
        kResultFalse
    }
}

pub fn host_app() -> ComPtr<FUnknown> {
    ComWrapper::new(HostApp).to_com_ptr::<FUnknown>().unwrap()
}

// ---------------------------------------------------------------------------

/// Receives edits from the plugin's editor and passes them to the audio
/// side, which hands them to the processor with its next block.
pub struct ComponentHandler {
    pub(crate) edits: Mutex<Producer<(ParamID, ParamValue)>>,
    /// The restart flags the plugin has raised (its latency changed,
    /// say), until the UI side takes them.
    pub restart: AtomicI32,
    /// Something the user did in the editor changed a parameter.
    pub edited: AtomicBool,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, v: ParamValue) -> tresult {
        self.edited.store(true, Ordering::Relaxed);
        // Called on the UI thread; a full queue drops the edit, and the
        // next one carries the value on.
        if let Ok(mut q) = self.edits.lock() {
            let _ = q.push((id, v));
        }
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.restart.fetch_or(flags, Ordering::Relaxed);
        kResultOk
    }
}

// ---------------------------------------------------------------------------

/// An in-memory IBStream, used to save and restore plugin state.
#[derive(Default)]
pub struct MemStream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl MemStream {
    pub fn with_bytes(bytes: Vec<u8>) -> Self {
        Self {
            inner: Mutex::new((bytes, 0)),
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.inner.lock().unwrap().0.clone()
    }
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, n: int32, read: *mut int32) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = (n.max(0) as usize).min(data.len().saturating_sub(*pos));
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buffer.cast::<u8>(), n) };
        *pos += n;
        if !read.is_null() {
            unsafe { *read = n as int32 };
        }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, n: int32, written: *mut int32) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = n.max(0) as usize;
        let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
        if data.len() < *pos + n {
            data.resize(*pos + n, 0);
        }
        data[*pos..*pos + n].copy_from_slice(src);
        *pos += n;
        if !written.is_null() {
            unsafe { *written = n as int32 };
        }
        kResultOk
    }
    unsafe fn seek(&self, to: int64, mode: int32, result: *mut int64) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let base = match mode {
            0 => 0,                 // kIBSeekSet
            1 => *pos as i64,       // kIBSeekCur
            2 => data.len() as i64, // kIBSeekEnd
            _ => return kInvalidArgument,
        };
        let new = base + to;
        if new < 0 {
            return kResultFalse;
        }
        *pos = new as usize;
        if !result.is_null() {
            unsafe { *result = new };
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        unsafe { *pos = self.inner.lock().unwrap().1 as int64 };
        kResultOk
    }
}

// ---------------------------------------------------------------------------
// Parameter changes for one block. Everything is allocated up front and
// reused, because the audio thread fills and clears these every block.

/// Most points one parameter gets in a block.
const POINTS: usize = 16;
/// Most parameters changed in one block; later ones wait a block.
pub const QUEUES: usize = 64;

/// One parameter's points for a block.
pub struct ParamQueue {
    inner: Mutex<(ParamID, Vec<(int32, ParamValue)>)>,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.inner.lock().unwrap().0
    }
    unsafe fn getPointCount(&self) -> int32 {
        self.inner.lock().unwrap().1.len() as int32
    }
    unsafe fn getPoint(&self, i: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        match self.inner.lock().unwrap().1.get(i as usize) {
            Some(&(o, v)) => {
                unsafe {
                    *offset = o;
                    *value = v;
                }
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let p = &mut g.1;
        if p.len() == p.capacity() {
            return kResultFalse;
        }
        p.push((offset, value));
        if !index.is_null() {
            unsafe { *index = p.len() as int32 - 1 };
        }
        kResultOk
    }
}

/// Parameter changes for one process() call: a fixed pool of queues, of
/// which the first `used` are live.
pub struct ParamChanges {
    pool: Vec<ComPtr<IParamValueQueue>>,
    wrappers: Vec<ComWrapper<ParamQueue>>,
    used: Mutex<usize>,
}

impl ParamChanges {
    pub fn new() -> ParamChanges {
        let wrappers: Vec<_> = (0..QUEUES)
            .map(|_| {
                ComWrapper::new(ParamQueue {
                    inner: Mutex::new((0, Vec::with_capacity(POINTS))),
                })
            })
            .collect();
        // Some platforms' mutexes allocate on first lock (macOS); take
        // each once now so the audio thread never is the first.
        for w in &wrappers {
            drop(w.inner.lock());
        }
        let used = Mutex::new(0);
        drop(used.lock());
        ParamChanges {
            pool: wrappers
                .iter()
                .map(|w| w.to_com_ptr::<IParamValueQueue>().unwrap())
                .collect(),
            wrappers,
            used,
        }
    }

    /// Empties every queue. Does not allocate.
    pub fn clear(&self) {
        let mut used = self.used.lock().unwrap();
        for w in &self.wrappers[..*used] {
            w.inner.lock().unwrap().1.clear();
        }
        *used = 0;
    }

    /// The queue for `id`, starting one if there is room.
    fn queue(&self, id: ParamID) -> Option<(usize, &ComWrapper<ParamQueue>)> {
        let mut used = self.used.lock().unwrap();
        if let Some(i) = self.wrappers[..*used]
            .iter()
            .position(|w| w.inner.lock().unwrap().0 == id)
        {
            return Some((i, &self.wrappers[i]));
        }
        if *used == QUEUES {
            return None;
        }
        let i = *used;
        *used += 1;
        self.wrappers[i].inner.lock().unwrap().0 = id;
        Some((i, &self.wrappers[i]))
    }

    /// Adds a change at `offset` into the block. False if full.
    pub fn add(&self, id: ParamID, offset: i32, value: f64) -> bool {
        match self.queue(id) {
            Some((_, q)) => unsafe { q.addPoint(offset, value, std::ptr::null_mut()) == kResultOk },
            None => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self.used.lock().unwrap() == 0
    }
}

impl Default for ParamChanges {
    fn default() -> ParamChanges {
        ParamChanges::new()
    }
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        *self.used.lock().unwrap() as int32
    }
    unsafe fn getParameterData(&self, i: int32) -> *mut IParamValueQueue {
        let used = *self.used.lock().unwrap();
        if (0..used).contains(&(i as usize)) {
            self.pool[i as usize].as_ptr()
        } else {
            std::ptr::null_mut()
        }
    }
    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        match self.queue(unsafe { *id }) {
            Some((i, _)) => {
                if !index.is_null() {
                    unsafe { *index = i as int32 };
                }
                self.pool[i].as_ptr()
            }
            None => std::ptr::null_mut(),
        }
    }
}

// ---------------------------------------------------------------------------

/// Most note events one process() call carries: a full block of the
/// engine's, and a note-off for every key on every channel.
pub const EVENTS: usize = daw_engine::midi::MAX_EVENTS + 16 * 128;

/// The notes for one process() call, in time order. Room for
/// [`EVENTS`] is made up front, so filling it never allocates.
pub struct EventList {
    events: Mutex<Vec<Event>>,
}

impl EventList {
    pub fn new() -> EventList {
        let events = Mutex::new(Vec::with_capacity(EVENTS));
        // As for [`ParamChanges`]: the first lock is not on the audio
        // thread.
        drop(events.lock());
        EventList { events }
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }

    /// Adds an event; false if full.
    pub fn push(&self, e: Event) -> bool {
        let mut events = self.events.lock().unwrap();
        if events.len() == events.capacity() {
            return false;
        }
        events.push(e);
        true
    }

    pub fn is_empty(&self) -> bool {
        self.events.lock().unwrap().is_empty()
    }
}

impl Default for EventList {
    fn default() -> EventList {
        EventList::new()
    }
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.events.lock().unwrap().len() as int32
    }
    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        let events = self.events.lock().unwrap();
        match (usize::try_from(index).ok()).and_then(|i| events.get(i)) {
            Some(ev) if !e.is_null() => {
                unsafe { *e = *ev };
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        match unsafe { e.as_ref() } {
            Some(ev) if self.push(*ev) => kResultOk,
            _ => kResultFalse,
        }
    }
}
