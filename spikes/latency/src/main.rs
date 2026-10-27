// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.4: true round-trip latency with a loopback cable, compared with
//! what the driver reports.
//!
//! Connect an output of the interface straight to an input with a cable
//! (output 1 to input 1 by default). The tool plays short test pulses,
//! records them coming back, and measures each one's arrival to a fraction
//! of a sample. It restarts the streams several times per buffer size,
//! because a recording offset is only usable if it is the same every time
//! the audio starts.
//!
//! Two figures per pulse:
//! - **Round trip (frames):** input frame where the pulse arrived, minus
//!   the output frame where it was sent, counting frames from stream start.
//! - **Timestamp error:** the arrival time by the driver's input
//!   timestamps, minus the send time by its output timestamps. Zero means
//!   the driver's reported latencies are honest and a DAW can place
//!   recordings from them alone. Anything else is the recording offset a
//!   DAW must add.
//!
//! Usage:
//!   latency --list
//!   latency [--host asio|wasapi] [--device NAME] [--rate 48000]
//!           [--buffers 64,128,256] [--repeats 5] [--out 1] [--in 1]
//!
//! The report is printed and saved as `latency-report.txt`.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, FromSample, Host, Sample, SampleFormat, SizedSample, StreamConfig};
use latency::{find, median, probe};

const PULSES: usize = 6;
const MIN_SNR: f32 = 8.0;

struct Opts {
    list: bool,
    host: Option<String>,
    device: Option<String>,
    rate: u32,
    buffers: Vec<u32>,
    repeats: usize,
    out_ch: usize,
    in_ch: usize,
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts {
        list: false,
        host: None,
        device: None,
        rate: 48_000,
        buffers: vec![64, 128, 256],
        repeats: 5,
        out_ch: 1,
        in_ch: 1,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or(format!("{a} needs a value"));
        let num = |v: String, n: &str| v.parse::<usize>().map_err(|e| format!("{n}: {e}"));
        match a.as_str() {
            "--list" => o.list = true,
            "--host" => o.host = Some(val()?),
            "--device" => o.device = Some(val()?),
            "--rate" => o.rate = num(val()?, "--rate")? as u32,
            "--buffers" => {
                o.buffers = val()?
                    .split(',')
                    .map(|b| b.trim().parse().map_err(|e| format!("--buffers: {e}")))
                    .collect::<Result<_, _>>()?
            }
            "--repeats" => o.repeats = num(val()?, "--repeats")?.max(1),
            "--out" => o.out_ch = num(val()?, "--out")?.max(1),
            "--in" => o.in_ch = num(val()?, "--in")?.max(1),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option {other}")),
        }
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
            continue;
        };
        let Ok(devices) = host.devices() else {
            continue;
        };
        for d in devices {
            let ins = d.default_input_config().map(|c| c.channels()).unwrap_or(0);
            let outs = d.default_output_config().map(|c| c.channels()).unwrap_or(0);
            println!("  Device: {} ({ins} in, {outs} out)", device_name(&d));
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

/// One callback's frame range and the driver's timestamps, in nanoseconds
/// on the stream clock.
#[derive(Clone, Copy, Default)]
struct Cb {
    start: u64,
    frames: u64,
    /// Output: when the first frame will be heard. Input: when it was captured.
    edge_ns: i128,
    callback_ns: i128,
}

fn find_cb(cbs: &[Cb], frame: f64) -> Option<&Cb> {
    cbs.iter()
        .find(|c| frame >= c.start as f64 && frame < (c.start + c.frames) as f64)
}

struct Pulse {
    round_trip: f64,
    ts_error: Option<f64>,
    reported: Option<f64>,
    snr: f32,
    inverted: bool,
}

fn pulse_start(n: usize, rate: u32) -> u64 {
    (rate as u64 / 2) + n as u64 * rate as u64
}

/// Opens the streams once, plays the pulses and measures them.
fn one_run<T>(
    device: &Device,
    o: &Opts,
    buffer: u32,
    in_chs: u16,
    out_chs: u16,
) -> Result<Vec<Option<Pulse>>, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
    f32: FromSample<T>,
{
    let rate = o.rate;
    let p = Arc::new(probe(rate));
    let total = pulse_start(PULSES, rate) as usize;
    let (mut s_tx, mut s_rx) = rtrb::RingBuffer::<f32>::new(total + 4 * rate as usize);
    let (mut oc_tx, mut oc_rx) = rtrb::RingBuffer::<Cb>::new(1 << 15);
    let (mut ic_tx, mut ic_rx) = rtrb::RingBuffer::<Cb>::new(1 << 15);
    let errors = Arc::new(AtomicU64::new(0));

    let cfg = |ch| StreamConfig {
        channels: ch,
        sample_rate: rate,
        buffer_size: BufferSize::Fixed(buffer),
    };
    let (in_ch, out_ch) = (o.in_ch - 1, o.out_ch - 1);
    if in_ch >= in_chs as usize || out_ch >= out_chs as usize {
        return Err(format!("device has {in_chs} inputs and {out_chs} outputs"));
    }

    let mut in_idx = 0u64;
    let e = errors.clone();
    let input = device
        .build_input_stream(
            cfg(in_chs),
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                let ts = info.timestamp();
                let frames = (data.len() / in_chs as usize) as u64;
                let _ = ic_tx.push(Cb {
                    start: in_idx,
                    frames,
                    edge_ns: ts.capture.as_nanos() as i128,
                    callback_ns: ts.callback.as_nanos() as i128,
                });
                for f in data.chunks(in_chs as usize) {
                    let _ = s_tx.push(f32::from_sample(f[in_ch]));
                }
                in_idx += frames;
            },
            move |err| {
                e.fetch_add(1, Relaxed);
                eprintln!("input stream error: {err}");
            },
            None,
        )
        .map_err(|e| format!("could not open input: {e}"))?;

    let mut out_idx = 0u64;
    let pp = p.clone();
    let e = errors.clone();
    let output = device
        .build_output_stream(
            cfg(out_chs),
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let ts = info.timestamp();
                let frames = (data.len() / out_chs as usize) as u64;
                let _ = oc_tx.push(Cb {
                    start: out_idx,
                    frames,
                    edge_ns: ts.playback.as_nanos() as i128,
                    callback_ns: ts.callback.as_nanos() as i128,
                });
                for (i, f) in data.chunks_mut(out_chs as usize).enumerate() {
                    let k = out_idx + i as u64;
                    let mut v = 0.0;
                    for n in 0..PULSES {
                        let s = pulse_start(n, rate);
                        if k >= s && k < s + pp.len() as u64 {
                            v = pp[(k - s) as usize];
                        }
                    }
                    for (c, x) in f.iter_mut().enumerate() {
                        *x = T::from_sample(if c == out_ch { v } else { 0.0 });
                    }
                }
                out_idx += frames;
            },
            move |err| {
                e.fetch_add(1, Relaxed);
                eprintln!("output stream error: {err}");
            },
            None,
        )
        .map_err(|e| format!("could not open output: {e}"))?;

