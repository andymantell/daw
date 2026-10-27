// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedding a plugin's editor in a window we create (Windows).

#![allow(non_snake_case)]

use std::ffi::CStr;
use std::fmt::Write as _;
use std::ptr;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// Outer window size for a given client size.
fn outer(w: i32, h: i32) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    unsafe { AdjustWindowRectEx(&mut r, WS_OVERLAPPEDWINDOW, 0, 0) };
    (r.right - r.left, r.bottom - r.top)
}

/// Receives resize requests from the plugin.
struct Frame {
    hwnd: AtomicIsize,
    requests: AtomicU32,
}

impl Class for Frame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, view: *mut IPlugView, rect: *mut ViewRect) -> tresult {
        self.requests.fetch_add(1, Ordering::Relaxed);
        let r = unsafe { *rect };
        let (w, h) = outer(r.right - r.left, r.bottom - r.top);
        let hwnd = self.hwnd.load(Ordering::Relaxed) as HWND;
        unsafe {
            SetWindowPos(hwnd, ptr::null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER);
            if let Some(v) = vst3::ComRef::from_raw(view) {
                v.onSize(rect);
            }
        }
        kResultOk
    }
}

fn pump(for_: Duration) {
    let until = Instant::now() + for_;
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while Instant::now() < until {
        unsafe {
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Opens the editor in our window, keeps it up for `seconds`, tries a
/// resize if the plugin allows one, then closes it.
pub fn exercise(view: &ComPtr<IPlugView>, seconds: f64) -> Result<String, String> {
    let hwnd_type: &CStr = c"HWND";
    if unsafe { view.isPlatformTypeSupported(hwnd_type.as_ptr()) } != kResultTrue {
        return Err("HWND not supported".into());
    }
    let mut size = ViewRect {
        left: 0,
        top: 0,
        right: 640,
        bottom: 480,
    };
    unsafe { view.getSize(&mut size) };
    let (w, h) = (size.right - size.left, size.bottom - size.top);

    let class = wide("DawSpikePluginEditor");
    let title = wide("Plugin editor (DAW spike)");
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(ptr::null());
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc); // Fails harmlessly if already registered.
        let (ow, oh) = outer(w.max(1), h.max(1));
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ow,
            oh,
            ptr::null_mut(),
            ptr::null_mut(),
            hinst,
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err("could not create window".into());
    }

    let frame = ComWrapper::new(Frame {
        hwnd: AtomicIsize::new(hwnd as isize),
        requests: AtomicU32::new(0),
    });
    let frame_ptr = frame.to_com_ptr::<IPlugFrame>().unwrap();
    let mut report = String::new();
    unsafe {
        view.setFrame(frame_ptr.as_ptr());
        let r = view.attached(hwnd as *mut _, hwnd_type.as_ptr());
        if r != kResultOk {
            DestroyWindow(hwnd);
            return Err(format!("attached() returned {r}"));
        }
        let _ = write!(report, "opened at {w}x{h}");
        ShowWindow(hwnd, SW_SHOW);
        pump(Duration::from_secs_f64(seconds / 2.0));

        if view.canResize() == kResultTrue {
            let mut want = ViewRect {
                left: 0,
                top: 0,
                right: w + 120,
                bottom: h + 80,
            };
            view.checkSizeConstraint(&mut want);
            let (ow, oh) = outer(want.right - want.left, want.bottom - want.top);
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                0,
                0,
                ow,
                oh,
                SWP_NOMOVE | SWP_NOZORDER,
            );
            let r = view.onSize(&mut want);
            let _ = write!(
                report,
                ", resized to {}x{} ({})",
                want.right - want.left,
                want.bottom - want.top,
                if r == kResultOk {
                    "accepted"
                } else {
                    "refused"
                }
            );
        } else {
            let _ = write!(report, ", fixed size");
        }
        pump(Duration::from_secs_f64(seconds / 2.0));

        let requests = frame.requests.load(Ordering::Relaxed);
        if requests > 0 {
            let _ = write!(report, ", plugin asked to resize {requests} time(s)");
        }
        view.removed();
        view.setFrame(ptr::null_mut());
        DestroyWindow(hwnd);
        pump(Duration::from_millis(100));
    }
    report.push_str(", closed cleanly");
    Ok(report)
}
