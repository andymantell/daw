// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugins in a helper process (roadmap 2.4).
//!
//! A plugin that crashes takes its process down. When that process is a
//! helper rather than the app, the app carries on: the track goes
//! silent, the effect is marked as crashed, and the user can start it
//! again. The helper is the app itself run with [`HOST_ARG`], talking
//! over its standard input and output in the messages of
//! [`crate::proto`]. A helper can hold any number of plugins (a shared
//! helper), or one (its own process).
//!
//! **Audio.** The audio thread must never wait on another process, so
//! a live [`RemoteProcessor`] runs one block behind: each block it is
//! given goes into a queue for the helper's I/O thread, and what comes
//! back is the helper's answer to the block before. The extra block of
//! delay is reported as latency, which the engine makes up for like a
//! plugin's own. If the helper falls behind (or dies) the block is
//! silent, which is counted, and alignment is kept. An offline
//! processor, for rendering, calls the helper directly and adds no
//! delay.
//!
//! **Hangs.** A watchdog stops a helper that has not answered a request
//! for [`HANG_TIMEOUT`]; that counts as a crash.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};

use daw_engine::graph::Insert;

use crate::module::Result;
use crate::plugin::{Buses, Layout, Library, Param, Plugin, PluginState, Processor};
use crate::proto::{ProgramList, Reply, Request, Status, read_frame, write_frame};

/// The argument that makes the app run as a plugin helper.
pub const HOST_ARG: &str = "--host-plugins";

/// How long a helper may take to answer before it is stopped.
pub const HANG_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a plugin runs. Chosen per plugin by the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sandbox {
    /// In the app: lowest overhead, but a crash takes the app down.
    InApp,
    /// In a helper process shared with other plugins so set.
    #[default]
    Shared,
    /// In a helper process of its own.
    Own,
}

impl Sandbox {
    pub const ALL: [Sandbox; 3] = [Sandbox::InApp, Sandbox::Shared, Sandbox::Own];

    pub fn label(self) -> &'static str {
        match self {
            Sandbox::InApp => "In the app",
            Sandbox::Shared => "Shared helper process",
            Sandbox::Own => "Its own process",
        }
    }

    /// For the plugin list file.
    pub fn key(self) -> &'static str {
        match self {
            Sandbox::InApp => "app",
            Sandbox::Shared => "shared",
            Sandbox::Own => "own",
        }
    }

    pub fn from_key(key: &str) -> Option<Sandbox> {
        Sandbox::ALL.into_iter().find(|s| s.key() == key)
    }
}

// ---------------------------------------------------------------------------
// The helper's side

/// One plugin the helper holds.
struct Held {
    plugin: Plugin,
    processor: Processor,
}

#[derive(Default)]
struct Helper {
    modules: HashMap<PathBuf, Library>,
    plugins: HashMap<u32, Held>,
}

impl Helper {
    fn held(&mut self, id: u32) -> std::result::Result<&mut Held, Reply> {
        self.plugins
            .get_mut(&id)
            .ok_or_else(|| Reply::Error("no such plugin in the helper".into()))
    }

