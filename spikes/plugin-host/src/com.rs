// SPDX-License-Identifier: GPL-3.0-or-later
//! Host-side COM objects that a VST 3 plugin calls back into.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};

pub const HOST_NAME: &str = "DAW spike host";

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
        // Host-provided messages and attribute lists are not needed by the
        // spike's test plugin; real plugins that ask get a polite refusal.
        unsafe { *obj = std::ptr::null_mut() };
        kResultFalse
    }
}

pub fn host_app() -> ComPtr<FUnknown> {
    ComWrapper::new(HostApp).to_com_ptr::<FUnknown>().unwrap()
}

// ---------------------------------------------------------------------------

/// Counts edits the plugin's editor reports.
#[derive(Default)]
pub struct ComponentHandler {
    pub edits: AtomicU32,
    pub restarts: AtomicU32,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, _id: ParamID, _v: ParamValue) -> tresult {
        self.edits.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        self.restarts.fetch_add(1, Ordering::Relaxed);
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

/// One parameter's automation points for a block.
pub struct ParamQueue {
    id: ParamID,
    points: Mutex<Vec<(int32, ParamValue)>>,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id
    }
    unsafe fn getPointCount(&self) -> int32 {
        self.points.lock().unwrap().len() as int32
    }
    unsafe fn getPoint(&self, i: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        match self.points.lock().unwrap().get(i as usize) {
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
        let mut p = self.points.lock().unwrap();
        p.push((offset, value));
        if !index.is_null() {
            unsafe { *index = p.len() as int32 - 1 };
        }
        kResultOk
    }
}

/// Parameter changes for one process() call.
#[derive(Default)]
pub struct ParamChanges {
    queues: Mutex<Vec<ComPtr<IParamValueQueue>>>,
    ids: Mutex<Vec<ParamID>>,
}

impl ParamChanges {
    pub fn with_point(id: ParamID, offset: i32, value: f64) -> Self {
        let c = Self::default();
        let mut idx = 0;
        unsafe {
            let q = c.add(id, &mut idx);
            q.addPoint(offset, value, std::ptr::null_mut());
        }
        c
    }

    fn add(&self, id: ParamID, index: &mut int32) -> ComPtr<IParamValueQueue> {
        let mut ids = self.ids.lock().unwrap();
        let mut qs = self.queues.lock().unwrap();
        if let Some(i) = ids.iter().position(|&x| x == id) {
            *index = i as int32;
            return qs[i].clone();
        }
        let q = ComWrapper::new(ParamQueue {
            id,
            points: Mutex::new(Vec::new()),
        })
        .to_com_ptr::<IParamValueQueue>()
        .unwrap();
        ids.push(id);
        qs.push(q.clone());
        *index = qs.len() as int32 - 1;
        q
    }
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.queues.lock().unwrap().len() as int32
    }
    unsafe fn getParameterData(&self, i: int32) -> *mut IParamValueQueue {
        self.queues
            .lock()
            .unwrap()
            .get(i as usize)
            .map_or(std::ptr::null_mut(), |q| q.as_ptr())
    }
    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        let mut i = 0;
        let q = self.add(unsafe { *id }, &mut i);
        if !index.is_null() {
            unsafe { *index = i };
        }
        q.as_ptr()
    }
}
