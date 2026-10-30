// SPDX-License-Identifier: GPL-3.0-or-later
//! A CLAP plugin's own window, inside a window of ours (Windows).
//!
//! As for VST 3 editors ([`crate::editor_win`]): the window lives on the
//! UI thread, closing it only hides it and sets a flag, and the app then
//! closes the editor, which has the plugin let go of the window before
//! it is destroyed.
//!
//! **Scaling.** CLAP plugins on Windows size themselves in physical
//! pixels. The plugin is told the display's scale before it is placed in
//! the window and again whenever the window moves to a display with
//! another scale; it then asks for the size it needs, which the app
//! passes on ([`EditorWindow::resize`]).

use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use clap_sys::ext::gui::*;
use clap_sys::plugin::clap_plugin;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::editor_win::{STYLE, outer, scale_of, wide};

/// What the window procedure needs, kept at GWLP_USERDATA.
struct WinState {
    closed: AtomicBool,
    plugin: *const clap_plugin,
    gui: *const clap_plugin_gui,
    resizable: bool,
    dpi: AtomicU32,
}

impl WinState {
    fn gui(&self) -> &clap_plugin_gui {
        unsafe { &*self.gui }
    }
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
                // The plugin asks for its new size through the host.
                if let Some(f) = state.gui().set_scale {
                    unsafe { f(state.plugin, scale_of(dpi) as f64) };
                }
                return 0;
            }
            WM_SIZE if state.resizable => {
                let (mut cw, mut ch) = ((l & 0xFFFF) as u32, ((l >> 16) & 0xFFFF) as u32);
                let g = state.gui();
                unsafe {
                    if let Some(f) = g.adjust_size {
                        f(state.plugin, &mut cw, &mut ch);
                    }
                    if let Some(f) = g.set_size {
                        f(state.plugin, cw, ch);
                    }
                }
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// An open plugin window. Dropping it destroys the window; the plugin's
/// part must be destroyed first.
pub struct EditorWindow {
    hwnd: HWND,
    style: u32,
    state: Box<WinState>,
}

impl EditorWindow {
    /// Puts the plugin's window, already created for this API and not
    /// floating, in a new window titled `title`.
    ///
    /// # Safety
    ///
    /// `plugin` and `gui` must be valid for as long as the window is.
    pub unsafe fn open(
        plugin: *const clap_plugin,
        gui: *const clap_plugin_gui,
        title: &str,
    ) -> Result<EditorWindow, String> {
        let g = unsafe { &*gui };
        let resizable = g.can_resize.is_some_and(|f| unsafe { f(plugin) });
        let style = if resizable {
            STYLE | WS_THICKFRAME | WS_MAXIMIZEBOX
        } else {
            STYLE
        };
        let class = wide("DawClapEditor");
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
            let (ow, oh) = outer(style, 640, 480, 96);
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
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let state = Box::new(WinState {
            closed: AtomicBool::new(false),
            plugin,
            gui,
            resizable,
            dpi: AtomicU32::new(dpi),
        });
        let window = EditorWindow { hwnd, style, state };
        unsafe {
            if let Some(f) = g.set_scale {
                f(plugin, scale_of(dpi) as f64);
            }
            let (mut w, mut h) = (640, 480);
            if let Some(f) = g.get_size {
                f(plugin, &mut w, &mut h);
            }
            window.resize(w, h);
            let parent = clap_window {
                api: CLAP_WINDOW_API_WIN32.as_ptr(),
                specific: clap_window_handle {
                    win32: hwnd as *mut _,
                },
            };
            if g.set_parent.is_none_or(|f| !f(plugin, &parent)) {
                // Dropping the window destroys it.
                return Err("the plugin could not open its editor".into());
            }
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                &*window.state as *const WinState as isize,
            );
            if let Some(f) = g.show {
                f(plugin);
            }
            ShowWindow(hwnd, SW_SHOW);
        }
        Ok(window)
    }

    /// Sizes the window to fit the plugin's part, `w` by `h` pixels.
    pub fn resize(&self, w: u32, h: u32) {
        let dpi = self.state.dpi.load(Ordering::Relaxed);
        let (ow, oh) = outer(self.style, w.max(1) as i32, h.max(1) as i32, dpi);
        unsafe {
            SetWindowPos(
                self.hwnd,
                ptr::null_mut(),
                0,
                0,
                ow,
                oh,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
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
            DestroyWindow(self.hwnd);
        }
    }
}