    input.play().map_err(|e| e.to_string())?;
    output.play().map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_secs_f64(total as f64 / rate as f64 + 1.0));
    drop(output);
    drop(input);
    if errors.load(Relaxed) > 0 {
        return Err(format!("{} stream errors", errors.load(Relaxed)));
    }

    let mut rec = Vec::with_capacity(s_rx.slots());
    while let Ok(v) = s_rx.pop() {
        rec.push(v);
    }
    let mut ocb = Vec::new();
    while let Ok(c) = oc_rx.pop() {
        ocb.push(c);
    }
    let mut icb = Vec::new();
    while let Ok(c) = ic_rx.pop() {
        icb.push(c);
    }

    let ns = 1e9 / rate as f64;
    Ok((0..PULSES)
        .map(|n| {
            let k = pulse_start(n, rate) as f64;
            let from = (k - 0.3 * rate as f64).max(0.0) as usize;
            let to = (k + 0.6 * rate as f64) as usize;
            let f = find(&rec, &p, from, to).filter(|f| f.snr >= MIN_SNR)?;
            let oc = find_cb(&ocb, k);
            let ic = find_cb(&icb, f.at);
            let (ts_error, reported) = match (oc, ic) {
                (Some(oc), Some(ic)) => {
                    let t_out = oc.edge_ns as f64 + (k - oc.start as f64) * ns;
                    let t_in = ic.edge_ns as f64 + (f.at - ic.start as f64) * ns;
                    let rep = (oc.edge_ns - oc.callback_ns + ic.callback_ns - ic.edge_ns) as f64;
                    (Some((t_in - t_out) / ns), Some(rep / ns))
                }
                _ => (None, None),
            };
            Some(Pulse {
                round_trip: f.at - k,
                ts_error,
                reported,
                snr: f.snr,
                inverted: f.inverted,
            })
        })
        .collect())
}

fn spread(v: &[f64]) -> f64 {
    let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    hi - lo
}

