// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 D: a lock-free command path from the UI to the audio thread,
//! and a render graph swapped without glitches or audio-thread allocation.
//!
//! Throwaway prototype. The real engine is designed in ARCHITECTURE §4.

pub mod rt_check;

use rtrb::{Consumer, Producer, RingBuffer};

/// Largest block the engine accepts. Scratch buffers are sized to this
/// up front so the audio thread never allocates.
pub const MAX_BLOCK: usize = 1024;

/// One sine oscillator. Stands in for a real processor node.
#[derive(Clone, Debug)]
pub struct Osc {
    pub freq_hz: f32,
    pub gain: f32,
    phase: f32,
    smoothed_gain: f32,
}

impl Osc {
    pub fn new(freq_hz: f32, gain: f32) -> Self {
        Self::with_phase(freq_hz, gain, 0.0)
    }

    pub fn with_phase(freq_hz: f32, gain: f32, phase: f32) -> Self {
        Self {
            freq_hz,
            gain,
            phase,
            smoothed_gain: gain,
        }
    }
}

/// An immutable-shape render graph: built on the UI side, then handed to
/// the audio thread whole.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub oscs: Vec<Osc>,
}

impl Graph {
    pub fn new(oscs: Vec<Osc>) -> Self {
        Self { oscs }
    }

    /// Adds this graph's output into `out`.
    fn render_add(&mut self, out: &mut [f32], sample_rate: f32) {
        // One-pole smoothing (about 5 ms) so gain changes don't click.
        let smooth = 1.0 - (-1.0 / (0.005 * sample_rate)).exp();
        for osc in &mut self.oscs {
            let inc = std::f32::consts::TAU * osc.freq_hz / sample_rate;
            for s in out.iter_mut() {
                osc.smoothed_gain += (osc.gain - osc.smoothed_gain) * smooth;
                *s += osc.phase.sin() * osc.smoothed_gain;
                osc.phase += inc;
                if osc.phase >= std::f32::consts::TAU {
                    osc.phase -= std::f32::consts::TAU;
                }
            }
        }
    }
}

/// Messages from the UI thread to the audio thread.
pub enum Command {
    SetGain {
        osc: usize,
        gain: f32,
    },
    /// A complete new graph. Ownership moves through the queue, so the
    /// audio thread only swaps a pointer.
    SwapGraph(Box<Graph>),
}

/// How a graph swap is applied. `Crossfade` is the real behaviour;
/// `Hard` exists so tests can prove the glitch detector works.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwapMode {
    Crossfade,
    Hard,
}

/// The UI-side handle.
pub struct Controller {
    commands: Producer<Command>,
    retired: Consumer<Box<Graph>>,
}

impl Controller {
    /// Queues a command. Returns it back if the queue is full.
    pub fn send(&mut self, cmd: Command) -> Result<(), Command> {
        self.commands
            .push(cmd)
            .map_err(|rtrb::PushError::Full(c)| c)
    }

    /// Frees graphs the audio thread has finished with. Call regularly
    /// from a non-real-time thread.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while let Ok(old) = self.retired.pop() {
            drop(old);
            n += 1;
        }
        n
    }
}

/// The audio-thread side. Everything it needs is allocated in [`new`].
pub struct Engine {
    sample_rate: f32,
    mode: SwapMode,
    current: Box<Graph>,
    /// The graph being faded out during a crossfade block.
    outgoing: Option<Box<Graph>>,
    /// Retired graphs waiting for room in the return queue.
    pending_retire: [Option<Box<Graph>>; 8],
    commands: Consumer<Command>,
    retired: Producer<Box<Graph>>,
    scratch: Vec<f32>,
}

pub fn new(initial: Graph, sample_rate: f32, mode: SwapMode) -> (Controller, Engine) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(256);
    let (ret_tx, ret_rx) = RingBuffer::new(64);
    let engine = Engine {
        sample_rate,
        mode,
        current: Box::new(initial),
        outgoing: None,
        pending_retire: Default::default(),
        commands: cmd_rx,
        retired: ret_tx,
        scratch: vec![0.0; MAX_BLOCK],
    };
    (
        Controller {
            commands: cmd_tx,
            retired: ret_rx,
        },
        engine,
    )
}

impl Engine {
    /// Renders one block. Real-time safe: no allocation, locking or
    /// freeing.
    pub fn process(&mut self, out: &mut [f32]) {
        assert!(out.len() <= MAX_BLOCK);
        self.retire_pending();

        while let Ok(cmd) = self.commands.pop() {
            match cmd {
                Command::SetGain { osc, gain } => {
                    if let Some(o) = self.current.oscs.get_mut(osc) {
                        o.gain = gain;
                    }
                }
                Command::SwapGraph(new) => {
                    let old = std::mem::replace(&mut self.current, new);
                    if self.outgoing.is_none() {
                        // Fade out what was audible in the last block.
                        self.outgoing = Some(old);
                    } else {
                        // A second swap in the same block: `old` was
                        // never heard, so retire it and keep fading out
                        // the graph that was.
                        self.retire(old);
                    }
                }
            }
        }

        out.fill(0.0);
        self.current.render_add(out, self.sample_rate);

        if let Some(mut old) = self.outgoing.take() {
            if self.mode == SwapMode::Crossfade {
                let n = out.len();
                let fade = &mut self.scratch[..n];
                fade.fill(0.0);
                old.render_add(fade, self.sample_rate);
                for (i, (o, f)) in out.iter_mut().zip(fade.iter()).enumerate() {
                    let t = (i + 1) as f32 / n as f32;
                    *o = *o * t + *f * (1.0 - t);
                }
            }
            self.retire(old);
        }
    }

    fn retire(&mut self, graph: Box<Graph>) {
        self.retire_pending();
        if let Err(rtrb::PushError::Full(g)) = self.retired.push(graph) {
            // Queue full: hold on to it rather than freeing it here.
            let slot = self.pending_retire.iter_mut().find(|s| s.is_none());
            *slot.expect("UI thread is not collecting retired graphs") = Some(g);
        }
    }

    fn retire_pending(&mut self) {
        for slot in &mut self.pending_retire {
            if let Some(g) = slot.take()
                && let Err(rtrb::PushError::Full(g)) = self.retired.push(g)
            {
                *slot = Some(g);
                return;
            }
        }
    }
}
