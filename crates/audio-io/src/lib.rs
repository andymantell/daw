// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio devices (ARCHITECTURE §4.1): listing drivers and devices, and
//! running the engine on them, with input for monitoring and recording.
//!
//! A [`DeviceProfile`] says which driver ("host"), device, sample rate,
//! buffer size and output channels to use. [`start`] opens that device and
//! calls [`Engine::process_split`](daw_engine::engine::Engine::process_split)
//! from its audio callback, converting to whatever sample format and
//! channel layout the device uses. On Windows, ASIO is preferred where a
//! driver is installed.
//!
//! Input arrives in its own callback. It is passed to the output callback
//! through a lock-free queue holding a little slack (two buffers of
//! silence to start with), since some drivers run the two callbacks on
//! separate threads; with ASIO both come from the same driver callback.
//! If the device is unplugged
//! the stream reports it in [`StreamStatus::failed`] and stops; the app
//! watches for the device with [`is_present`] and starts again.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, FromSample, Host, SampleFormat, SizedSample, StreamConfig};
use daw_engine::engine::Engine;
use rtrb::{Consumer, Producer, RingBuffer};

/// The largest block the engine is asked for in one go. Device buffers
/// bigger than this are processed in several calls.
pub const MAX_BLOCK: usize = 4096;

/// Which device to use and how.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceProfile {
    /// Driver type, e.g. "ASIO", "WASAPI", "ALSA", "CoreAudio".
    pub host: String,
    pub device: String,
    pub rate: u32,
    /// Frames per buffer. `None` lets the driver choose.
    pub buffer: Option<u32>,
    /// Device channels (from 0) for the main left and right outputs.
    pub main: (usize, usize),
    /// Device channel for the separate metronome output, if any.
    pub click: Option<usize>,
    /// Device to record from: the same as `device` for an audio
    /// interface; `None` for no input.
    pub input: Option<String>,
    /// How much later recorded audio arrives than it was played along to,
    /// in frames: the interface's round trip, measured with a loopback
    /// cable. `None` uses the latency the driver reports instead.
    pub record_offset: Option<u32>,
    /// The player listens through the interface's own direct monitoring,
    /// so the app never plays inputs back (no doubled sound).
    pub direct_monitoring: bool,
}

/// One device as the driver describes it.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub output_channels: u16,
    pub input_channels: u16,
    pub default_rate: Option<u32>,
    /// Sample rates the device says it supports, from a fixed list of
    /// common ones.
    pub rates: Vec<u32>,
    /// Smallest and largest buffer the driver accepts, if it says.
    pub buffer_range: Option<(u32, u32)>,
}

#[derive(Clone, Debug)]
pub struct HostInfo {
    pub name: String,
    pub devices: Vec<DeviceInfo>,
}

const COMMON_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

pub fn device_name(d: &Device) -> String {
    d.description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "(unnamed)".into())
}

fn describe(d: &Device) -> DeviceInfo {
    let out = d.default_output_config().ok();
    let inp = d.default_input_config().ok();
    let mut rates = Vec::new();
    let mut buffer_range = None;
    if let Ok(cfgs) = d.supported_output_configs() {
        for c in cfgs {
            for r in COMMON_RATES {
                if (c.min_sample_rate()..=c.max_sample_rate()).contains(&r) && !rates.contains(&r) {
                    rates.push(r);
                }
            }
            if let cpal::SupportedBufferSize::Range { min, max } = c.buffer_size() {
                buffer_range = Some((*min, *max));
            }
        }
    }
    rates.sort();
    DeviceInfo {
        name: device_name(d),
        output_channels: out.as_ref().map_or(0, |c| c.channels()),
        input_channels: inp.as_ref().map_or(0, |c| c.channels()),
        default_rate: out.as_ref().map(|c| c.sample_rate()),
        rates,
        buffer_range,
    }
}

/// Every driver and its devices that can play audio.
pub fn list() -> Vec<HostInfo> {
    cpal::available_hosts()
        .into_iter()
        .filter_map(|id| {
            let host = cpal::host_from_id(id).ok()?;
            let devices = host
                .output_devices()
                .map(|ds| ds.map(|d| describe(&d)).collect())
                .unwrap_or_default();
            Some(HostInfo {
                name: id.name().to_string(),
                devices,
            })
        })
        .collect()
}

