// SPDX-License-Identifier: GPL-3.0-or-later
//! Measuring the recording offset with a loopback cable (roadmap 1.1 and
//! 1.2, from spike 0.4).
//!
//! The user connects an output of the interface straight to an input.
//! [`Measuring`] opens the device with a [`LoopbackTest`] in the engine's
//! place, through the same input queue the engine uses, so what it
//! measures is exactly how late recorded audio reaches the engine: the
//! interface's round trip plus the queue's slack. It plays a few short
//! test pulses, finds each one in the recording to a fraction of a sample,
//! and starts the audio again a few times, because an offset is only
//! usable if it is the same every time the audio starts.
//!
//! The pulse is a short swept sine. Cross-correlating the recording with
//! it gives a sharp peak at the pulse's arrival, which survives noise,
//! level changes and a polarity flip in the interface.

use std::f32::consts::PI;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::{DeviceProfile, Duplex, Running, SilentInput};

/// A 20 ms sweep from 200 Hz to 8 kHz with a raised-cosine envelope.
pub fn probe(rate: u32) -> Vec<f32> {
    let n = (rate as f32 * 0.02) as usize;
    let (f0, f1) = (200.0f32, 8_000.0f32);
    let dur = n as f32 / rate as f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let phase = 2.0 * PI * (f0 * t + 0.5 * (f1 - f0) / dur * t * t);
            let env = 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos();
            0.5 * env * phase.sin()
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    /// Index in the recording where the pulse starts, to a fraction of a
    /// sample.
    pub at: f64,
    /// Correlation peak over the median correlation level.
    pub snr: f32,
    /// The interface flipped the signal's polarity.
    pub inverted: bool,
}

/// Searches `rec[from..to]` for the start of `probe`.
pub fn find(rec: &[f32], probe: &[f32], from: usize, to: usize) -> Option<Found> {
    let to = to.min(rec.len().saturating_sub(probe.len()));
    if from + 2 >= to {
        return None;
    }
    let corr: Vec<f32> = (from..to)
        .map(|s| {
            rec[s..s + probe.len()]
                .iter()
                .zip(probe)
                .map(|(a, b)| a * b)
                .sum()
        })
        .collect();
    let (i, &peak) = corr
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    let mut mags: Vec<f32> = corr.iter().map(|c| c.abs()).collect();
    mags.sort_by(f32::total_cmp);
    let median = mags[mags.len() / 2].max(1e-12);
    // Parabolic interpolation on the magnitude for a fractional position.
    let frac = if i > 0 && i + 1 < corr.len() {
        let (a, b, c) = (corr[i - 1].abs(), peak.abs(), corr[i + 1].abs());
        let d = a - 2.0 * b + c;
        if d.abs() > 1e-12 {
            0.5 * (a - c) / d
        } else {
            0.0
        }
    } else {
        0.0
    };
    Some(Found {
        at: (from + i) as f64 + frac as f64,
        snr: peak.abs() / median,
        inverted: peak < 0.0,
    })
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

fn spread(v: &[f64]) -> f64 {
    let lo = v.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    hi - lo
}

/// Pulses played in each run.
pub const PULSES: usize = 4;
/// A pulse is only counted if it stands this far above the noise.
const MIN_SNR: f32 = 8.0;
/// Starts of the audio measured, by default.
pub const RUNS: usize = 3;

/// Where pulse `n` starts: a quarter of a second in, then every half
/// second. The longest round trip looked for is just under the gap.
fn pulse_start(n: usize, rate: u32) -> u64 {
    rate as u64 / 4 + n as u64 * rate as u64 / 2
}

/// The longest round trip looked for, in frames: 0.45 s, far more than
/// any interface at its largest buffer.
fn max_round_trip(rate: u32) -> usize {
    rate as usize * 9 / 20
}

/// Frames recorded in one run.
pub fn run_frames(rate: u32) -> usize {
    pulse_start(PULSES, rate) as usize
}

/// Plays the pulses on the main outputs and records one input channel:
/// stands in for the engine while measuring.
pub struct LoopbackTest {
    probe: Vec<f32>,
    rate: u32,
    channel: usize,
    frame: u64,
    total: u64,
    rec: Producer<f32>,
}

impl LoopbackTest {
    /// A test recording from input `channel` (from 0), and the queue its
    /// recording arrives on, one sample per frame from the first block.
    pub fn new(rate: u32, channel: usize) -> (LoopbackTest, Consumer<f32>) {
        let total = run_frames(rate);
        let (rec, rx) = RingBuffer::new(total);
        let t = LoopbackTest {
            probe: probe(rate),
            rate,
            channel,
            frame: 0,
            total: total as u64,
            rec,
        };
        (t, rx)
    }
}

impl Duplex for LoopbackTest {
    fn process_duplex(
        &mut self,
        input: &[f32],
        channels: usize,
        left: &mut [f32],
        right: &mut [f32],
        click: Option<&mut [f32]>,
    ) {
        if let Some(c) = click {
            c.fill(0.0);
        }
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let k = self.frame + i as u64;
            let mut v = 0.0;
            for n in 0..PULSES {
                let s = pulse_start(n, self.rate);
                if k >= s && k < s + self.probe.len() as u64 {
                    v = self.probe[(k - s) as usize];
                }
            }
            (*l, *r) = (v, v);
            if k < self.total {
                let x = if self.channel < channels {
                    input.get(i * channels + self.channel).copied()
                } else {
                    None
                };
                let _ = self.rec.push(x.unwrap_or(0.0));
            }
        }
        self.frame += left.len() as u64;
    }
}

