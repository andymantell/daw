// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 A: full-duplex audio at small buffers, with no dropouts under
//! a synthetic DSP load.
//!
//! Plays input channel 1 (the guitar) straight back out on outputs 1 and 2,
//! so you can hear the monitoring latency, while spending a fixed share of
//! every callback on dummy DSP work. It counts anything that would be a
//! dropout and prints a verdict at the end.
//!
//! Usage:
//!   audio-io --list
//!   audio-io [--host asio|wasapi] [--device NAME] [--rate 48000]
//!            [--buffer 128] [--minutes 30] [--load 0.5]
//!
//! The report is printed and also saved as `audio-spike-report.txt`.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, FromSample, Host, Sample, SampleFormat, SizedSample, StreamConfig};
use spike_engine::rt_check::{self, RtGuard};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

struct Opts {
    list: bool,
    host: Option<String>,
    device: Option<String>,
    rate: u32,
    buffer: u32,
    minutes: f64,
    load: f64,
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts {
        list: false,
        host: None,
        device: None,
        rate: 48_000,
        buffer: 128,
        minutes: 30.0,
        load: 0.5,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--list" => o.list = true,
            "--host" => o.host = Some(val()?),
            "--device" => o.device = Some(val()?),
            "--rate" => o.rate = val()?.parse().map_err(|e| format!("--rate: {e}"))?,
            "--buffer" => o.buffer = val()?.parse().map_err(|e| format!("--buffer: {e}"))?,
            "--minutes" => o.minutes = val()?.parse().map_err(|e| format!("--minutes: {e}"))?,
            "--load" => o.load = val()?.parse().map_err(|e| format!("--load: {e}"))?,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if !(0.0..0.95).contains(&o.load) {
        return Err("--load must be between 0 and 0.95".into());
    }
    Ok(o)
}

fn device_name(d: &Device) -> String {
    d.description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "(unnamed)".into())
}

fn list() {
    for id in cpal::available_hosts() {
        println!("Host: {}", id.name());
        let Ok(host) = cpal::host_from_id(id) else {
            println!("  (could not open)");
            continue;
        };
        let Ok(devices) = host.devices() else {
            continue;
        };
        for d in devices {
            println!("  Device: {}", device_name(&d));
            if let Ok(c) = d.default_input_config() {
                println!(
                    "    input:  {} ch, {} Hz, {:?}, buffer {:?}",
                    c.channels(),
                    c.sample_rate(),
                    c.sample_format(),
                    c.buffer_size()
                );
            }
            if let Ok(c) = d.default_output_config() {
                println!(
                    "    output: {} ch, {} Hz, {:?}, buffer {:?}",
                    c.channels(),
                    c.sample_rate(),
                    c.sample_format(),
                    c.buffer_size()
                );
            }
        }
    }
}

fn pick_host(want: Option<&str>) -> Result<Host, String> {
    let hosts = cpal::available_hosts();
    let id = match want {
        Some(w) => hosts
            .into_iter()
            .find(|h| h.name().eq_ignore_ascii_case(w))
            .ok_or(format!("host {w} not available (try --list)"))?,
        // Prefer ASIO where it exists.
        None => hosts
            .iter()
            .copied()
            .find(|h| h.name().eq_ignore_ascii_case("asio"))
            .unwrap_or_else(|| cpal::default_host().id()),
    };
    cpal::host_from_id(id).map_err(|e| e.to_string())
}

fn pick_device(host: &Host, want: Option<&str>) -> Result<Device, String> {
    match want {
        Some(w) => {
            let w = w.to_lowercase();
            host.devices()
                .map_err(|e| e.to_string())?
                .find(|d| device_name(d).to_lowercase().contains(&w))
                .ok_or(format!("no device matching {w:?} (try --list)"))
        }
        None => host
            .default_output_device()
            .ok_or("no default device".into()),
    }
}

/// Counters shared between the audio callbacks and the reporting thread.
#[derive(Default)]
struct Stats {
    out_callbacks: AtomicU64,
    in_callbacks: AtomicU64,
    /// Callbacks whose own processing took longer than the buffer period.
    overruns: AtomicU64,
    /// Gaps between callbacks of more than 1.5 buffer periods.
    late: AtomicU64,
    /// Output callbacks that found too little input to play.
    underruns: AtomicU64,
    stream_errors: AtomicU64,
    max_callback_ns: AtomicU64,
    max_gap_ns: AtomicU64,
    min_frames: AtomicU64,
    max_frames: AtomicU64,
    out_latency_us: AtomicU64,
    in_latency_us: AtomicU64,
    allocations: AtomicU64,
    /// Set once the warm-up period is over; earlier glitches don't count.
    counting: std::sync::atomic::AtomicBool,
}

impl Stats {
    fn record_frames(&self, frames: u64) {
        self.max_frames.fetch_max(frames, Relaxed);
        self.min_frames.fetch_min(frames, Relaxed);
    }
    fn counting(&self) -> bool {
        self.counting.load(Relaxed)
    }
}