/// A sensible first profile: ASIO if installed, otherwise the system's
/// default driver, its default output device, at that device's rate.
pub fn default_profile() -> Option<DeviceProfile> {
    let host = preferred_host();
    let d = host.default_output_device()?;
    let info = describe(&d);
    // An interface records from itself; otherwise the default input.
    let input = if info.input_channels > 0 {
        Some(info.name.clone())
    } else {
        host.default_input_device().map(|d| device_name(&d))
    };
    Some(DeviceProfile {
        host: host.id().name().to_string(),
        device: info.name,
        rate: info.default_rate.unwrap_or(48_000),
        buffer: None,
        main: (0, 1.min(info.output_channels.saturating_sub(1) as usize)),
        click: None,
        input,
        record_offset: None,
        direct_monitoring: false,
    })
}

fn preferred_host() -> Host {
    let asio = cpal::available_hosts()
        .into_iter()
        .find(|h| h.name().eq_ignore_ascii_case("asio"));
    asio.and_then(|id| cpal::host_from_id(id).ok())
        .unwrap_or_else(cpal::default_host)
}

fn find(p: &DeviceProfile) -> Result<(Host, Device), String> {
    let id = cpal::available_hosts()
        .into_iter()
        .find(|h| h.name().eq_ignore_ascii_case(&p.host))
        .ok_or(format!("the {} driver is not available", p.host))?;
    let host = cpal::host_from_id(id).map_err(|e| e.to_string())?;
    // The default device is not always in the list (ALSA's "default").
    let device = host
        .output_devices()
        .map_err(|e| e.to_string())?
        .find(|d| device_name(d) == p.device)
        .or_else(|| {
            host.default_output_device()
                .filter(|d| device_name(d) == p.device)
        })
        .ok_or(format!("device {:?} not found", p.device))?;
    Ok((host, device))
}

fn find_input(host: &Host, name: &str) -> Option<Device> {
    host.input_devices()
        .ok()?
        .find(|d| device_name(d) == name)
        .or_else(|| {
            host.default_input_device()
                .filter(|d| device_name(d) == name)
        })
}

/// Devices on `host` that can record, for choosing the input.
pub fn input_devices(host: &str) -> Vec<DeviceInfo> {
    let Some(id) = cpal::available_hosts()
        .into_iter()
        .find(|h| h.name().eq_ignore_ascii_case(host))
    else {
        return Vec::new();
    };
    let Ok(host) = cpal::host_from_id(id) else {
        return Vec::new();
    };
    host.input_devices()
        .map(|ds| ds.map(|d| describe(&d)).collect())
        .unwrap_or_default()
}

/// Whether the device in `profile` can be found now, for example after it
/// was unplugged.
pub fn is_present(profile: &DeviceProfile) -> bool {
    find(profile).is_ok()
}

/// Shared with the UI while a device runs.
#[derive(Default)]
pub struct StreamStatus {
    /// Callbacks so far.
    pub callbacks: AtomicU64,
    /// Frames in the most recent callback.
    pub last_frames: AtomicU64,
    /// Output latency the driver reported on the last callback, in µs.
    pub output_latency_us: AtomicU64,
    /// Input latency the driver reported on the last input callback, µs.
    pub input_latency_us: AtomicU64,
    /// Output callbacks that found less input waiting than they needed
    /// (the gap is filled with silence).
    pub input_short: AtomicU64,
    /// Set if the driver reported an error (for example the device was
    /// unplugged); playback has stopped.
    pub failed: AtomicBool,
    /// Time spent in the callback, and the length of audio it made, both
    /// in ns: their ratio is the DSP load.
    pub busy_ns: AtomicU64,
    pub audio_ns: AtomicU64,
    /// The heaviest callback since last taken, as a fraction of its
    /// buffer's length, in thousandths.
    pub peak_permille: AtomicU64,
    /// Callbacks that took longer than their buffer lasts: each is a
    /// certain dropout.
    pub overloads: AtomicU64,
}