/// One run's result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    /// How many frames after a pulse was played it arrived, the median
    /// over the pulses heard, to a fraction of a frame.
    pub round_trip: f64,
    pub heard: usize,
    /// The quietest pulse's level over the noise.
    pub weakest: f32,
    pub inverted: bool,
}

/// Why a measurement failed, in words for the user.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// Nothing came back.
    NotHeard,
    /// Some pulses came back and some didn't, or they came back at
    /// different times.
    Unreliable(String),
    /// The device could not be used.
    Device(String),
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Problem::NotHeard => write!(
                f,
                "No test pulses came back. Check the cable runs from the main left output to the chosen input, and that the input's gain is up."
            ),
            Problem::Unreliable(why) => write!(
                f,
                "{why} Try again with less noise, or turn the input gain up a little."
            ),
            Problem::Device(e) => write!(f, "{e}"),
        }
    }
}

/// Finds the pulses in one run's recording.
pub fn analyse(rec: &[f32], rate: u32) -> Result<Run, Problem> {
    let p = probe(rate);
    let found: Vec<(u64, Found)> = (0..PULSES)
        .filter_map(|n| {
            let k = pulse_start(n, rate);
            let from = k as usize;
            let f = find(rec, &p, from, from + max_round_trip(rate))?;
            (f.snr >= MIN_SNR).then_some((k, f))
        })
        .collect();
    if found.is_empty() {
        return Err(Problem::NotHeard);
    }
    if found.len() < PULSES - 1 {
        return Err(Problem::Unreliable(format!(
            "Only {} of {PULSES} test pulses came back.",
            found.len()
        )));
    }
    let mut rt: Vec<f64> = found.iter().map(|(k, f)| f.at - *k as f64).collect();
    let s = spread(&rt);
    if s > 2.0 {
        return Err(Problem::Unreliable(format!(
            "The test pulses came back at times {s:.0} frames apart."
        )));
    }
    Ok(Run {
        round_trip: median(&mut rt),
        heard: found.len(),
        weakest: found
            .iter()
            .map(|(_, f)| f.snr)
            .fold(f32::INFINITY, f32::min),
        inverted: found.iter().any(|(_, f)| f.inverted),
    })
}

