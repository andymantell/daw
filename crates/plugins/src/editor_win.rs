// SPDX-License-Identifier: GPL-3.0-or-later
//! A plugin's own editor in a window of its own (Windows).
//!
//! The window lives on the UI thread; the app's event loop dispatches its
//! messages along with its own. Closing it only hides it and sets a flag;
//! the app sees [`EditorWindow::is_open`] go false and drops the editor,
//! which detaches the plugin's view before destroying the window, as the
//! VST 3 lifecycle asks.

#![allow(non_snake_case)]

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

/// Outer window size for a given client size.
fn outer(style: u32, w: i32, h: i32) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    unsafe { AdjustWindowRectEx(&mut r, style, 0, 0) };
    (r.right - r.left, r.bottom - r.top)
}

/// What the window procedure needs, kept at GWLP_USERDATA.
struct WinState {
    closed: AtomicBool,
    view: *mut IPlugView,
    resizable: bool,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WinState;
    if let Some(state) = unsafe { state.as_ref() } {
        match msg {
            WM_CLOSE => {
                unsafe { ShowWindow(hwnd, SW_HIDE) };
                state.closed.store(true, Ordering::Relaxed);
                return 0;
            }
            WM_SIZE if state.resizable && !state.view.is_null() => {
                let mut r = ViewRect {
                    left: 0,
                    top: 0,
                    right: (l & 0xFFFF) as i32,
                    bottom: ((l >> 16) & 0xFFFF) as i32,
                };
                if let Some(v) = unsafe { vst3::ComRef::from_raw(state.view) } {
                    unsafe { v.onSize(&mut r) };
                }
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// Receives resize requests from the plugin.
struct Frame {
    hwnd: AtomicIsize,
    style: u32,
}

impl Class for Frame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, view: *mut IPlugView, rect: *mut ViewRect) -> tresult {
        let r = unsafe { *rect };
        let (w, h) = outer(self.style, r.right - r.left, r.bottom - r.top);
        let hwnd = self.hwnd.load(Ordering::Relaxed) as HWND;
        unsafe {
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                0,
                0,
                w,
                h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if let Some(v) = vst3::ComRef::from_raw(view) {
                v.onSize(rect);
            }
        }
        kResultOk
    }
}

/// An open plugin editor. Dropping it closes it.
pub struct EditorWindow {
    hwnd: HWND,
    view: ComPtr<IPlugView>,
    _frame: ComWrapper<Frame>,
    state: Box<WinState>,
}

impl EditorWindow {
    /// Opens `view` in a new window titled `title`.
    pub fn open(view: ComPtr<IPlugView>, title: &str) -> Result<EditorWindow, String> {
        let hwnd_type: &CStr = c"HWND";
        if unsafe { view.isPlatformTypeSupported(hwnd_type.as_ptr()) } != kResultTrue {
            return Err("this plugin's editor does not support Windows".into());
        }
        let mut size = ViewRect {
            left: 0,
            top: 0,
            right: 640,
            bottom: 480,
        };
        unsafe { view.getSize(&mut size) };
        let (w, h) = (size.right - size.left, size.bottom - size.top);
        let resizable = unsafe { view.canResize() } == kResultTrue;
        let style = if resizable {
            STYLE | WS_THICKFRAME | WS_MAXIMIZEBOX
        } else {
            STYLE
        };

        let class = wide("DawPluginEditor");
        let title = wide(title);
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
            let (ow, oh) = outer(style, w.max(1), h.max(1));
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                style,
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
            return Err("could not create the editor window".into());
        }
        let state = Box::new(WinState {
            closed: AtomicBool::new(false),
            view: view.as_ptr(),
            resizable,
        });
        let frame = ComWrapper::new(Frame {
            hwnd: AtomicIsize::new(hwnd as isize),
            style,
        });
        let frame_ptr = frame.to_com_ptr::<IPlugFrame>().unwrap();
        unsafe {
            view.setFrame(frame_ptr.as_ptr());
            let r = view.attached(hwnd as *mut _, hwnd_type.as_ptr());
            if r != kResultOk {
                view.setFrame(ptr::null_mut());
                DestroyWindow(hwnd);
                return Err(format!("the plugin could not open its editor ({r})"));
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*state as *const WinState as isize);
            ShowWindow(hwnd, SW_SHOW);
        }
        Ok(EditorWindow {
            hwnd,
            view,
            _frame: frame,
            state,
        })
    }

    /// False once the user has closed the window.
    pub fn is_open(&self) -> bool {
        !self.state.closed.load(Ordering::Relaxed)
    }

    /// Brings the window to the front.
    pub fn focus(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
        }
        self.state.closed.store(false, Ordering::Relaxed);
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            self.view.removed();
            self.view.setFrame(ptr::null_mut());
            DestroyWindow(self.hwnd);
        }
    }
}