impl StreamStatus {
    /// Records one callback that took `busy` to make `frames` frames.
    fn note_busy(&self, busy: std::time::Duration, frames: usize, rate: u32) {
        let busy = busy.as_nanos() as u64;
        let budget = (frames as u64 * 1_000_000_000 / rate.max(1) as u64).max(1);
        self.busy_ns.fetch_add(busy, Ordering::Relaxed);
        self.audio_ns.fetch_add(budget, Ordering::Relaxed);
        self.peak_permille
            .fetch_max(busy * 1000 / budget, Ordering::Relaxed);
        if busy > budget {
            self.overloads.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The heaviest callback since the last call, as a fraction of its
    /// buffer's length (1.0 is a dropout).
    pub fn take_peak_load(&self) -> f32 {
        self.peak_permille.swap(0, Ordering::Relaxed) as f32 / 1000.0
    }
}

/// A running device. Dropping it stops the audio.
pub struct Running {
    _backend: Backend,
    pub status: Arc<StreamStatus>,
    pub channels: u16,
    /// Input channels being passed to the engine; 0 for none.
    pub input_channels: u16,
    /// Why there is no input, if one was asked for and failed.
    pub input_error: Option<String>,
    pub rate: u32,
}

impl Running {
    /// The recording offset to use: the profile's measured one, or else
    /// the input and output latency the driver reports.
    pub fn record_offset(&self, profile: &DeviceProfile) -> u32 {
        profile.record_offset.unwrap_or_else(|| {
            let us = self.status.input_latency_us.load(Ordering::Relaxed)
                + self.status.output_latency_us.load(Ordering::Relaxed);
            (us * self.rate as u64 / 1_000_000) as u32
        })
    }
}

/// Opens the device in `profile` and starts calling `engine` from its
/// audio callback.
// The fields are held only to keep the audio running until dropped.
#[allow(dead_code)]
enum Backend {
    Device(Box<(cpal::Stream, Option<cpal::Stream>)>),
    Silent(SilentThread),
}

struct SilentThread {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SilentThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Runs the engine in real time with no device, discarding the sound:
/// for machines without audio (such as build servers) and for trying the
/// app out. Playback, meters and the playhead all behave as normal.
///
/// With the environment variable `DAW_TEST_INPUT` set, it also feeds two
/// input channels of test tone (220 Hz left, 330 Hz right), so recording
/// can be tried without an interface.
pub fn start_silent(rate: u32, buffer: u32, mut engine: Engine) -> Running {
    let test_input = std::env::var_os("DAW_TEST_INPUT").is_some();
    let in_ch: usize = if test_input { 2 } else { 0 };
    let status = Arc::new(StreamStatus::default());
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st) = (status.clone(), stop.clone());
    let n = (buffer as usize).clamp(1, MAX_BLOCK);
    let handle = std::thread::Builder::new()
        .name("silent audio".into())
        .spawn(move || {
            let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
            let mut inp = vec![0.0f32; n * in_ch];
            let mut t = 0u64;
            let period = std::time::Duration::from_secs_f64(n as f64 / rate as f64);
            let mut next = std::time::Instant::now();
            while !st.load(Ordering::Relaxed) {
                let t0 = std::time::Instant::now();
                for frame in inp.chunks_mut(2) {
                    let x = t as f32 / rate as f32 * std::f32::consts::TAU;
                    frame[0] = (x * 220.0).sin() * 0.3;
                    frame[1] = (x * 330.0).sin() * 0.3;
                    t += 1;
                }
                engine.process_duplex(&inp, in_ch, &mut l, &mut r, None);
                s.note_busy(t0.elapsed(), n, rate);
                s.callbacks.fetch_add(1, Ordering::Relaxed);
                s.last_frames.store(n as u64, Ordering::Relaxed);
                next += period;
                if let Some(wait) = next.checked_duration_since(std::time::Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
        })
        .expect("could not start the silent audio thread");
    Running {
        _backend: Backend::Silent(SilentThread {
            stop,
            handle: Some(handle),
        }),
        status,
        channels: 2,
        input_channels: in_ch as u16,
        input_error: None,
        rate,
    }
}

pub fn start(profile: &DeviceProfile, engine: Engine) -> Result<Running, String> {
    let (host, device) = find(profile)?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("could not read the device's settings: {e}"))?;
    let channels = supported.channels();
    for ch in [profile.main.0, profile.main.1]
        .into_iter()
        .chain(profile.click)
    {
        if ch >= channels as usize {
            return Err(format!("the device has no output {}", ch + 1));
        }
    }
    let buffer_size = profile
        .buffer
        .map_or(BufferSize::Default, BufferSize::Fixed);
    let config = StreamConfig {
        channels,
        sample_rate: profile.rate,
        buffer_size,
    };
    let status = Arc::new(StreamStatus::default());

    // Input first, so the output callback has its queue from the start.
    let slack = 2 * profile.buffer.unwrap_or(1024) as usize;
    let (input, input_error) = match &profile.input {
        None => (None, None),
        Some(name) => match open_input(&host, name, profile.rate, buffer_size, slack, &status) {
            Ok(i) => (Some(i), None),
            Err(e) => (None, Some(e)),
        },
    };
    let (in_stream, in_rx, input_channels) = match input {
        Some((s, rx, ch)) => (Some(s), Some(rx), ch),
        None => (None, None, 0),
    };
    let out = Out {
        profile,
        input: in_rx,
        input_channels: input_channels as usize,
        slack,
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, out, engine, &status),
        SampleFormat::I16 => build::<i16>(&device, &config, out, engine, &status),
        SampleFormat::I32 => build::<i32>(&device, &config, out, engine, &status),
        SampleFormat::U16 => build::<u16>(&device, &config, out, engine, &status),
        f => Err(format!("unsupported sample format {f:?}")),
    }?;
    if let Some(i) = &in_stream {
        i.play().map_err(|e| e.to_string())?;
    }
    stream.play().map_err(|e| e.to_string())?;
    Ok(Running {
        _backend: Backend::Device(Box::new((stream, in_stream))),
        status,
        channels,
        input_channels,
        input_error,
        rate: profile.rate,
    })
}

/// The input side: a stream that pushes interleaved `f32` samples into a
/// queue, the queue's other end, and the channel count.
type InputSide = (cpal::Stream, Consumer<f32>, u16);

fn open_input(
    host: &Host,
    name: &str,
    rate: u32,
    buffer_size: BufferSize,
    slack: usize,
    status: &Arc<StreamStatus>,
) -> Result<InputSide, String> {
    let device = find_input(host, name).ok_or(format!("input device {name:?} not found"))?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("could not read the input's settings: {e}"))?;
    let channels = supported.channels();
    let config = StreamConfig {
        channels,
        sample_rate: rate,
        buffer_size,
    };
    // Room for plenty of slack; a full queue drops input.
    let (mut tx, rx) = RingBuffer::<f32>::new((slack * 8).max(MAX_BLOCK * 4) * channels as usize);
    for _ in 0..slack * channels as usize {
        let _ = tx.push(0.0);
    }
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&device, &config, tx, status),
        SampleFormat::I16 => build_input::<i16>(&device, &config, tx, status),
        SampleFormat::I32 => build_input::<i32>(&device, &config, tx, status),
        SampleFormat::U16 => build_input::<u16>(&device, &config, tx, status),
        f => Err(format!("unsupported input sample format {f:?}")),
    }?;
    Ok((stream, rx, channels))
}