/// A finished measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// The recording offset to use, in whole frames.
    pub frames: u32,
    pub rate: u32,
    /// How much the round trip changed between starts of the audio, in
    /// frames.
    pub spread: f64,
    pub runs: Vec<Run>,
}

impl Measured {
    pub fn ms(&self) -> f64 {
        self.frames as f64 * 1000.0 / self.rate as f64
    }

    /// The same, to within a frame, every time the audio started: one
    /// fixed offset lines recordings up exactly.
    pub fn stable(&self) -> bool {
        self.spread <= 1.0
    }

    pub fn inverted(&self) -> bool {
        self.runs.iter().any(|r| r.inverted)
    }
}

/// Puts runs together into one offset.
pub fn combine(runs: Vec<Run>, rate: u32) -> Measured {
    let mut rt: Vec<f64> = runs.iter().map(|r| r.round_trip).collect();
    let s = spread(&rt);
    let m = median(&mut rt);
    Measured {
        frames: m.round().max(0.0) as u32,
        rate,
        spread: s,
        runs,
    }
}

/// What to measure on.
#[derive(Clone, Debug)]
pub enum Target {
    Device(DeviceProfile),
    /// No device: the silent stand-in, which loops its output back with
    /// [`SilentInput::Loopback`]. For testing without an interface.
    Silent {
        rate: u32,
        buffer: u32,
        input: SilentInput,
    },
}

impl Target {
    fn rate(&self) -> u32 {
        match self {
            Target::Device(p) => p.rate,
            Target::Silent { rate, .. } => *rate,
        }
    }
}

/// A measurement in progress. The audio device is held for the whole
/// measurement, so the app's own audio must be stopped first.
pub struct Measuring {
    target: Target,
    channel: usize,
    wanted: usize,
    runs: Vec<Run>,
    current: Option<Current>,
    /// The last run's recording being searched, off the UI thread: at
    /// high sample rates it takes a moment.
    analysing: Option<std::sync::mpsc::Receiver<Result<Run, Problem>>>,
}

struct Current {
    running: Running,
    rx: Consumer<f32>,
    rec: Vec<f32>,
    started: Instant,
}

impl Measuring {
    /// Starts measuring `runs` starts of the audio, recording from input
    /// `channel` (from 0).
    pub fn start(target: Target, channel: usize, runs: usize) -> Result<Measuring, Problem> {
        let mut m = Measuring {
            target,
            channel,
            wanted: runs.max(1),
            runs: Vec::new(),
            current: None,
            analysing: None,
        };
        m.next_run()?;
        Ok(m)
    }

    fn next_run(&mut self) -> Result<(), Problem> {
        // Close the last run's stream before opening the next.
        self.current = None;
        let rate = self.target.rate();
        let (test, rx) = LoopbackTest::new(rate, self.channel);
        let running = match &self.target {
            Target::Device(p) => {
                if p.input.is_none() {
                    return Err(Problem::Device("Choose an input device first.".into()));
                }
                crate::start(p, test).map_err(Problem::Device)?
            }
            Target::Silent {
                rate,
                buffer,
                input,
            } => crate::start_silent_with(*rate, *buffer, test, *input),
        };
        if let Some(e) = &running.input_error {
            return Err(Problem::Device(format!("No input: {e}")));
        }
        if self.channel >= running.input_channels as usize {
            return Err(Problem::Device(format!(
                "The input has no channel {}.",
                self.channel + 1
            )));
        }
        self.current = Some(Current {
            running,
            rx,
            rec: Vec::with_capacity(run_frames(rate)),
            started: Instant::now(),
        });
        Ok(())
    }

