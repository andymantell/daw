// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 C: a native child window inside a UI toolkit's window, for a
//! plugin's editor to draw into.
//!
//! Both toolkit spikes (`ui-timeline`, `ui-timeline-slint`) reserve an area
//! of their window, create a [`Panel`] there, and move it whenever their
//! layout changes. On Windows the panel can host a VST 3 editor
//! (`IPlugView::attached` with "HWND"). On Linux it is a plain X11 child
//! window that draws a test pattern: enough to check, in the cloud, that
//! the toolkit leaves a foreign child window visible and in place. Linux
//! plugin editors also need a host run loop, which this spike does not
//! build.

use raw_window_handle::HasWindowHandle;

/// Physical pixels, relative to the parent window's client area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub struct Panel {
    // The plugin goes before the window so it is detached first.
    #[cfg(windows)]
    plugin: Option<win::Hosted>,
    imp: imp::Panel,
    rect: Rect,
    status: String,
}

impl Panel {
    pub fn new(parent: &impl HasWindowHandle, rect: Rect) -> Result<Panel, String> {
        let handle = parent.window_handle().map_err(|e| e.to_string())?;
        let imp = imp::Panel::new(handle.as_raw(), rect)?;
        Ok(Panel {
            #[cfg(windows)]
            plugin: None,
            imp,
            rect,
            status: "empty panel".into(),
        })
    }

    /// Moves the panel; cheap to call every frame.
    pub fn set_rect(&mut self, rect: Rect) {
        if rect != self.rect {
            self.rect = rect;
            self.imp.set_rect(rect);
        }
        self.imp.poll();
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Opens the editor of the first audio class in `bundle` inside the
    /// panel.
    #[cfg(windows)]
    pub fn open_plugin(&mut self, bundle: &std::path::Path) -> Result<String, String> {
        self.plugin = None;
        let hosted = win::Hosted::open(bundle, self.imp.hwnd())?;
        self.status = hosted.describe();
        self.plugin = Some(hosted);
        Ok(self.status.clone())
    }

    #[cfg(not(windows))]
    pub fn open_plugin(&mut self, _bundle: &std::path::Path) -> Result<String, String> {
        Err("plugin editors are only embedded on Windows in this spike".into())
    }

    /// What the panel holds, for a status line.
    pub fn status(&self) -> String {
        #[cfg(windows)]
        if let Some(p) = &self.plugin {
            return p.describe();
        }
        self.status.clone()
    }
}

#[cfg(windows)]
use win as imp;

#[cfg(target_os = "linux")]
use x11 as imp;

#[cfg(windows)]
mod win {
    #![allow(non_snake_case)]

    use std::ffi::CStr;
    use std::path::Path;
    use std::ptr;
    use std::sync::Mutex;

    use plugin_host::plugin::{Instance, Module};
    use raw_window_handle::RawWindowHandle;
    use vst3::Steinberg::*;
    use vst3::{Class, ComPtr, ComWrapper};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::Rect;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    pub struct Panel {
        hwnd: HWND,
    }

