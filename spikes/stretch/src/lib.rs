// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.6: real-time speed and pitch change for guitar practice.
//!
//! Three candidates behind one interface:
//! - `signalsmith`: the Signalsmith Stretch library (MIT, C++) through its
//!   Rust wrapper. Speed and pitch.
//! - `timestretch`: a pure-Rust library (MIT), using its wide-range keylock
//!   engine. Speed only: its pitch shifting is offline.
//! - `own`: our phase vocoder written from the literature (see `pv`).

pub mod pv;
pub mod signals;

use signalsmith_stretch::Stretch as Ss;
use timestretch::engine::{
    Engine, EngineConfig, EngineController, EngineProcessor, EngineProfile, SourceProducer,
};

pub const RATE: u32 = 48_000;

/// A clip playing from memory. Reads past the end wrap round to the start,
/// so long benchmarks can run from short material.
pub struct Feed<'a> {
    data: &'a [f32],
    pos: u64,
    /// Input samples used so far, as reported by the stretcher.
    pub consumed: f64,
}

impl<'a> Feed<'a> {
    pub fn new(data: &'a [f32]) -> Self {
        Feed {
            data,
            pos: 0,
            consumed: 0.0,
        }
    }

    /// Random access, for stretchers that read where they like.
    pub fn at(&self, i: i64) -> f32 {
        self.data[i.rem_euclid(self.data.len() as i64) as usize]
    }

    /// Sequential access, for stretchers that are fed a stream.
    pub fn read(&mut self, dst: &mut [f32]) {
        for d in dst.iter_mut() {
            *d = self.data[(self.pos % self.data.len() as u64) as usize];
            self.pos += 1;
        }
        self.consumed = self.pos as f64;
    }
}

pub trait Stretcher {
    fn name(&self) -> &str;
    /// Returns false if this candidate cannot do that setting.
    fn set(&mut self, speed: f64, semitones: f64) -> bool;
    /// Fills `out` at the current settings. Called on the audio thread.
    fn process(&mut self, feed: &mut Feed, out: &mut [f32]);
}

// ---------------------------------------------------------------------------

pub struct Signalsmith {
    inner: Ss,
    speed: f64,
    carry: f64,
    input: Vec<f32>,
}

impl Signalsmith {
    pub fn new() -> Self {
        Signalsmith {
            inner: Ss::preset_default(1, RATE),
            speed: 1.0,
            carry: 0.0,
            input: vec![0.0; 16_384],
        }
    }
}

impl Default for Signalsmith {
    fn default() -> Self {
        Self::new()
    }
}

impl Stretcher for Signalsmith {
    fn name(&self) -> &str {
        "signalsmith"
    }
    fn set(&mut self, speed: f64, semitones: f64) -> bool {
        self.speed = speed;
        self.inner
            .set_transpose_factor_semitones(semitones as f32, None);
        true
    }
    fn process(&mut self, feed: &mut Feed, out: &mut [f32]) {
        let want = self.carry + self.speed * out.len() as f64;
        let n = (want.floor() as usize).min(self.input.len());
        self.carry = want - n as f64;
        feed.read(&mut self.input[..n]);
        self.inner.process(&self.input[..n], &mut *out);
    }
}

// ---------------------------------------------------------------------------

pub struct TimeStretch {
    controller: EngineController,
    processor: EngineProcessor,
    source: SourceProducer,
    chunk: Vec<f32>,
}

impl TimeStretch {
    pub fn new(block: usize) -> Self {
        let h = Engine::build(EngineConfig {
            sample_rate: RATE,
            channels: 1,
            profile: EngineProfile::WideKeylock,
            initial_tempo_rate: 1.0,
            max_block_frames: block,
            source_capacity_frames: 65_536,
            pre_analysis: None,
        })
        .expect("engine config");
        TimeStretch {
            controller: h.controller,
            processor: h.processor,
            source: h.source,
            chunk: vec![0.0; 65_536],
        }
    }

    fn consumed(&self) -> f64 {
        (self.source.pushed_frames() - self.source.occupied_frames() as u64) as f64
    }
}

impl Stretcher for TimeStretch {
    fn name(&self) -> &str {
        "timestretch"
    }
    fn set(&mut self, speed: f64, semitones: f64) -> bool {
        if semitones != 0.0 {
            return false;
        }
        self.controller.set_tempo_rate(speed);
        true
    }
    fn process(&mut self, feed: &mut Feed, out: &mut [f32]) {
        // Keep the source ring topped up, as a disk-reading thread would.
        let free = self.source.free_frames();
        if free > 0 {
            feed.read(&mut self.chunk[..free]);
            self.source.push(&self.chunk[..free]);
        }
        self.processor.process(out);
        feed.consumed = self.consumed();
    }
}

pub const CANDIDATES: [&str; 4] = ["signalsmith", "timestretch", "own-2048", "own-4096"];

/// A fresh instance of a candidate by name.
pub fn build(name: &str, block: usize) -> Box<dyn Stretcher> {
    match name {
        "signalsmith" => Box::new(Signalsmith::new()),
        "timestretch" => Box::new(TimeStretch::new(block)),
        "own-2048" => Box::new(pv::PhaseVocoder::new("own-2048", 2048, 512)),
        "own-4096" => Box::new(pv::PhaseVocoder::new("own-4096", 4096, 512)),
        _ => panic!("unknown candidate {name}"),
    }
}