    /// How far through, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let per = run_frames(self.target.rate()) as f32;
        let now = self.current.as_ref().map_or(0, |c| c.rec.len()) as f32;
        ((self.runs.len() as f32 + now / per) / self.wanted as f32).min(1.0)
    }

    /// Moves the measurement on. Call it often (every frame of the UI);
    /// it returns the result once there is one. The device is closed by
    /// then.
    pub fn poll(&mut self) -> Option<Result<Measured, Problem>> {
        let rate = self.target.rate();
        let total = run_frames(rate);
        if let Some(rx) = &self.analysing {
            let result = match rx.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => return None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(Problem::Device(
                    "The measurement stopped unexpectedly.".into(),
                )),
            };
            self.analysing = None;
            match result {
                Ok(run) => self.runs.push(run),
                Err(e) => return Some(Err(e)),
            }
            if self.runs.len() >= self.wanted {
                return Some(Ok(combine(std::mem::take(&mut self.runs), rate)));
            }
            return self.next_run().err().map(Err);
        }
        let c = self.current.as_mut()?;
        if let Ok(chunk) = c.rx.read_chunk(c.rx.slots()) {
            c.rec.extend(chunk);
        }
        if c.running.status.failed.load(Ordering::Relaxed) {
            self.current = None;
            return Some(Err(Problem::Device(
                "The audio device stopped with an error.".into(),
            )));
        }
        if c.rec.len() < total {
            let limit = Duration::from_secs_f64(total as f64 / rate as f64 + 3.0);
            if c.started.elapsed() > limit {
                self.current = None;
                return Some(Err(Problem::Device(
                    "The audio device stopped running.".into(),
                )));
            }
            return None;
        }
        let rec = std::mem::take(&mut c.rec);
        self.current = None;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(analyse(&rec, rate));
        });
        self.analysing = Some(rx);
        None
    }

    /// Still recording or searching.
    pub fn busy(&self) -> bool {
        self.current.is_some() || self.analysing.is_some()
    }
}

/// A loopback cable in software: what was played comes back `delay`
/// frames later, scaled by `gain`. For the silent stand-in and tests.
pub(crate) struct SimLoopback {
    queue: std::collections::VecDeque<f32>,
    gain: f32,
}

impl SimLoopback {
    /// `delay` must be at least the block size, as for a real device:
    /// a block's input is captured before its output is made.
    pub(crate) fn new(delay: usize, gain: f32) -> SimLoopback {
        SimLoopback {
            queue: std::iter::repeat_n(0.0, delay).collect(),
            gain,
        }
    }

    /// Fills `input` (one channel per `channels`, all the same) with what
    /// comes back now.
    pub(crate) fn input(&mut self, input: &mut [f32], channels: usize) {
        for frame in input.chunks_mut(channels) {
            frame.fill(self.queue.pop_front().unwrap_or(0.0) * self.gain);
        }
    }

