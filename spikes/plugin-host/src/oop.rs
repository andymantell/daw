// SPDX-License-Identifier: GPL-3.0-or-later
//! Out-of-process plugin hosting, so a plugin that crashes or hangs
//! cannot take the engine down.
//!
//! Each plugin runs in its own child process (`plugin-host serve`). Audio
//! passes through a shared memory file. The host writes a block of input,
//! bumps a request counter and wakes the child; the child processes the
//! block, bumps a response counter and wakes the host. Waking uses a futex
//! on Linux and named events on Windows; elsewhere it spins.
//!
//! If the child misses its deadline the host plays silence for that
//! plugin, and if the child has died the host starts a new one and restores
//! the plugin's last saved state.

use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use memmap2::MmapMut;

use crate::plugin::{Instance, Module, Result};

pub const MAX_FRAMES: usize = 4096;
pub const CHANNELS: usize = 2;
const STATE_MAX: usize = 1 << 20;

const READY_STARTING: u32 = 0;
const READY_OK: u32 = 1;
const READY_FAILED: u32 = 2;

const CMD_PROCESS: u32 = 1;
const CMD_SAVE: u32 = 2;
const CMD_LOAD: u32 = 3;
const CMD_QUIT: u32 = 4;

/// The shared memory layout. Both processes map the same file.
#[repr(C)]
struct Shared {
    req: AtomicU32,
    resp: AtomicU32,
    ready: AtomicU32,
    cmd: AtomicU32,
    /// 0 when the last command worked.
    status: AtomicU32,
    frames: AtomicU32,
    latency: AtomicU32,
    state_len: AtomicU32,
    input: [[f32; MAX_FRAMES]; CHANNELS],
    output: [[f32; MAX_FRAMES]; CHANNELS],
    state: [u8; STATE_MAX],
}

fn shared(map: &MmapMut) -> &Shared {
    // The file is sized to Shared and mapped page-aligned.
    unsafe { &*(map.as_ptr() as *const Shared) }
}

#[allow(clippy::mut_from_ref)]
fn shared_mut(map: &MmapMut) -> &mut Shared {
    // Each side writes only the parts the protocol gives it at that moment.
    unsafe { &mut *(map.as_ptr() as *mut Shared) }
}

// ---------------------------------------------------------------------------
// Waking the other side.

#[cfg(target_os = "linux")]
mod wake {
    use super::*;

    pub struct Waker;

    impl Waker {
        pub fn new(_name: &str, _create: bool) -> Waker {
            Waker
        }
        pub fn wake(&self, word: &AtomicU32) {
            unsafe {
                libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAKE, 1);
            }
        }
        /// Waits until `word` differs from `seen` or the timeout passes.
        pub fn wait(&self, word: &AtomicU32, seen: u32, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            loop {
                if word.load(Ordering::Acquire) != seen {
                    return true;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                let ts = libc::timespec {
                    tv_sec: left.as_secs() as _,
                    tv_nsec: left.subsec_nanos() as _,
                };
                unsafe {
                    libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAIT, seen, &ts);
                }
            }
        }
    }
}

#[cfg(windows)]
mod wake {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

    /// A named auto-reset event; one per direction.
    pub struct Waker(HANDLE);

    unsafe impl Send for Waker {}

    impl Waker {
        pub fn new(name: &str, _create: bool) -> Waker {
            let w: Vec<u16> = format!("Local\\{name}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            // Creates the event, or opens it if the other side already did.
            Waker(unsafe { CreateEventW(std::ptr::null(), 0, 0, w.as_ptr()) })
        }
        pub fn wake(&self, _word: &AtomicU32) {
            unsafe { SetEvent(self.0) };
        }
        pub fn wait(&self, word: &AtomicU32, seen: u32, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            loop {
                if word.load(Ordering::Acquire) != seen {
                    return true;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                let ms = left.as_micros().div_ceil(1000) as u32;
                unsafe { WaitForSingleObject(self.0, ms) };
            }
        }
    }

    impl Drop for Waker {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod wake {
    use super::*;

    pub struct Waker;

    impl Waker {
        pub fn new(_name: &str, _create: bool) -> Waker {
            Waker
        }
        pub fn wake(&self, _word: &AtomicU32) {}
        pub fn wait(&self, word: &AtomicU32, seen: u32, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            while word.load(Ordering::Acquire) == seen {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::yield_now();
            }
            true
        }
    }
}

use wake::Waker;

/// Asks the OS to treat the calling thread as an audio thread. Returns
/// what was granted, for the report.
pub fn realtime_thread() -> &'static str {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        let name: Vec<u16> = "Pro Audio".encode_utf16().chain(Some(0)).collect();
        let mut index = 0u32;
        let h = unsafe { AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut index) };
        if h.is_null() {
            "normal priority (MMCSS refused)"
        } else {
            "MMCSS Pro Audio"
        }
    }
    #[cfg(target_os = "linux")]
    {
        let p = libc::sched_param { sched_priority: 70 };
        if unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &p) } == 0 {
            "SCHED_FIFO"
        } else {
            "normal priority (no permission for SCHED_FIFO)"
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        "normal priority"
    }
}