fn build_input<T>(
    device: &Device,
    config: &StreamConfig,
    mut tx: Producer<f32>,
    status: &Arc<StreamStatus>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let s = status.clone();
    let on_error = {
        let s = status.clone();
        move |e: cpal::Error| {
            s.failed.store(true, Ordering::Relaxed);
            eprintln!("audio input error: {e}");
        }
    };
    device
        .build_input_stream(
            *config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                let ts = info.timestamp();
                if let Some(d) = ts.callback.checked_duration_since(ts.capture) {
                    s.input_latency_us
                        .store(d.as_micros() as u64, Ordering::Relaxed);
                }
                // A full queue means the output side has stalled; the
                // input is dropped rather than blocking.
                for x in data {
                    if tx.push(x.to_sample::<f32>()).is_err() {
                        break;
                    }
                }
            },
            on_error,
            None,
        )
        .map_err(|e| format!("could not open the input: {e}"))
}

/// What the output callback needs besides the engine.
struct Out<'a> {
    profile: &'a DeviceProfile,
    input: Option<Consumer<f32>>,
    input_channels: usize,
    /// Input frames to keep queued; more than a few times this is
    /// dropped, so monitoring latency cannot creep up.
    slack: usize,
}

fn build<T>(
    device: &Device,
    config: &StreamConfig,
    out: Out,
    mut engine: Engine,
    status: &Arc<StreamStatus>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let chans = config.channels as usize;
    let rate = config.sample_rate;
    let (l_ch, r_ch, click_ch) = (out.profile.main.0, out.profile.main.1, out.profile.click);
    let (mut input, in_ch, slack) = (out.input, out.input_channels, out.slack);
    // Scratch, allocated here, never in the callback.
    let mut l = vec![0.0f32; MAX_BLOCK];
    let mut r = vec![0.0f32; MAX_BLOCK];
    let mut c = vec![0.0f32; MAX_BLOCK];
    let mut inp = vec![0.0f32; MAX_BLOCK * in_ch];
    let s = status.clone();
    let on_error = {
        let s = status.clone();
        move |e: cpal::Error| {
            s.failed.store(true, Ordering::Relaxed);
            eprintln!("audio device error: {e}");
        }
    };
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let t0 = std::time::Instant::now();
                let frames = data.len() / chans;
                s.callbacks.fetch_add(1, Ordering::Relaxed);
                s.last_frames.store(frames as u64, Ordering::Relaxed);
                let ts = info.timestamp();
                if let Some(d) = ts.playback.checked_duration_since(ts.callback) {
                    s.output_latency_us
                        .store(d.as_micros() as u64, Ordering::Relaxed);
                }
                if let Some(q) = &mut input {
                    // Too much queued: drop the oldest down to the slack.
                    let queued = q.slots() / in_ch;
                    if queued > frames + 4 * slack
                        && let Ok(chunk) = q.read_chunk((queued - frames - slack) * in_ch)
                    {
                        chunk.commit_all();
                    }
                }
                for block in data.chunks_mut(MAX_BLOCK * chans) {
                    let n = block.len() / chans;
                    let (bl, br, bc) = (&mut l[..n], &mut r[..n], &mut c[..n]);
                    let bi = &mut inp[..n * in_ch];
                    if let Some(q) = &mut input {
                        let got = q.slots().min(bi.len());
                        if let Ok(chunk) = q.read_chunk(got) {
                            let (a, b) = chunk.as_slices();
                            bi[..a.len()].copy_from_slice(a);
                            bi[a.len()..got].copy_from_slice(b);
                            chunk.commit_all();
                        }
                        if got < bi.len() {
                            bi[got..].fill(0.0);
                            s.input_short.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    engine.process_duplex(bi, in_ch, bl, br, click_ch.map(|_| &mut *bc));
                    for (i, frame) in block.chunks_mut(chans).enumerate() {
                        for (ch, out) in frame.iter_mut().enumerate() {
                            let mut v = 0.0;
                            if ch == l_ch {
                                v += bl[i];
                            }
                            if ch == r_ch {
                                v += br[i];
                            }
                            if Some(ch) == click_ch {
                                v += bc[i];
                            }
                            *out = T::from_sample(v);
                        }
                    }
                }
                s.note_busy(t0.elapsed(), frames, rate);
            },
            on_error,
            None,
        )
        .map_err(|e| format!("could not open the output: {e}"))
}