    fn handle(&mut self, req: Request) -> Reply {
        match req {
            Request::Load {
                id,
                bundle,
                cid,
                rate,
                max_block,
                layout,
            } => {
                let bundle = PathBuf::from(bundle);
                let module = match self.modules.get(&bundle) {
                    Some(m) => m.clone(),
                    None => match Library::load(&bundle) {
                        Ok(m) => {
                            self.modules.insert(bundle, m.clone());
                            m
                        }
                        Err(e) => return Reply::Error(e),
                    },
                };
                let tuid = cid.map(|b| b as _);
                match module.instantiate(&tuid, rate, max_block as usize, &layout) {
                    Ok((plugin, processor)) => {
                        let reply = Reply::Loaded {
                            latency: plugin.latency(),
                            params: plugin.params(),
                            buses: plugin.buses(),
                        };
                        self.plugins.insert(id, Held { plugin, processor });
                        reply
                    }
                    Err(e) => Reply::Error(e),
                }
            }
            Request::Unload { id } => {
                self.plugins.remove(&id);
                Reply::Ok
            }
            Request::Process {
                id,
                mut left,
                mut right,
                key,
            } => match self.held(id) {
                Ok(h) => {
                    match &key {
                        Some((kl, kr)) => {
                            h.processor.process_keyed(&mut left, &mut right, (kl, kr))
                        }
                        None => h.processor.process(&mut left, &mut right),
                    }
                    Reply::Audio { left, right }
                }
                Err(e) => e,
            },
            Request::SetParam { id, param, value } => match self.held(id) {
                Ok(h) => {
                    h.plugin.set_param(param, value);
                    Reply::Ok
                }
                Err(e) => e,
            },
            Request::GetParam { id, param } => match self.held(id) {
                Ok(h) => Reply::Value(h.plugin.param(param)),
                Err(e) => e,
            },
            Request::ParamText { id, param, value } => match self.held(id) {
                Ok(h) => Reply::Text(h.plugin.param_text(param, value)),
                Err(e) => e,
            },
            Request::Params { id } => match self.held(id) {
                Ok(h) => Reply::Params(h.plugin.params()),
                Err(e) => e,
            },
            Request::SaveState { id } => match self.held(id) {
                Ok(h) => match h.plugin.save_state() {
                    Ok(s) => Reply::State(s),
                    Err(e) => Reply::Error(e),
                },
                Err(e) => e,
            },
            Request::LoadState { id, state } => match self.held(id) {
                Ok(h) => match h.plugin.load_state(&state) {
                    Ok(()) => Reply::Ok,
                    Err(e) => Reply::Error(e),
                },
                Err(e) => e,
            },
            Request::Poll { id } => match self.held(id) {
                Ok(h) => Reply::Status(h.plugin.poll()),
                Err(e) => e,
            },
            Request::OpenEditor { id, title } => match self.held(id) {
                Ok(h) => match h.plugin.open_editor(&title) {
                    Ok(()) => Reply::Ok,
                    Err(e) => Reply::Error(e),
                },
                Err(e) => e,
            },
            Request::CloseEditor { id } => match self.held(id) {
                Ok(h) => {
                    h.plugin.close_editor();
                    Reply::Ok
                }
                Err(e) => e,
            },
            Request::Programs { id } => match self.held(id) {
                Ok(h) => Reply::Programs(h.plugin.programs()),
                Err(e) => e,
            },
            Request::RemapParam { id, old, param } => match self.held(id) {
                Ok(h) => Reply::ParamId(h.plugin.compatible_param(&old.map(|b| b as _), param)),
                Err(e) => e,
            },
            Request::Ping => Reply::Ok,
            Request::Quit => Reply::Ok,
        }
    }

    fn any_editor_open(&self) -> bool {
        self.plugins.values().any(|h| h.plugin.editor_open())
    }
}

/// Dispatches this thread's pending window messages, so plugin editors
/// opened in the helper stay alive.
#[cfg(windows)]
fn pump_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The helper's main loop: answers requests from standard input on
/// standard output until told to quit or the app goes away. Returns
/// the exit code.
pub fn helper_main() -> i32 {
    // A crashing plugin should end this process quietly, not show a
    // system error box the user has to dismiss.
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::*;
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
    // Plugin windows opened here scale with each display, as the app's
    // own windows do (see `editor_win`).
    #[cfg(windows)]
    crate::editor_win::per_monitor_dpi();
    // Requests are read on a thread so this one can also serve plugin
    // windows while waiting.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut stdin = BufReader::new(std::io::stdin().lock());
        while let Ok(Some(frame)) = read_frame(&mut stdin) {
            if tx.send(frame).is_err() {
                break;
            }
        }
    });
    let mut out = BufWriter::new(std::io::stdout().lock());
    let mut helper = Helper::default();
    loop {
        let frame = if helper.any_editor_open() {
            #[cfg(windows)]
            pump_messages();
            match rx.recv_timeout(Duration::from_millis(1)) {
                Ok(f) => f,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return 0,
            }
        } else {
            match rx.recv() {
                Ok(f) => f,
                Err(_) => return 0,
            }
        };
        let req = match Request::decode(&frame) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_frame(&mut out, &Reply::Error(e).encode());
                return 3;
            }
        };
        let quit = req == Request::Quit;
        let reply = helper.handle(req);
        if write_frame(&mut out, &reply.encode()).is_err() || quit {
            return 0;
        }
    }
}

// ---------------------------------------------------------------------------
// The app's side