fn map_file(path: &Path, create: bool) -> std::io::Result<MmapMut> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(create)
        .open(path)?;
    if create {
        f.set_len(std::mem::size_of::<Shared>() as u64)?;
    }
    unsafe { MmapMut::map_mut(&f) }
}

// ---------------------------------------------------------------------------
// Host side.

/// Why a block came back without the plugin's audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Miss {
    /// The child answered too late; it may still be alive.
    Late,
    /// The child process has exited.
    Died,
}

pub struct Remote {
    exe: PathBuf,
    bundle: PathBuf,
    block: usize,
    file: PathBuf,
    id: String,
    map: MmapMut,
    to_child: Waker,
    to_host: Waker,
    child: Child,
    seq: u32,
    pub latency: u32,
    pub restarts: u32,
    saved: Option<Vec<u8>>,
}

impl Remote {
    pub fn spawn(exe: &Path, bundle: &Path, block: usize) -> Result<Remote> {
        if block > MAX_FRAMES {
            return Err(format!("block over {MAX_FRAMES}"));
        }
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let id = format!(
            "daw-spike-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        );
        let file = std::env::temp_dir().join(format!("{id}.shm"));
        let map = map_file(&file, true).map_err(|e| format!("shared memory: {e}"))?;
        let to_child = Waker::new(&format!("{id}-req"), true);
        let to_host = Waker::new(&format!("{id}-resp"), true);
        let child = Self::start(exe, bundle, &file, &id, block)?;
        let mut r = Remote {
            exe: exe.into(),
            bundle: bundle.into(),
            block,
            file,
            id,
            map,
            to_child,
            to_host,
            child,
            seq: 0,
            latency: 0,
            restarts: 0,
            saved: None,
        };
        r.wait_ready()?;
        Ok(r)
    }

    fn start(exe: &Path, bundle: &Path, file: &Path, id: &str, block: usize) -> Result<Child> {
        Command::new(exe)
            .arg("serve")
            .arg(bundle)
            .arg(file)
            .arg(id)
            .arg(block.to_string())
            // The child exits when this pipe closes, so it never outlives us.
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start plugin process: {e}"))
    }

    fn wait_ready(&mut self) -> Result<()> {
        let sh = shared(&self.map);
        let start = Instant::now();
        loop {
            match sh.ready.load(Ordering::Acquire) {
                READY_OK => break,
                READY_FAILED => return Err("plugin process could not load the plugin".into()),
                _ => {}
            }
            if let Ok(Some(st)) = self.child.try_wait() {
                return Err(format!("plugin process exited during load ({st})"));
            }
            if start.elapsed() > Duration::from_secs(60) {
                return Err("plugin process took over 60 s to load".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.latency = sh.latency.load(Ordering::Acquire);
        self.seq = sh.resp.load(Ordering::Acquire);
        Ok(())
    }

    /// Sends a command and waits up to `timeout` for the answer.
    fn call(&mut self, cmd: u32, timeout: Duration) -> std::result::Result<(), Miss> {
        let sh = shared(&self.map);
        let seen = sh.resp.load(Ordering::Acquire);
        if seen != self.seq {
            // A late answer to an earlier block has arrived; resynchronise.
            self.seq = seen;
        }
        sh.cmd.store(cmd, Ordering::Relaxed);
        self.seq = self.seq.wrapping_add(1);
        sh.req.store(self.seq, Ordering::Release);
        self.to_child.wake(&sh.req);
        if self.to_host.wait(&sh.resp, seen, timeout) && sh.resp.load(Ordering::Acquire) == self.seq
        {
            Ok(())
        } else if matches!(self.child.try_wait(), Ok(Some(_))) {
            Err(Miss::Died)
        } else {
            Err(Miss::Late)
        }
    }

    /// Processes one block. On a miss, `output` is silence.
    pub fn process(
        &mut self,
        input: &[&[f32]; CHANNELS],
        output: &mut [Vec<f32>; CHANNELS],
        timeout: Duration,
    ) -> std::result::Result<(), Miss> {
        let n = self.block;
        {
            let sh = shared_mut(&self.map);
            for (dst, src) in sh.input.iter_mut().zip(input) {
                dst[..n].copy_from_slice(&src[..n]);
            }
            sh.frames.store(n as u32, Ordering::Relaxed);
        }
        let r = self.call(CMD_PROCESS, timeout);
        let sh = shared(&self.map);
        for (dst, src) in output.iter_mut().zip(&sh.output) {
            if r.is_ok() {
                dst[..n].copy_from_slice(&src[..n]);
            } else {
                dst[..n].fill(0.0);
            }
        }
        r
    }

    /// Asks the plugin for its state and keeps it for restarts.
    pub fn save_state(&mut self) -> Result<Vec<u8>> {
        self.call(CMD_SAVE, Duration::from_secs(10))
            .map_err(|m| format!("save state: {m:?}"))?;
        let sh = shared(&self.map);
        if sh.status.load(Ordering::Acquire) != 0 {
            return Err("plugin could not save its state".into());
        }
        let len = sh.state_len.load(Ordering::Acquire) as usize;
        let bytes = sh.state[..len].to_vec();
        self.saved = Some(bytes.clone());
        Ok(bytes)
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > STATE_MAX {
            return Err("state too large for this spike".into());
        }
        {
            let sh = shared_mut(&self.map);
            sh.state[..bytes.len()].copy_from_slice(bytes);
            sh.state_len.store(bytes.len() as u32, Ordering::Release);
        }
        self.call(CMD_LOAD, Duration::from_secs(10))
            .map_err(|m| format!("load state: {m:?}"))?;
        if shared(&self.map).status.load(Ordering::Acquire) != 0 {
            return Err("plugin refused its state".into());
        }
        Ok(())
    }

    /// Simulates a crash.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Starts a fresh child and restores the last saved state.
    pub fn restart(&mut self) -> Result<()> {
        self.kill();
        {
            let sh = shared(&self.map);
            sh.ready.store(READY_STARTING, Ordering::Release);
        }
        self.child = Self::start(&self.exe, &self.bundle, &self.file, &self.id, self.block)?;
        self.wait_ready()?;
        self.restarts += 1;
        if let Some(s) = self.saved.clone() {
            self.load_state(&s)?;
        }
        Ok(())
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        if self.is_alive() {
            let _ = self.call(CMD_QUIT, Duration::from_millis(500));
        }
        self.kill();
        let _ = std::fs::remove_file(&self.file);
    }
}

// ---------------------------------------------------------------------------
// Child side.

/// Runs in the child process until told to quit or the host goes away.
pub fn serve(bundle: &Path, file: &Path, id: &str, block: usize) -> Result<()> {
    // Exit as soon as the host's end of stdin closes, even mid-block.
    std::thread::spawn(|| {
        let mut buf = [0u8; 64];
        while matches!(std::io::stdin().read(&mut buf), Ok(n) if n > 0) {}
        std::process::exit(0);
    });

    realtime_thread();
    let map = map_file(file, false).map_err(|e| format!("shared memory: {e}"))?;
    let to_child = Waker::new(&format!("{id}-req"), false);
    let to_host = Waker::new(&format!("{id}-resp"), false);
    let sh = shared(&map);

    let loaded = (|| -> Result<(Module, Instance)> {
        let module = Module::load(bundle)?;
        let class = module
            .classes()
            .into_iter()
            .find(|c| c.category == "Audio Module Class")
            .ok_or("no audio module class in factory")?;
        let inst = Instance::new(&module, &class.cid, crate::checks::RATE, block)?;
        Ok((module, inst))
    })();
    let (_module, inst) = match loaded {
        Ok(v) => v,
        Err(e) => {
            sh.ready.store(READY_FAILED, Ordering::Release);
            return Err(e);
        }
    };
    sh.latency.store(inst.latency(), Ordering::Release);
    let mut seen = sh.req.load(Ordering::Acquire);
    sh.resp.store(seen, Ordering::Release);
    sh.ready.store(READY_OK, Ordering::Release);

    loop {
        if !to_child.wait(&sh.req, seen, Duration::from_secs(1)) {
            continue;
        }
        seen = sh.req.load(Ordering::Acquire);
        let shm = shared_mut(&map);
        let ok = match shm.cmd.load(Ordering::Relaxed) {
            CMD_PROCESS => {
                let n = (shm.frames.load(Ordering::Relaxed) as usize).min(block);
                let input: Vec<Vec<f32>> =
                    (0..CHANNELS).map(|c| shm.input[c][..n].to_vec()).collect();
                match inst.process(&input, None) {
                    Ok(out) => {
                        for c in 0..CHANNELS {
                            match out.get(c).or(out.first()) {
                                Some(o) => shm.output[c][..n].copy_from_slice(&o[..n]),
                                None => shm.output[c][..n].fill(0.0),
                            }
                        }
                        true
                    }
                    Err(_) => false,
                }
            }
            CMD_SAVE => match inst.save_state() {
                Ok((bytes, _)) if bytes.len() <= STATE_MAX => {
                    shm.state[..bytes.len()].copy_from_slice(&bytes);
                    shm.state_len.store(bytes.len() as u32, Ordering::Release);
                    true
                }
                _ => false,
            },
            CMD_LOAD => {
                let len = shm.state_len.load(Ordering::Acquire) as usize;
                inst.load_state(&shm.state[..len.min(STATE_MAX)], None)
                    .is_ok()
            }
            CMD_QUIT => {
                sh.resp.store(seen, Ordering::Release);
                to_host.wake(&sh.resp);
                return Ok(());
            }
            _ => false,
        };
        sh.status.store(if ok { 0 } else { 1 }, Ordering::Release);
        sh.resp.store(seen, Ordering::Release);
        to_host.wake(&sh.resp);
    }
}
