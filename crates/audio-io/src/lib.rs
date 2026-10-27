// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio devices (ARCHITECTURE §4.1): listing drivers and devices, and
//! running the engine on an output device.
//!
//! A [`DeviceProfile`] says which driver ("host"), device, sample rate,
//! buffer size and output channels to use. [`start`] opens that device and
//! calls [`Engine::process_split`](daw_engine::engine::Engine::process_split)
//! from its audio callback, converting to whatever sample format and
//! channel layout the device uses. On Windows, ASIO is preferred where a
//! driver is installed.
//!
//! Not here yet: audio input (roadmap 1.2), and reconnecting after the
//! device is unplugged (the stream reports the error and stops).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, FromSample, Host, SampleFormat, SizedSample, StreamConfig};
use daw_engine::engine::Engine;

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
    Some(DeviceProfile {
        host: host.id().name().to_string(),
        device: info.name,
        rate: info.default_rate.unwrap_or(48_000),
        buffer: None,
        main: (0, 1.min(info.output_channels.saturating_sub(1) as usize)),
        click: None,
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
    let device = host
        .output_devices()
        .map_err(|e| e.to_string())?
        .find(|d| device_name(d) == p.device)
        .ok_or(format!("device {:?} not found", p.device))?;
    Ok((host, device))
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
    /// Set if the driver reported an error (for example the device was
    /// unplugged); playback has stopped.
    pub failed: AtomicBool,
}

/// A running device. Dropping it stops the audio.
pub struct Running {
    _stream: cpal::Stream,
    pub status: Arc<StreamStatus>,
    pub channels: u16,
    pub rate: u32,
}

/// Opens the device in `profile` and starts calling `engine` from its
/// audio callback.
pub fn start(profile: &DeviceProfile, engine: Engine) -> Result<Running, String> {
    let (_host, device) = find(profile)?;
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
    let config = StreamConfig {
        channels,
        sample_rate: profile.rate,
        buffer_size: profile
            .buffer
            .map_or(BufferSize::Default, BufferSize::Fixed),
    };
    let status = Arc::new(StreamStatus::default());
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, profile, engine, &status),
        SampleFormat::I16 => build::<i16>(&device, &config, profile, engine, &status),
        SampleFormat::I32 => build::<i32>(&device, &config, profile, engine, &status),
        SampleFormat::U16 => build::<u16>(&device, &config, profile, engine, &status),
        f => Err(format!("unsupported sample format {f:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Running {
        _stream: stream,
        status,
        channels,
        rate: profile.rate,
    })
}

fn build<T>(
    device: &Device,
    config: &StreamConfig,
    profile: &DeviceProfile,
    mut engine: Engine,
    status: &Arc<StreamStatus>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let chans = config.channels as usize;
    let (l_ch, r_ch, click_ch) = (profile.main.0, profile.main.1, profile.click);
    // Planar scratch, allocated here, never in the callback.
    let mut l = vec![0.0f32; MAX_BLOCK];
    let mut r = vec![0.0f32; MAX_BLOCK];
    let mut c = vec![0.0f32; MAX_BLOCK];
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
                let frames = data.len() / chans;
                s.callbacks.fetch_add(1, Ordering::Relaxed);
                s.last_frames.store(frames as u64, Ordering::Relaxed);
                let ts = info.timestamp();
                if let Some(d) = ts.playback.checked_duration_since(ts.callback) {
                    s.output_latency_us
                        .store(d.as_micros() as u64, Ordering::Relaxed);
                }
                for block in data.chunks_mut(MAX_BLOCK * chans) {
                    let n = block.len() / chans;
                    let (bl, br, bc) = (&mut l[..n], &mut r[..n], &mut c[..n]);
                    engine.process_split(bl, br, click_ch.map(|_| &mut *bc));
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
            },
            on_error,
            None,
        )
        .map_err(|e| format!("could not open the output: {e}"))
}
