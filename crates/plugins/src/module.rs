// SPDX-License-Identifier: GPL-3.0-or-later
//! Loading a VST 3 module (a `.vst3` bundle) and reading what it offers.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;

use vst3::Steinberg::*;
use vst3::{ComPtr, Interface};

use crate::com::read_cstr;

pub type Result<T> = std::result::Result<T, String>;

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

/// One class a module offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassInfo {
    pub cid: TUID,
    pub name: String,
    /// "Audio Module Class" for plugins; other classes are helpers.
    pub category: String,
    /// VST 3 sub-categories, e.g. "Fx|Distortion" or "Instrument|Synth".
    pub sub_categories: String,
    pub vendor: String,
}

impl ClassInfo {
    pub fn is_audio_plugin(&self) -> bool {
        self.category == "Audio Module Class"
    }

    pub fn is_instrument(&self) -> bool {
        self.sub_categories.split('|').any(|c| c == "Instrument")
    }
}

/// A loaded module. Dropping it calls the module's exit function.
pub struct Module {
    pub path: PathBuf,
    // Field order matters: the factory is released before the library is
    // unloaded.
    pub(crate) factory: ComPtr<IPluginFactory>,
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
            Ok(Module {
                path: bundle.to_owned(),
                factory,
                lib,
            })
        }
    }

    pub fn classes(&self) -> Vec<ClassInfo> {
        let mut vendor = String::new();
        let mut finfo: PFactoryInfo = unsafe { std::mem::zeroed() };
        if unsafe { self.factory.getFactoryInfo(&mut finfo) } == kResultOk {
            vendor = read_cstr(&finfo.vendor);
        }
        let f2 = self.factory.cast::<IPluginFactory2>();
        let n = unsafe { self.factory.countClasses() };
        (0..n)
            .filter_map(|i| {
                if let Some(f2) = &f2 {
                    let mut info: PClassInfo2 = unsafe { std::mem::zeroed() };
                    if unsafe { f2.getClassInfo2(i, &mut info) } == kResultOk {
                        let v = read_cstr(&info.vendor);
                        return Some(ClassInfo {
                            cid: info.cid,
                            name: read_cstr(&info.name),
                            category: read_cstr(&info.category),
                            sub_categories: read_cstr(&info.subCategories),
                            vendor: if v.is_empty() { vendor.clone() } else { v },
                        });
                    }
                }
                let mut info: PClassInfo = unsafe { std::mem::zeroed() };
                (unsafe { self.factory.getClassInfo(i, &mut info) } == kResultOk).then(|| {
                    ClassInfo {
                        cid: info.cid,
                        name: read_cstr(&info.name),
                        category: read_cstr(&info.category),
                        sub_categories: String::new(),
                        vendor: vendor.clone(),
                    }
                })
            })
            .collect()
    }

    pub(crate) fn create<I: Interface>(&self, cid: &TUID) -> Option<ComPtr<I>> {
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

/// A class ID as 32 hex digits, for saving.
pub fn cid_to_hex(cid: &TUID) -> String {
    cid.iter().map(|b| format!("{:02X}", *b as u8)).collect()
}

pub fn cid_from_hex(s: &str) -> Option<TUID> {
    if s.len() != 32 {
        return None;
    }
    let mut cid: TUID = [0; 16];
    for (i, c) in cid.iter_mut().enumerate() {
        *c = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()? as _;
    }
    Some(cid)
}