/// Device profiles saved between runs: the one in use, and every one
/// used before, so each device keeps its own settings (its recording
/// offset in particular). Stored as plain `key = value` text, one
/// `[profile]` section each, the first being the one in use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedProfiles {
    pub profiles: Vec<DeviceProfile>,
}

impl SavedProfiles {
    /// The profile in use, if any.
    pub fn current(&self) -> Option<&DeviceProfile> {
        self.profiles.first()
    }

    /// The saved profile for this driver and device, if it was used before.
    pub fn for_device(&self, host: &str, device: &str) -> Option<&DeviceProfile> {
        self.profiles
            .iter()
            .find(|p| p.host == host && p.device == device)
    }

    /// Makes `p` the profile in use, replacing any saved for its device.
    pub fn use_profile(&mut self, p: DeviceProfile) {
        self.profiles
            .retain(|q| !(q.host == p.host && q.device == p.device));
        self.profiles.insert(0, p);
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for p in &self.profiles {
            s += "[profile]\n";
            s += &format!("host = {}\n", p.host);
            s += &format!("device = {}\n", p.device);
            s += &format!("rate = {}\n", p.rate);
            if let Some(b) = p.buffer {
                s += &format!("buffer = {b}\n");
            }
            s += &format!("main = {} {}\n", p.main.0, p.main.1);
            if let Some(c) = p.click {
                s += &format!("click = {c}\n");
            }
            if let Some(i) = &p.input {
                s += &format!("input = {i}\n");
            }
            if let Some(o) = p.record_offset {
                s += &format!("record_offset = {o}\n");
            }
            if p.direct_monitoring {
                s += "direct_monitoring = yes\n";
            }
            s += "\n";
        }
        s
    }