fn measure<T>(device: &Device, o: &Opts, in_chs: u16, out_chs: u16) -> String
where
    T: SizedSample + FromSample<f32> + Send + 'static,
    f32: FromSample<T>,
{
    let mut r = String::new();
    let ms = |s: f64| s * 1000.0 / o.rate as f64;
    for &buffer in &o.buffers {
        let _ = writeln!(r, "\nBuffer {buffer} frames ({:.2} ms)", ms(buffer as f64));
        let mut run_rt = Vec::new();
        let mut run_err = Vec::new();
        for run in 1..=o.repeats {
            match one_run::<T>(device, o, buffer, in_chs, out_chs) {
                Err(e) => {
                    let _ = writeln!(r, "  run {run}: failed: {e}");
                }
                Ok(pulses) => {
                    let found: Vec<&Pulse> = pulses.iter().flatten().collect();
                    if found.is_empty() {
                        let _ = writeln!(
                            r,
                            "  run {run}: no pulses heard. Check the loopback cable (output {} to input {}) and levels.",
                            o.out_ch, o.in_ch
                        );
                        continue;
                    }
                    let mut rt: Vec<f64> = found.iter().map(|p| p.round_trip).collect();
                    let rt_spread = spread(&rt);
                    let rt_med = median(&mut rt);
                    let mut te: Vec<f64> = found.iter().filter_map(|p| p.ts_error).collect();
                    let te_med = median(&mut te);
                    let mut rep: Vec<f64> = found.iter().filter_map(|p| p.reported).collect();
                    let rep_med = median(&mut rep);
                    let min_snr = found.iter().map(|p| p.snr).fold(f32::INFINITY, f32::min);
                    let _ = writeln!(
                        r,
                        "  run {run}: {}/{PULSES} pulses; round trip {rt_med:.2} frames ({:.3} ms, spread {rt_spread:.2}); driver reports {rep_med:.1} frames; timestamp error {te_med:+.2} frames; weakest pulse {min_snr:.0}x noise{}",
                        found.len(),
                        ms(rt_med),
                        if found.iter().any(|p| p.inverted) {
                            "; polarity inverted"
                        } else {
                            ""
                        }
                    );
                    run_rt.push(rt_med);
                    if te_med.is_finite() {
                        run_err.push(te_med);
                    }
                }
            }
        }
        if run_rt.len() >= 2 {
            let rs = spread(&run_rt);
            let _ = writeln!(
                r,
                "  across restarts: round trip varies by {rs:.2} frames; timestamp error varies by {:.2} frames",
                if run_err.len() >= 2 {
                    spread(&run_err)
                } else {
                    f64::NAN
                }
            );
            let _ = writeln!(
                r,
                "  verdict: {}",
                if rs <= 1.0 {
                    "STABLE: one fixed recording offset works for this buffer size"
                } else if rs < buffer as f64 {
                    "WITHIN A BLOCK: meets the ±1 block target, but the offset is not exact"
                } else {
                    "UNSTABLE: the offset changes by a block or more between restarts"
                }
            );
        }
    }
    r
}

fn main() {
    let o = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprintln!(
                "usage: latency --list\n       latency [--host asio|wasapi] [--device NAME] [--rate 48000] [--buffers 64,128,256] [--repeats 5] [--out 1] [--in 1]"
            );
            std::process::exit(2);
        }
    };
    if o.list {
        list();
        return;
    }
    let result = (|| -> Result<String, String> {
        let host = pick_host(o.host.as_deref())?;
        let device = pick_device(&host, o.device.as_deref())?;
        let ic = device
            .default_input_config()
            .map_err(|e| format!("input: {e}"))?;
        let oc = device
            .default_output_config()
            .map_err(|e| format!("output: {e}"))?;
        let mut head = String::new();
        let _ = writeln!(
            head,
            "Latency spike 0.4 ({} {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let _ = writeln!(
            head,
            "  host: {}, device: {}",
            host.id().name(),
            device_name(&device)
        );
        let _ = writeln!(
            head,
            "  {} Hz; pulses out on output {}, back in on input {}; {} restarts per buffer size",
            o.rate, o.out_ch, o.in_ch, o.repeats
        );
        print!("{head}");
        let body = match oc.sample_format() {
            SampleFormat::F32 => measure::<f32>(&device, &o, ic.channels(), oc.channels()),
            SampleFormat::I32 => measure::<i32>(&device, &o, ic.channels(), oc.channels()),
            SampleFormat::I16 => measure::<i16>(&device, &o, ic.channels(), oc.channels()),
            f => return Err(format!("sample format {f:?} not handled by this spike")),
        };
        Ok(head + &body)
    })();
    let report = result.unwrap_or_else(|e| format!("Latency spike failed: {e}\n"));
    print!("{report}");
    if std::fs::write("latency-report.txt", &report).is_ok() {
        println!("Saved to latency-report.txt");
    }
    if cfg!(windows) {
        println!("Press Enter to close.");
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
}