    impl Panel {
        pub fn new(parent: RawWindowHandle, r: Rect) -> Result<Panel, String> {
            let RawWindowHandle::Win32(h) = parent else {
                return Err("not a Win32 window".into());
            };
            let class = wide("DawSpikeEmbedPanel");
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
                    hbrBackground: CreateSolidBrush(0x0033_2a2a),
                    lpszMenuName: ptr::null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc); // Fails harmlessly if already registered.
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    ptr::null(),
                    WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    r.x,
                    r.y,
                    r.w.max(1),
                    r.h.max(1),
                    h.hwnd.get() as HWND,
                    ptr::null_mut(),
                    hinst,
                    ptr::null(),
                )
            };
            if hwnd.is_null() {
                return Err("could not create the child window".into());
            }
            Ok(Panel { hwnd })
        }

        pub fn hwnd(&self) -> HWND {
            self.hwnd
        }

        pub fn set_rect(&mut self, r: Rect) {
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    ptr::null_mut(),
                    r.x,
                    r.y,
                    r.w.max(1),
                    r.h.max(1),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        /// The toolkit's own event loop dispatches messages for every
        /// window on its thread, including ours and the plugin's.
        pub fn poll(&mut self) {}
    }

    impl Drop for Panel {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Records the plugin's resize requests; the host's layout decides
    /// the panel size, so they are reported rather than obeyed.
    struct Frame {
        asked: Mutex<Option<(i32, i32)>>,
    }

    impl Class for Frame {
        type Interfaces = (IPlugFrame,);
    }

    impl IPlugFrameTrait for Frame {
        unsafe fn resizeView(&self, view: *mut IPlugView, rect: *mut ViewRect) -> tresult {
            let r = unsafe { *rect };
            *self.asked.lock().unwrap() = Some((r.right - r.left, r.bottom - r.top));
            if let Some(v) = unsafe { vst3::ComRef::from_raw(view) } {
                unsafe { v.onSize(rect) };
            }
            kResultOk
        }
    }

    pub struct Hosted {
        // Drop order: view, frame, instance, then the module.
        view: ComPtr<IPlugView>,
        frame: ComWrapper<Frame>,
        _instance: Instance,
        _module: Module,
        name: String,
        size: (i32, i32),
    }

    impl Hosted {
        pub fn open(bundle: &Path, parent: HWND) -> Result<Hosted, String> {
            let module = Module::load(bundle).map_err(|e| e.to_string())?;
            let class = module
                .classes()
                .into_iter()
                .find(|c| c.category == "Audio Module Class")
                .ok_or("no audio module class in the plugin")?;
            let instance =
                Instance::new(&module, &class.cid, 48_000.0, 256).map_err(|e| e.to_string())?;
            let view = instance.create_editor().ok_or("the plugin has no editor")?;
            let hwnd_type: &CStr = c"HWND";
            if unsafe { view.isPlatformTypeSupported(hwnd_type.as_ptr()) } != kResultTrue {
                return Err("the editor does not support HWND".into());
            }
            let mut size = ViewRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            unsafe { view.getSize(&mut size) };
            let frame = ComWrapper::new(Frame {
                asked: Mutex::new(None),
            });
            let fp = frame.to_com_ptr::<IPlugFrame>().unwrap();
            unsafe {
                view.setFrame(fp.as_ptr());
                let r = view.attached(parent as *mut _, hwnd_type.as_ptr());
                if r != kResultOk {
                    view.setFrame(ptr::null_mut());
                    return Err(format!("attached() returned {r}"));
                }
            }
            Ok(Hosted {
                view,
                frame,
                _instance: instance,
                _module: module,
                name: class.name,
                size: (size.right - size.left, size.bottom - size.top),
            })
        }

        pub fn describe(&self) -> String {
            let mut s = format!(
                "{} editor embedded, {}x{} px",
                self.name, self.size.0, self.size.1
            );
            if let Some((w, h)) = *self.frame.asked.lock().unwrap() {
                s.push_str(&format!(", asked to resize to {w}x{h}"));
            }
            s
        }
    }

    impl Drop for Hosted {
        fn drop(&mut self) {
            unsafe {
                self.view.removed();
                self.view.setFrame(ptr::null_mut());
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use raw_window_handle::RawWindowHandle;
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        ChangeGCAux, ConfigureWindowAux, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask,
        Rectangle, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;

    use super::Rect;

    /// Our own connection to the X server, so the toolkit's is untouched,
    /// as a plugin's would be.
    pub struct Panel {
        conn: RustConnection,
        win: u32,
        gc: u32,
        rect: Rect,
    }

    impl Panel {
        pub fn new(parent: RawWindowHandle, r: Rect) -> Result<Panel, String> {
            let parent = match parent {
                RawWindowHandle::Xlib(h) => h.window as u32,
                RawWindowHandle::Xcb(h) => h.window.get(),
                _ => return Err("not an X11 window (Wayland is not covered)".into()),
            };
            let (conn, _) = x11rb::connect(None).map_err(|e| e.to_string())?;
            let win = conn.generate_id().map_err(|e| e.to_string())?;
            let gc = conn.generate_id().map_err(|e| e.to_string())?;
            let e = |e: x11rb::errors::ConnectionError| e.to_string();
            conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                win,
                parent,
                r.x as i16,
                r.y as i16,
                r.w.max(1) as u16,
                r.h.max(1) as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .background_pixel(0x002a_2a33)
                    .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS),
            )
            .map_err(e)?;
            conn.create_gc(gc, win, &CreateGCAux::new().foreground(0x00ff_b050))
                .map_err(e)?;
            conn.map_window(win).map_err(e)?;
            conn.flush().map_err(e)?;
            let mut p = Panel {
                conn,
                win,
                gc,
                rect: r,
            };
            p.draw();
            Ok(p)
        }

        pub fn set_rect(&mut self, r: Rect) {
            self.rect = r;
            let _ = self.conn.configure_window(
                self.win,
                &ConfigureWindowAux::new()
                    .x(r.x)
                    .y(r.y)
                    .width(r.w.max(1) as u32)
                    .height(r.h.max(1) as u32),
            );
            self.draw();
        }

        /// Redraws after the X server says the window was uncovered.
        pub fn poll(&mut self) {
            let mut exposed = false;
            while let Ok(Some(ev)) = self.conn.poll_for_event() {
                if let Event::Expose(_) = ev {
                    exposed = true;
                }
            }
            if exposed {
                self.draw();
            }
        }

        /// A stand-in "editor": a grid of knobs drawn with core X calls.
        fn draw(&mut self) {
            let (w, h) = (self.rect.w.max(1) as i16, self.rect.h.max(1) as i16);
            let _ = self.conn.clear_area(false, self.win, 0, 0, 0, 0);
            let _ = self.conn.change_gc(self.gc, &ChangeGCAux::new().line_width(2));
            let _ = self.conn.poly_rectangle(
                self.win,
                self.gc,
                &[Rectangle {
                    x: 4,
                    y: 4,
                    width: (w - 8).max(1) as u16,
                    height: (h - 8).max(1) as u16,
                }],
            );
            let mut arcs = Vec::new();
            let mut x = 24i16;
            while x + 48 < w {
                arcs.push(x11rb::protocol::xproto::Arc {
                    x,
                    y: (h / 2 - 24).max(8),
                    width: 48,
                    height: 48,
                    angle1: -45 * 64,
                    angle2: 270 * 64,
                });
                x += 72;
            }
            let _ = self.conn.poly_arc(self.win, self.gc, &arcs);
            let _ = self.conn.image_text8(
                self.win,
                self.gc,
                12,
                22,
                b"Native child window (stand-in for a plugin editor)",
            );
            let _ = self.conn.flush();
        }
    }

    impl Drop for Panel {
        fn drop(&mut self) {
            let _ = self.conn.destroy_window(self.win);
            let _ = self.conn.flush();
        }
    }
}