    /// Sends a block of output down the cable.
    pub(crate) fn output(&mut self, out: &[f32]) {
        self.queue.extend(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, level: f32, seed: u32) -> Vec<f32> {
        let mut s = 0x9e37_79b9u32 ^ seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                level * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    #[test]
    fn finds_whole_sample_delay_through_noise_and_inversion() {
        let p = probe(48_000);
        let mut rec = noise(48_000, 0.05, 0);
        for (i, v) in p.iter().enumerate() {
            rec[10_123 + i] -= 0.3 * v;
        }
        let f = find(&rec, &p, 8_000, 20_000).unwrap();
        assert!((f.at - 10_123.0).abs() < 0.05, "{f:?}");
        assert!(f.inverted);
        assert!(f.snr > 10.0, "{f:?}");
    }

    #[test]
    fn finds_fractional_delay() {
        let p = probe(48_000);
        // Half-sample delay by averaging neighbours (a crude but known one).
        let mut rec = vec![0.0f32; 20_000];
        for i in 0..p.len() {
            rec[5_000 + i] += 0.5 * p[i];
            rec[5_001 + i] += 0.5 * p[i];
        }
        let f = find(&rec, &p, 4_000, 6_000).unwrap();
        assert!((f.at - 5_000.5).abs() < 0.2, "{f:?}");
    }

    #[test]
    fn silence_has_low_snr() {
        let p = probe(48_000);
        let rec = noise(20_000, 0.05, 0);
        let f = find(&rec, &p, 0, 10_000).unwrap();
        assert!(f.snr < 8.0, "{f:?}");
    }

    /// Runs the test through a simulated cable the way a device would:
    /// in blocks, each block's input captured before its output is made,
    /// with noise and a gain (negative to flip polarity) on the way back.
    fn simulate(rate: u32, block: usize, delay: usize, gain: f32, level: f32) -> Vec<f32> {
        let (mut test, mut rx) = LoopbackTest::new(rate, 1);
        let mut cable = SimLoopback::new(delay, gain);
        let mut hiss = noise(run_frames(rate) + 2 * block, level, delay as u32).into_iter();
        let (mut l, mut r) = (vec![0.0; block], vec![0.0; block]);
        let mut inp = vec![0.0; block * 2];
        let mut rec = Vec::new();
        while rec.len() < run_frames(rate) {
            cable.input(&mut inp, 2);
            for x in &mut inp {
                *x += hiss.next().unwrap_or(0.0);
            }
            test.process_duplex(&inp, 2, &mut l, &mut r, None);
            cable.output(&l);
            while let Ok(x) = rx.pop() {
                rec.push(x);
            }
        }
        rec
    }

    #[test]
    fn measures_a_simulated_cable() {
        for (rate, block, delay) in [(48_000, 128, 271), (44_100, 64, 64), (96_000, 512, 2_437)] {
            let run = analyse(&simulate(rate, block, delay, 0.5, 0.02), rate).unwrap();
            assert!(
                (run.round_trip - delay as f64).abs() < 0.1,
                "{rate} {delay}: {run:?}"
            );
            assert_eq!(run.heard, PULSES);
            assert!(!run.inverted);
        }
    }

    #[test]
    fn a_quiet_inverted_return_still_measures() {
        let run = analyse(&simulate(48_000, 256, 700, -0.05, 0.01), 48_000).unwrap();
        assert!((run.round_trip - 700.0).abs() < 0.1, "{run:?}");
        assert!(run.inverted);
    }

    #[test]
    fn no_cable_is_reported() {
        let rec = simulate(48_000, 128, 300, 0.0, 0.02);
        assert_eq!(analyse(&rec, 48_000), Err(Problem::NotHeard));
    }

    #[test]
    fn pulses_at_different_times_are_unreliable() {
        let rate = 48_000;
        let p = probe(rate);
        let mut rec = noise(run_frames(rate), 0.01, 7);
        for n in 0..PULSES {
            let at = pulse_start(n, rate) as usize + 300 + 40 * n;
            for (i, v) in p.iter().enumerate() {
                rec[at + i] += 0.5 * v;
            }
        }
        assert!(matches!(analyse(&rec, rate), Err(Problem::Unreliable(_))));
    }

    #[test]
    fn runs_combine_and_report_stability() {
        let run = |rt| Run {
            round_trip: rt,
            heard: PULSES,
            weakest: 50.0,
            inverted: false,
        };
        let m = combine(vec![run(270.8), run(271.1), run(271.0)], 48_000);
        assert_eq!(m.frames, 271);
        assert!(m.stable());
        let m = combine(vec![run(271.0), run(399.0), run(271.0)], 48_000);
        assert_eq!(m.frames, 271);
        assert!(!m.stable());
    }

    /// The whole measurement, through the silent stand-in's cable in real
    /// time (about two and a half seconds a run).
    #[test]
    fn measures_through_the_silent_device() {
        let target = Target::Silent {
            rate: 48_000,
            buffer: 128,
            input: SilentInput::Loopback { delay: 300 },
        };
        let mut m = Measuring::start(target, 0, 1).unwrap();
        let started = Instant::now();
        let result = loop {
            if let Some(r) = m.poll() {
                break r;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(20));
        };
        let got = result.unwrap();
        assert_eq!(got.frames, 300, "{got:?}");
        assert_eq!(got.runs.len(), 1);
    }
}
