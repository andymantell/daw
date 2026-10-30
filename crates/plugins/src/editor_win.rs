// SPDX-License-Identifier: GPL-3.0-or-later
//! A plugin's own editor in a window of its own (Windows).
//!
//! The window lives on the UI thread; the app's event loop dispatches its
//! messages along with its own. Closing it only hides it and sets a flag;
//! the app sees [`EditorWindow::is_open`] go false and drops the editor,
//! which detaches the plugin's view before destroying the window, as the
//! VST 3 lifecycle asks.
//!
//! **Scaling.** The window follows the scale of the display it is on.
//! A plugin that can draw itself at any scale is told the display's
//! scale before its view is attached, and again whenever the window
//! moves to a display with another scale; it then asks for the size it
//! needs. A plugin that can't is drawn as on a display at 100 % and
//! Windows stretches it, so it is the right size if a little soft.

#![allow(non_snake_case)]

use std::ffi::CStr;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The scale of a display at `dpi`, 1.0 being 100 %.
pub fn scale_of(dpi: u32) -> f32 {
    dpi.max(1) as f32 / 96.0
}

/// Makes this process follow each display's scale, so the windows it
/// opens are drawn at the display's own resolution. For the plugin
/// helper process; the app's windows already do.
pub fn per_monitor_dpi() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(crate) const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

/// Outer window size for a given client size, at `dpi`.
pub(crate) fn outer(style: u32, w: i32, h: i32, dpi: u32) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    unsafe { AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi) };
    (r.right - r.left, r.bottom - r.top)
}

/// What the window procedure needs, kept at GWLP_USERDATA.
struct WinState {
    closed: AtomicBool,
    view: *mut IPlugView,
    resizable: bool,
    /// The view's scaling, if it can scale itself.
    scaler: Option<ComPtr<IPlugViewContentScaleSupport>>,
    /// The window's display's DPI, shared with the frame.
    dpi: Arc<AtomicU32>,
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
            WM_DPICHANGED => {
                let dpi = (w & 0xFFFF) as u32;
                state.dpi.store(dpi, Ordering::Relaxed);
                // Where Windows suggests, so the window stays under the
                // pointer while dragged between displays.
                if let Some(r) = unsafe { (l as *const RECT).as_ref() } {
                    unsafe {
                        SetWindowPos(
                            hwnd,
                            ptr::null_mut(),
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        )
                    };
                }
                // The plugin asks for its new size through the frame.
                if let Some(s) = &state.scaler {
                    unsafe { s.setContentScaleFactor(scale_of(dpi)) };
                }
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
    dpi: Arc<AtomicU32>,
}

impl Class for Frame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, view: *mut IPlugView, rect: *mut ViewRect) -> tresult {
        let r = unsafe { *rect };
        let dpi = self.dpi.load(Ordering::Relaxed);
        let (w, h) = outer(self.style, r.right - r.left, r.bottom - r.top, dpi);
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

        // A view that can't scale itself is drawn at 100 % and stretched.
        let scaler = view.cast::<IPlugViewContentScaleSupport>();
        let thread_dpi = scaler.is_none().then(|| unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED)
        });
        let result = Self::create(view, title, scaler, style, resizable, (w, h));
        if let Some(old) = thread_dpi {
            unsafe { SetThreadDpiAwarenessContext(old) };
        }
        result
    }

    fn create(
        view: ComPtr<IPlugView>,
        title: &str,
        scaler: Option<ComPtr<IPlugViewContentScaleSupport>>,
        style: u32,
        resizable: bool,
        (w, h): (i32, i32),
    ) -> Result<EditorWindow, String> {
        let hwnd_type: &CStr = c"HWND";
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
            let (ow, oh) = outer(style, w.max(1), h.max(1), 96);
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
        // 96 for a window drawn at 100 % and stretched.
        let dpi = Arc::new(AtomicU32::new(unsafe { GetDpiForWindow(hwnd) }.max(96)));
        let frame = ComWrapper::new(Frame {
            hwnd: AtomicIsize::new(hwnd as isize),
            style,
            dpi: dpi.clone(),
        });
        let frame_ptr = frame.to_com_ptr::<IPlugFrame>().unwrap();
        unsafe {
            // The size the view wants at this display's scale, and the
            // window to fit it.
            let d = dpi.load(Ordering::Relaxed);
            let mut size = ViewRect {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            if let Some(s) = &scaler {
                s.setContentScaleFactor(scale_of(d));
                view.getSize(&mut size);
            }
            let (ow, oh) = outer(
                style,
                (size.right - size.left).max(1),
                (size.bottom - size.top).max(1),
                d,
            );
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                0,
                0,
                ow,
                oh,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let state = Box::new(WinState {
            closed: AtomicBool::new(false),
            view: view.as_ptr(),
            resizable,
            scaler,
            dpi,
        });
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