    /// Reads what [`to_text`](Self::to_text) wrote. Unknown keys and
    /// unreadable values are skipped, so an older or damaged file loses
    /// only what it cannot say.
    pub fn from_text(text: &str) -> SavedProfiles {
        let mut profiles = Vec::new();
        let mut cur: Option<DeviceProfile> = None;
        for line in text.lines().map(str::trim) {
            if line == "[profile]" {
                profiles.extend(cur.take());
                cur = Some(DeviceProfile {
                    host: String::new(),
                    device: String::new(),
                    rate: 48_000,
                    buffer: None,
                    main: (0, 1),
                    click: None,
                    input: None,
                    record_offset: None,
                    direct_monitoring: false,
                });
                continue;
            }
            let (Some(p), Some((k, v))) = (cur.as_mut(), line.split_once('=')) else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "host" => p.host = v.into(),
                "device" => p.device = v.into(),
                "rate" => p.rate = v.parse().unwrap_or(p.rate),
                "buffer" => p.buffer = v.parse().ok(),
                "main" => {
                    let mut it = v.split_whitespace().filter_map(|x| x.parse().ok());
                    if let (Some(l), Some(r)) = (it.next(), it.next()) {
                        p.main = (l, r);
                    }
                }
                "click" => p.click = v.parse().ok(),
                "input" => p.input = Some(v.into()),
                "record_offset" => p.record_offset = v.parse().ok(),
                "direct_monitoring" => p.direct_monitoring = v == "yes",
                _ => {}
            }
        }
        profiles.extend(cur);
        profiles.retain(|p| !p.host.is_empty() && !p.device.is_empty());
        SavedProfiles { profiles }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(device: &str) -> DeviceProfile {
        DeviceProfile {
            host: "ASIO".into(),
            device: device.into(),
            rate: 48_000,
            buffer: Some(128),
            main: (0, 1),
            click: Some(3),
            input: Some(device.into()),
            record_offset: Some(271),
            direct_monitoring: true,
        }
    }

    #[test]
    fn profiles_survive_a_round_trip() {
        let mut s = SavedProfiles::default();
        s.use_profile(profile("UMC1820"));
        let mut other = profile("Built-in = Speakers");
        other.buffer = None;
        other.click = None;
        other.input = None;
        other.record_offset = None;
        other.direct_monitoring = false;
        s.use_profile(other.clone());
        let back = SavedProfiles::from_text(&s.to_text());
        assert_eq!(back, s);
        assert_eq!(back.current(), Some(&other));
        assert_eq!(
            back.for_device("ASIO", "UMC1820").unwrap().record_offset,
            Some(271)
        );
    }

    #[test]
    fn using_a_device_again_replaces_its_old_profile() {
        let mut s = SavedProfiles::default();
        s.use_profile(profile("A"));
        s.use_profile(profile("B"));
        let mut a = profile("A");
        a.rate = 96_000;
        s.use_profile(a.clone());
        assert_eq!(s.profiles.len(), 2);
        assert_eq!(s.current(), Some(&a));
    }

    #[test]
    fn damaged_files_keep_what_they_can() {
        let s = SavedProfiles::from_text(
            "junk\n[profile]\nhost = WASAPI\ndevice = Speakers\nrate = lots\nfuture = 1\n[profile]\nrate = 44100\n",
        );
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.profiles[0].rate, 48_000);
        assert_eq!(s.profiles[0].device, "Speakers");
    }
}