/// The pipes to a helper.
struct Link {
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

/// What every side of a helper shares.
struct Shared {
    exe: PathBuf,
    link: Mutex<Option<Link>>,
    child: Mutex<Child>,
    /// Why the helper is gone, once it is.
    dead: Mutex<Option<String>>,
    /// The same, for the audio thread, which must not lock.
    dead_flag: AtomicBool,
    /// Set by the watchdog when it stops a hung helper.
    hung: AtomicBool,
    /// When the request in flight was sent.
    busy_since: Mutex<Option<Instant>>,
    next_id: AtomicU32,
    /// Live processors hand their queues to the I/O thread here.
    register: Mutex<mpsc::Sender<IoSide>>,
    /// The I/O thread, woken when a block is queued.
    io_thread: OnceLock<thread::Thread>,
}

impl Shared {
    fn is_dead(&self) -> Option<String> {
        self.dead.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn mark_dead(&self, why: &str) -> String {
        let mut dead = self.dead.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = &*dead {
            return d.clone();
        }
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let status = child.wait().ok();
        *self.link.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let reason = if self.hung.load(Ordering::Relaxed) {
            format!(
                "the plugin stopped answering for {} s and was stopped",
                HANG_TIMEOUT.as_secs()
            )
        } else {
            match status.and_then(|s| s.code()) {
                Some(c) => format!("{why} (exit code {c})"),
                None => why.to_string(),
            }
        };
        *dead = Some(reason.clone());
        self.dead_flag.store(true, Ordering::Relaxed);
        reason
    }

    /// Sends `req` and waits for the answer.
    fn call(&self, req: &Request) -> Result<Reply> {
        let mut link = self.link.lock().unwrap_or_else(|e| e.into_inner());
        let Some(l) = link.as_mut() else {
            return Err(self
                .is_dead()
                .unwrap_or_else(|| "the plugin process is gone".into()));
        };
        *self.busy_since.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        let r = write_frame(&mut l.stdin, &req.encode()).and_then(|()| read_frame(&mut l.stdout));
        *self.busy_since.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let frame = match r {
            Ok(Some(f)) => f,
            Ok(None) | Err(_) => {
                drop(link);
                return Err(self.mark_dead("the plugin process crashed"));
            }
        };
        drop(link);
        match Reply::decode(&frame) {
            Ok(Reply::Error(e)) => Err(e),
            Ok(r) => Ok(r),
            Err(e) => Err(self.mark_dead(&format!("the plugin process sent nonsense ({e})"))),
        }
    }

    fn wake_io(&self) {
        if let Some(t) = self.io_thread.get() {
            t.unpark();
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if self.is_dead().is_none() {
            let _ = self.call(&Request::Quit);
        }
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A helper process. Cloning shares it; it exits when the last clone
/// and every plugin in it are gone.
#[derive(Clone)]
pub struct Host(Arc<Shared>);

impl Host {
    /// Starts `exe` as a helper.
    pub fn spawn(exe: &Path) -> Result<Host> {
        let mut child = Command::new(exe)
            .arg(HOST_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start the plugin process: {e}"))?;
        let link = Link {
            stdin: BufWriter::new(child.stdin.take().unwrap()),
            stdout: BufReader::new(child.stdout.take().unwrap()),
        };
        let (reg_tx, reg_rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            exe: exe.to_owned(),
            link: Mutex::new(Some(link)),
            child: Mutex::new(child),
            dead: Mutex::new(None),
            dead_flag: AtomicBool::new(false),
            hung: AtomicBool::new(false),
            busy_since: Mutex::new(None),
            next_id: AtomicU32::new(1),
            register: Mutex::new(reg_tx),
            io_thread: OnceLock::new(),
        });
        let host = Host(shared.clone());
        host.0.call(&Request::Ping)?;

        let weak = Arc::downgrade(&shared);
        let io = thread::Builder::new()
            .name("plugin-io".into())
            .spawn(move || io_main(weak, reg_rx))
            .map_err(|e| format!("could not start the plugin I/O thread: {e}"))?;
        let _ = shared.io_thread.set(io.thread().clone());

        let weak = Arc::downgrade(&shared);
        let _ = thread::Builder::new()
            .name("plugin-watchdog".into())
            .spawn(move || watchdog_main(weak));
        Ok(host)
    }

    /// Where the helper came from.
    pub fn exe(&self) -> &Path {
        &self.0.exe
    }

    /// Why the helper is gone, if it is.
    pub fn failure(&self) -> Option<String> {
        self.0.is_dead()
    }

    /// Loads a plugin into the helper. With `live_block`, the processor
    /// runs one block of that many frames behind, for the audio thread;
    /// without, it waits for the helper, for rendering.
    pub fn instantiate(
        &self,
        bundle: &Path,
        cid: &vst3::Steinberg::TUID,
        rate: f64,
        max_block: usize,
        live_block: Option<usize>,
    ) -> Result<(RemotePlugin, RemoteProcessor)> {
        self.instantiate_with(bundle, cid, rate, max_block, live_block, &Layout::default())
    }

    /// As [`instantiate`](Self::instantiate), also switching on the
    /// buses `layout` asks for.
    pub fn instantiate_with(
        &self,
        bundle: &Path,
        cid: &vst3::Steinberg::TUID,
        rate: f64,
        max_block: usize,
        live_block: Option<usize>,
        layout: &Layout,
    ) -> Result<(RemotePlugin, RemoteProcessor)> {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        let reply = self.0.call(&Request::Load {
            id,
            bundle: bundle.to_string_lossy().into_owned(),
            cid: cid.map(|b| b as u8),
            rate,
            max_block: max_block as u32,
            layout: layout.clone(),
        })?;
        let Reply::Loaded {
            latency,
            params,
            buses,
        } = reply
        else {
            return Err("unexpected answer to loading a plugin".into());
        };
        // A sidechain goes down the pipe beside the track's audio.
        let width = if layout.sidechain && buses.sidechain().is_some() {
            4
        } else {
            2
        };
        let instance = Arc::new(Instance {
            id,
            host: self.0.clone(),
        });
        let mut late = None;
        let processor = match live_block {
            Some(block) => {
                let block = block.max(1);
                let frames = max_block.max(block) + 2 * block;
                let (in_tx, in_rx) = RingBuffer::<f32>::new(width * frames);
                let (mut out_tx, out_rx) = RingBuffer::<f32>::new(2 * frames);
                // One block of silence puts the output a block behind.
                for _ in 0..2 * block {
                    let _ = out_tx.push(0.0);
                }
                let state = Arc::new(ProcState {
                    gone: AtomicBool::new(false),
                    late: AtomicU64::new(0),
                    extra: AtomicU32::new(block as u32),
                });
                late = Some(state.clone());
                let (params_tx, params_rx) = RingBuffer::new(PARAM_SETS);
                let side = IoSide {
                    instance: instance.clone(),
                    state: state.clone(),
                    input: in_rx,
                    output: out_tx,
                    params: params_rx,
                    max_block,
                    width,
                };
                let _ = self.0.register.lock().unwrap().send(side);
                RemoteProcessor::Live(LiveProcessor {
                    _instance: instance.clone(),
                    state,
                    input: in_tx,
                    output: out_rx,
                    params: params_tx,
                    width,
                    skip: 0,
                    pad: 0,
                })
            }
            None => RemoteProcessor::Offline(OfflineProcessor {
                instance: instance.clone(),
                max_block,
                keyed: width == 4,
                params: Vec::new(),
            }),
        };
        let plugin = RemotePlugin {
            instance,
            params: Mutex::new(params),
            latency: AtomicU32::new(latency),
            editor_open: AtomicBool::new(false),
            late,
            buses,
        };
        Ok((plugin, processor))
    }
}

/// Stops a helper that has not answered for [`HANG_TIMEOUT`].
fn watchdog_main(shared: Weak<Shared>) {
    loop {
        thread::sleep(Duration::from_millis(100));
        let Some(s) = shared.upgrade() else {
            return;
        };
        if s.is_dead().is_some() {
            return;
        }
        let stuck = s
            .busy_since
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|t| t.elapsed() > HANG_TIMEOUT);
        if stuck {
            s.hung.store(true, Ordering::Relaxed);
            // Killing it ends the caller's read, which marks the helper
            // dead with the reason above.
            let _ = s.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
            return;
        }
    }
}

/// A plugin in a helper, unloaded when both its sides are gone.
struct Instance {
    id: u32,
    host: Arc<Shared>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        if self.host.is_dead().is_none() {
            let _ = self.host.call(&Request::Unload { id: self.id });
        }
    }
}

/// The UI side of a plugin in a helper.
pub struct RemotePlugin {
    instance: Arc<Instance>,
    params: Mutex<Vec<Param>>,
    latency: AtomicU32,
    editor_open: AtomicBool,
    late: Option<Arc<ProcState>>,
    buses: Buses,
}

impl RemotePlugin {
    fn id(&self) -> u32 {
        self.instance.id
    }

    fn call(&self, req: Request) -> Result<Reply> {
        self.instance.host.call(&req)
    }

    pub fn latency(&self) -> u32 {
        self.latency.load(Ordering::Relaxed) + self.extra()
    }

    pub fn buses(&self) -> &Buses {
        &self.buses
    }

    pub fn compatible_param(&self, old_cid: &vst3::Steinberg::TUID, old: u32) -> Option<u32> {
        match self.call(Request::RemapParam {
            id: self.id(),
            old: old_cid.map(|b| b as u8),
            param: old,
        }) {
            Ok(Reply::ParamId(p)) => p,
            _ => None,
        }
    }

    pub fn failure(&self) -> Option<String> {
        self.instance.host.is_dead()
    }

    /// Blocks the live processor played as silence because the helper
    /// was late with them.
    pub fn late(&self) -> u64 {
        self.late
            .as_ref()
            .map_or(0, |s| s.late.load(Ordering::Relaxed))
    }

    /// Frames the live processor adds on top of the plugin's own delay.
    fn extra(&self) -> u32 {
        self.late
            .as_ref()
            .map_or(0, |s| s.extra.load(Ordering::Relaxed))
    }

    pub fn poll(&mut self) -> Status {
        let mut status = match self.call(Request::Poll { id: self.id() }) {
            Ok(Reply::Status(s)) => s,
            _ => {
                return Status {
                    latency: self.latency(),
                    ..Status::default()
                };
            }
        };
        if status.restart
            && let Ok(Reply::Params(p)) = self.call(Request::Params { id: self.id() })
        {
            *self.params.lock().unwrap_or_else(|e| e.into_inner()) = p;
        }
        self.latency.store(status.latency, Ordering::Relaxed);
        self.editor_open
            .store(status.editor_open, Ordering::Relaxed);
        status.latency += self.extra();
        status
    }

    pub fn params(&self) -> Vec<Param> {
        self.params
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn param(&self, id: u32) -> Option<f64> {
        match self.call(Request::GetParam {
            id: self.id(),
            param: id,
        }) {
            Ok(Reply::Value(v)) => v,
            _ => None,
        }
    }

    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        match self.call(Request::ParamText {
            id: self.id(),
            param: id,
            value,
        }) {
            Ok(Reply::Text(t)) => t,
            _ => None,
        }
    }

    pub fn set_param(&self, id: u32, value: f64) {
        let _ = self.call(Request::SetParam {
            id: self.id(),
            param: id,
            value,
        });
    }

    pub fn save_state(&self) -> Result<PluginState> {
        match self.call(Request::SaveState { id: self.id() })? {
            Reply::State(s) => Ok(s),
            _ => Err("unexpected answer to saving state".into()),
        }
    }

    pub fn load_state(&self, state: &PluginState) -> Result<()> {
        self.call(Request::LoadState {
            id: self.id(),
            state: state.clone(),
        })
        .map(|_| ())
    }

    pub fn programs(&self) -> Vec<ProgramList> {
        match self.call(Request::Programs { id: self.id() }) {
            Ok(Reply::Programs(p)) => p,
            _ => Vec::new(),
        }
    }

    pub fn open_editor(&mut self, title: &str) -> Result<()> {
        self.call(Request::OpenEditor {
            id: self.id(),
            title: title.to_string(),
        })?;
        self.editor_open.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn editor_open(&self) -> bool {
        self.editor_open.load(Ordering::Relaxed)
    }

    pub fn close_editor(&mut self) {
        let _ = self.call(Request::CloseEditor { id: self.id() });
        self.editor_open.store(false, Ordering::Relaxed);
    }
}

/// What a live processor and the I/O thread share.
struct ProcState {
    /// The processor was dropped; the I/O thread lets its queues go.
    gone: AtomicBool,
    /// Blocks (or parts) that came back too late and played as silence.
    late: AtomicU64,
    /// Frames the queues put between input and output: the block size
    /// asked for, grown to the largest block the engine has sent.
    extra: AtomicU32,
}

/// Settings the engine can set on a live processor between two blocks.
const PARAM_SETS: usize = 256;

/// The I/O thread's end of a live processor's queues.
struct IoSide {
    instance: Arc<Instance>,
    state: Arc<ProcState>,
    input: Consumer<f32>,
    output: Producer<f32>,
    /// Settings the engine set for the next block (automation lanes).
    params: Consumer<(u32, f64)>,
    max_block: usize,
    /// Samples per frame in `input`: left and right, then the
    /// sidechain's left and right if it listens to one.
    width: usize,
}

/// Runs queued blocks through the helper, one request per block.
fn io_main(shared: Weak<Shared>, register: mpsc::Receiver<IoSide>) {
    let mut sides: Vec<IoSide> = Vec::new();
    loop {
        thread::park_timeout(Duration::from_millis(5));
        let Some(s) = shared.upgrade() else {
            return;
        };
        while let Ok(side) = register.try_recv() {
            sides.push(side);
        }
        sides.retain(|s| !s.state.gone.load(Ordering::Relaxed));
        if s.is_dead().is_some() {
            drop(s);
            // Nothing more will come back; sleep until dropped.
            if shared.upgrade().is_none() {
                return;
            }
            continue;
        }
        for side in &mut sides {
            // Never more than the output queue has room for, so nothing
            // is lost between the two.
            let frames = (side.input.slots() / side.width)
                .min(side.output.slots() / 2)
                .min(side.max_block);
            if frames == 0 {
                continue;
            }
            let mut take = || side.input.pop().unwrap_or(0.0);
            let (mut l, mut r) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
            let mut key =
                (side.width == 4).then(|| (Vec::with_capacity(frames), Vec::with_capacity(frames)));
            for _ in 0..frames {
                l.push(take());
                r.push(take());
                if let Some((kl, kr)) = &mut key {
                    kl.push(take());
                    kr.push(take());
                }
            }
            // Settings the engine set go first, so the block hears them.
            while let Ok((param, value)) = side.params.pop() {
                let _ = s.call(&Request::SetParam {
                    id: side.instance.id,
                    param,
                    value,
                });
            }
            let reply = s.call(&Request::Process {
                id: side.instance.id,
                left: l.clone(),
                right: r.clone(),
                key,
            });
            let (left, right) = match reply {
                Ok(Reply::Audio { left, right })
                    if left.len() == frames && right.len() == frames =>
                {
                    (left, right)
                }
                // A plugin that fails a block leaves the audio as it was.
                _ => (l, r),
            };
            for (a, b) in left.iter().zip(&right) {
                if side.output.push(*a).is_err() || side.output.push(*b).is_err() {
                    break;
                }
            }
        }
    }
}

/// The audio thread's end: an [`Insert`] that never waits.
pub struct LiveProcessor {
    _instance: Arc<Instance>,
    state: Arc<ProcState>,
    input: Producer<f32>,
    output: Consumer<f32>,
    /// Settings the engine sets (automation lanes), for the I/O thread
    /// to hand to the helper before the next block.
    params: Producer<(u32, f64)>,
    /// Samples per frame going in (see [`IoSide::width`]).
    width: usize,
    /// Frames of late output to throw away, so the delay stays fixed.
    skip: usize,
    /// Frames of silence to play before the next output, making up for
    /// input that did not fit in the queue.
    pad: usize,
}

impl LiveProcessor {
    /// Blocks that played as silence because the helper was late.
    pub fn late(&self) -> u64 {
        self.state.late.load(Ordering::Relaxed)
    }

    /// Frames the helper has answered that have not been played yet.
    pub fn available(&self) -> usize {
        self.output.slots() / 2
    }
}

impl Drop for LiveProcessor {
    fn drop(&mut self) {
        self.state.gone.store(true, Ordering::Relaxed);
    }
}

impl LiveProcessor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let n = left.len().min(right.len());
        if self._instance.host.dead_flag.load(Ordering::Relaxed) {
            // Pass the audio through unchanged.
            return;
        }
        // A block bigger than the queues were set up for would never be
        // back in time: move the output that much further behind, once.
        // The UI sees the new delay on its next poll and rebuilds.
        let extra = self.state.extra.load(Ordering::Relaxed) as usize;
        if n > extra {
            self.pad += n - extra;
            self.state.extra.store(n as u32, Ordering::Relaxed);
        }
        // Whole frames only, so the queue never comes out of step.
        let fit = (self.input.slots() / self.width).min(n);
        let dropped = n - fit;
        let k = |c: Option<&[f32]>, i: usize| c.and_then(|c| c.get(i)).copied().unwrap_or(0.0);
        for i in 0..fit {
            let _ = self.input.push(left[i]);
            let _ = self.input.push(right[i]);
            if self.width == 4 {
                let _ = self.input.push(k(key.map(|k| k.0), i));
                let _ = self.input.push(k(key.map(|k| k.1), i));
            }
        }
        self._instance.host.wake_io();
        while self.skip > 0 {
            if self.output.pop().is_err() || self.output.pop().is_err() {
                break;
            }
            self.skip -= 1;
        }
        let mut missing = 0;
        for (l, r) in left[..n].iter_mut().zip(&mut right[..n]) {
            if self.pad > 0 {
                self.pad -= 1;
                *l = 0.0;
                *r = 0.0;
                continue;
            }
            match (self.output.pop(), self.output.pop()) {
                (Ok(a), Ok(b)) => {
                    *l = a;
                    *r = b;
                }
                _ => {
                    *l = 0.0;
                    *r = 0.0;
                    missing += 1;
                }
            }
        }
        self.skip += missing;
        self.pad += dropped;
        if missing > 0 || dropped > 0 {
            self.state.late.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// An [`Insert`] that waits for the helper, for rendering off the
/// audio thread.
pub struct OfflineProcessor {
    instance: Arc<Instance>,
    max_block: usize,
    /// It listens to a sidechain.
    keyed: bool,
    /// Settings the engine set for the next block (automation lanes).
    params: Vec<(u32, f64)>,
}

impl OfflineProcessor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let n = left.len().min(right.len());
        for (param, value) in self.params.drain(..) {
            let _ = self.instance.host.call(&Request::SetParam {
                id: self.instance.id,
                param,
                value,
            });
        }
        let mut done = 0;
        while done < n {
            let end = (done + self.max_block).min(n);
            let part = |c: &[f32]| {
                let mut v = c.get(done..end.min(c.len())).unwrap_or(&[]).to_vec();
                v.resize(end - done, 0.0);
                v
            };
            let key = self.keyed.then(|| match key {
                Some((kl, kr)) => (part(kl), part(kr)),
                None => (vec![0.0; end - done], vec![0.0; end - done]),
            });
            let reply = self.instance.host.call(&Request::Process {
                id: self.instance.id,
                left: left[done..end].to_vec(),
                right: right[done..end].to_vec(),
                key,
            });
            if let Ok(Reply::Audio { left: l, right: r }) = reply
                && l.len() == end - done
                && r.len() == end - done
            {
                left[done..end].copy_from_slice(&l);
                right[done..end].copy_from_slice(&r);
            }
            done = end;
        }
    }
}

/// The audio side of a plugin in a helper.
pub enum RemoteProcessor {
    Live(LiveProcessor),
    Offline(OfflineProcessor),
}

impl RemoteProcessor {
    /// Blocks that played as silence because the helper was late (live
    /// processors only).
    pub fn late(&self) -> u64 {
        match self {
            RemoteProcessor::Live(p) => p.late(),
            RemoteProcessor::Offline(_) => 0,
        }
    }

    /// Frames answered by the helper and waiting to be played (live
    /// processors only).
    pub fn available(&self) -> usize {
        match self {
            RemoteProcessor::Live(p) => p.available(),
            RemoteProcessor::Offline(_) => 0,
        }
    }
}

impl Insert for RemoteProcessor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self {
            RemoteProcessor::Live(p) => p.process(left, right, None),
            RemoteProcessor::Offline(p) => p.process(left, right, None),
        }
    }

    fn process_keyed(&mut self, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        match self {
            RemoteProcessor::Live(p) => p.process(left, right, Some(key)),
            RemoteProcessor::Offline(p) => p.process(left, right, Some(key)),
        }
    }

    /// A setting for the next block, from an automation lane: the
    /// helper is told before the block, so the plugin's window there
    /// shows it too. A live processor's queue has room for a block's
    /// worth; more in one block are left out.
    fn set_param(&mut self, param: u32, value: f64) {
        match self {
            RemoteProcessor::Live(p) => {
                let _ = p.params.push((param, value));
            }
            RemoteProcessor::Offline(p) => p.params.push((param, value)),
        }
    }
}