/// Spends `ns` nanoseconds doing arithmetic the optimiser can't remove.
fn burn(ns: u64, seed: f32) -> f32 {
    let start = Instant::now();
    let mut x = seed;
    while (start.elapsed().as_nanos() as u64) < ns {
        for _ in 0..64 {
            x = (x * 1.000_001 + 0.000_1).sin();
        }
    }
    std::hint::black_box(x)
}

fn run<T>(device: &Device, opts: &Opts, in_ch: u16, out_ch: u16) -> Result<String, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
    f32: FromSample<T>,
{
    let period_ns = 1e9 * opts.buffer as f64 / opts.rate as f64;
    let burn_ns = (period_ns * opts.load) as u64;
    let stats = Arc::new(Stats {
        min_frames: AtomicU64::new(u64::MAX),
        ..Default::default()
    });

    // Mono guitar signal from input to output. Pre-filled with two buffers
    // of silence, which covers drivers that run input and output callbacks
    // separately.
    let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(16 * 1024);
    for _ in 0..2 * opts.buffer {
        tx.push(0.0).unwrap();
    }

    let in_cfg = StreamConfig {
        channels: in_ch,
        sample_rate: opts.rate,
        buffer_size: BufferSize::Fixed(opts.buffer),
    };
    let out_cfg = StreamConfig {
        channels: out_ch,
        ..in_cfg
    };

    let s = stats.clone();
    let mut last_in: Option<Instant> = None;
    let input = device
        .build_input_stream(
            in_cfg,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                let _rt = RtGuard::enter();
                let now = Instant::now();
                s.in_callbacks.fetch_add(1, Relaxed);
                let ts = info.timestamp();
                s.in_latency_us.store(
                    ts.callback.duration_since(ts.capture).as_micros() as u64,
                    Relaxed,
                );
                if let Some(prev) = last_in.replace(now)
                    && s.counting()
                    && (now - prev).as_nanos() as f64 > 1.5 * period_ns
                {
                    s.late.fetch_add(1, Relaxed);
                }
                for frame in data.chunks(in_ch as usize) {
                    // If the ring is full the output side has stalled; that
                    // shows up as late or underrun callbacks there.
                    let _ = tx.push(f32::from_sample(frame[0]));
                }
                s.allocations
                    .fetch_max(rt_check::violations() as u64, Relaxed);
            },
            {
                let s = stats.clone();
                move |e| {
                    s.stream_errors.fetch_add(1, Relaxed);
                    eprintln!("input stream error: {e}");
                }
            },
            None,
        )
        .map_err(|e| format!("could not open input: {e}"))?;

    let s = stats.clone();
    let mut last_out: Option<Instant> = None;
    let output = device
        .build_output_stream(
            out_cfg,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let _rt = RtGuard::enter();
                let start = Instant::now();
                s.out_callbacks.fetch_add(1, Relaxed);
                let frames = (data.len() / out_ch as usize) as u64;
                s.record_frames(frames);
                let ts = info.timestamp();
                s.out_latency_us.store(
                    ts.playback.duration_since(ts.callback).as_micros() as u64,
                    Relaxed,
                );
                if let Some(prev) = last_out.replace(start) {
                    let gap = (start - prev).as_nanos() as u64;
                    if s.counting() {
                        s.max_gap_ns.fetch_max(gap, Relaxed);
                        if gap as f64 > 1.5 * period_ns {
                            s.late.fetch_add(1, Relaxed);
                        }
                    }
                }

                let mut short = false;
                let mut seed = 0.0;
                for frame in data.chunks_mut(out_ch as usize) {
                    let v = rx.pop().unwrap_or_else(|_| {
                        short = true;
                        0.0
                    });
                    seed += v;
                    for (i, out) in frame.iter_mut().enumerate() {
                        *out = T::from_sample(if i < 2 { v } else { 0.0 });
                    }
                }
                burn(burn_ns, seed);

                let took = start.elapsed().as_nanos() as u64;
                if s.counting() {
                    if short {
                        s.underruns.fetch_add(1, Relaxed);
                    }
                    s.max_callback_ns.fetch_max(took, Relaxed);
                    if took as f64 > period_ns {
                        s.overruns.fetch_add(1, Relaxed);
                    }
                }
                s.allocations
                    .fetch_max(rt_check::violations() as u64, Relaxed);
            },
            {
                let s = stats.clone();
                move |e| {
                    s.stream_errors.fetch_add(1, Relaxed);
                    eprintln!("output stream error: {e}");
                }
            },
            None,
        )
        .map_err(|e| format!("could not open output: {e}"))?;

    input.play().map_err(|e| e.to_string())?;
    output.play().map_err(|e| e.to_string())?;
    println!("Running. Play your guitar into input 1; you should hear it on outputs 1 and 2.");

    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(2));
    stats.counting.store(true, Relaxed);
    let total = Duration::from_secs_f64(opts.minutes * 60.0);
    let mut next = Duration::from_secs(10);
    while started.elapsed() < total {
        std::thread::sleep(Duration::from_millis(200));
        if started.elapsed() >= next {
            next += Duration::from_secs(10);
            println!(
                "{:>5.0} s  overruns {}  late {}  underruns {}  errors {}  worst callback {:.2} ms",
                started.elapsed().as_secs_f64(),
                stats.overruns.load(Relaxed),
                stats.late.load(Relaxed),
                stats.underruns.load(Relaxed),
                stats.stream_errors.load(Relaxed),
                stats.max_callback_ns.load(Relaxed) as f64 / 1e6,
            );
        }
    }
    drop(output);
    drop(input);

    let st = &stats;
    let g = |a: &AtomicU64| a.load(Relaxed);
    let glitches = g(&st.overruns) + g(&st.late) + g(&st.underruns) + g(&st.stream_errors);
    let mut r = String::new();
    let _ = writeln!(
        r,
        "\nResult after {:.1} minutes",
        started.elapsed().as_secs_f64() / 60.0
    );
    let _ = writeln!(
        r,
        "  requested: {} Hz, {}-frame buffer, DSP load {:.0}%",
        opts.rate,
        opts.buffer,
        opts.load * 100.0
    );
    let _ = writeln!(
        r,
        "  frames per callback seen: {}..{}",
        g(&st.min_frames).min(g(&st.max_frames)),
        g(&st.max_frames)
    );
    let _ = writeln!(
        r,
        "  callbacks: {} output, {} input",
        g(&st.out_callbacks),
        g(&st.in_callbacks)
    );
    let _ = writeln!(
        r,
        "  overruns (callback slower than real time): {}",
        g(&st.overruns)
    );
    let _ = writeln!(r, "  late callbacks (gap > 1.5 buffers): {}", g(&st.late));
    let _ = writeln!(
        r,
        "  underruns (output ran out of input): {}",
        g(&st.underruns)
    );
    let _ = writeln!(r, "  stream errors: {}", g(&st.stream_errors));
    let _ = writeln!(
        r,
        "  worst callback: {:.3} ms of {:.3} ms budget",
        g(&st.max_callback_ns) as f64 / 1e6,
        period_ns / 1e6
    );
    let _ = writeln!(
        r,
        "  worst gap between callbacks: {:.3} ms",
        g(&st.max_gap_ns) as f64 / 1e6
    );
    let _ = writeln!(
        r,
        "  driver-reported latency: input {} µs, output {} µs",
        g(&st.in_latency_us),
        g(&st.out_latency_us)
    );
    let _ = writeln!(
        r,
        "  allocations on audio threads (incl. driver glue): {}",
        g(&st.allocations)
    );
    let _ = writeln!(
        r,
        "  verdict: {}",
        if glitches == 0 {
            "PASS (no dropouts detected)"
        } else {
            "FAIL (see counts above)"
        }
    );
    let _ = writeln!(
        r,
        "  note: driver-reported latency is not the true round-trip; that is measured with a loopback cable (spike 0.4)."
    );
    Ok(r)
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprintln!(
                "usage: audio-io --list\n       audio-io [--host asio|wasapi] [--device NAME] [--rate 48000] [--buffer 128] [--minutes 30] [--load 0.5]"
            );
            std::process::exit(2);
        }
    };
    if opts.list {
        list();
        return;
    }

    let result = (|| -> Result<String, String> {
        let host = pick_host(opts.host.as_deref())?;
        let device = pick_device(&host, opts.device.as_deref())?;
        let in_cfg = device
            .default_input_config()
            .map_err(|e| format!("input: {e}"))?;
        let out_cfg = device
            .default_output_config()
            .map_err(|e| format!("output: {e}"))?;
        let mut head = String::new();
        let _ = writeln!(
            head,
            "Audio I/O spike ({} {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let _ = writeln!(head, "  host: {}", host.id().name());
        let _ = writeln!(head, "  device: {}", device_name(&device));
        let _ = writeln!(
            head,
            "  input: {} ch {:?}; output: {} ch {:?}",
            in_cfg.channels(),
            in_cfg.sample_format(),
            out_cfg.channels(),
            out_cfg.sample_format()
        );
        print!("{head}");
        let body = match out_cfg.sample_format() {
            SampleFormat::F32 => run::<f32>(&device, &opts, in_cfg.channels(), out_cfg.channels()),
            SampleFormat::I32 => run::<i32>(&device, &opts, in_cfg.channels(), out_cfg.channels()),
            SampleFormat::I16 => run::<i16>(&device, &opts, in_cfg.channels(), out_cfg.channels()),
            f => Err(format!("sample format {f:?} not handled by this spike")),
        }?;
        Ok(head + &body)
    })();

    let report = match result {
        Ok(r) => r,
        Err(e) => format!("Audio I/O spike failed: {e}\n"),
    };
    print!("{report}");
    if std::fs::write("audio-spike-report.txt", &report).is_ok() {
        println!("Saved to audio-spike-report.txt");
    }
    if cfg!(windows) {
        println!("Press Enter to close.");
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
}
