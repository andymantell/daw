// SPDX-License-Identifier: GPL-3.0-or-later
//! The real-time side (ARCHITECTURE §4.3–4.4) and its controller.
//!
//! [`new`] returns a [`Controller`] for the UI side and an [`Engine`] for
//! the audio callback. They talk through lock-free queues: commands go
//! in, finished graphs come back to be freed off the audio thread, and
//! meters and the play position are shared atomics. [`Engine::process`]
//! never allocates, locks or blocks.
//!
//! The transport is sample-accurate: a block that crosses the loop end is
//! split there, and play continues from the loop start in the same block.
//! A count-in plays clicks only, for a whole number of bars before the
//! play position, then playback starts on the exact frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::graph::{Graph, MAX_TRACKS, Mix};
use crate::metronome::{self, Metronome, Voice};
use crate::time::{Frames, Ticks};

/// Messages from the controller to the audio thread.
pub enum Command {
    Play,
    /// Plays `bars` bars of clicks before the play position, then plays.
    CountIn {
        bars: u32,
    },
    Stop,
    /// Moves the play position.
    Seek(Frames),
    /// Loops `[start, end)` while playing; `None` turns looping off.
    SetLoop(Option<(Frames, Frames)>),
    /// A whole new graph. Ownership moves through the queue, so the audio
    /// thread only swaps a pointer.
    SetGraph(Box<Graph>),
    /// Mixer change on one track, without a new graph.
    SetMix {
        track: usize,
        mix: Mix,
    },
    SetMetronome(Metronome),
}

/// Shared with the UI: where playback is, and peak levels since the UI
/// last read them.
pub struct Status {
    position: AtomicI64,
    playing: AtomicBool,
    counting_in: AtomicBool,
    /// Peak per track, left and right, as `f32` bits. Positive floats
    /// order the same as their bits, so `fetch_max` works on them.
    track_peaks: Vec<[AtomicU32; 2]>,
    master_peaks: [AtomicU32; 2],
}

impl Status {
    fn new() -> Status {
        Status {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            counting_in: AtomicBool::new(false),
            track_peaks: (0..MAX_TRACKS)
                .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
                .collect(),
            master_peaks: [AtomicU32::new(0), AtomicU32::new(0)],
        }
    }

    pub fn position(&self) -> Frames {
        Frames(self.position.load(Ordering::Relaxed))
    }

    /// True while playing, including during a count-in.
    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// True during a count-in.
    pub fn counting_in(&self) -> bool {
        self.counting_in.load(Ordering::Relaxed)
    }

    /// Peak (left, right) on a track since the last call, and resets it.
    pub fn take_track_peak(&self, track: usize) -> (f32, f32) {
        self.track_peaks.get(track).map_or((0.0, 0.0), take_pair)
    }

    /// Peak (left, right) on the master bus since the last call.
    pub fn take_master_peak(&self) -> (f32, f32) {
        take_pair(&self.master_peaks)
    }
}

fn take_pair(p: &[AtomicU32; 2]) -> (f32, f32) {
    (
        f32::from_bits(p[0].swap(0, Ordering::Relaxed)),
        f32::from_bits(p[1].swap(0, Ordering::Relaxed)),
    )
}

fn raise(slot: &AtomicU32, v: f32) {
    if v.is_finite() && v > 0.0 {
        slot.fetch_max(v.to_bits(), Ordering::Relaxed);
    }
}

/// The UI-side handle.
pub struct Controller {
    commands: Producer<Command>,
    retired: Consumer<Box<Graph>>,
    status: Arc<Status>,
    max_block: usize,
}

impl Controller {
    /// Queues a command. Returns it if the queue is full.
    pub fn send(&mut self, cmd: Command) -> Result<(), Command> {
        self.commands
            .push(cmd)
            .map_err(|rtrb::PushError::Full(c)| c)
    }

    /// The block size graphs must be compiled for.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    pub fn status(&self) -> &Status {
        &self.status
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

/// The audio-thread side.
pub struct Engine {
    commands: Consumer<Command>,
    retired: Producer<Box<Graph>>,
    status: Arc<Status>,
    graph: Option<Box<Graph>>,
    max_block: usize,
    playing: bool,
    pos: i64,
    looping: Option<(i64, i64)>,
    metronome: Metronome,
    voice: Option<Voice>,
    /// During a count-in: where playback starts. `pos` runs up to it.
    count_in_to: Option<i64>,
    /// Clicks for the current block, preallocated.
    click_buf: Vec<f32>,
    /// Per-block track peaks, preallocated.
    peaks: Vec<(f32, f32)>,
}

/// Commands that fit in the queue before `send` refuses more.
const QUEUE: usize = 1024;

/// Creates a connected controller and engine. `max_block` is the largest
/// block the audio callback will ask for; larger ones are split.
pub fn new(max_block: usize) -> (Controller, Engine) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(QUEUE);
    // Room for every graph that could be in flight at once.
    let (ret_tx, ret_rx) = RingBuffer::new(QUEUE + 1);
    let status = Arc::new(Status::new());
    let max_block = max_block.max(1);
    (
        Controller {
            commands: cmd_tx,
            retired: ret_rx,
            status: status.clone(),
            max_block,
        },
        Engine {
            commands: cmd_rx,
            retired: ret_tx,
            status,
            graph: None,
            max_block,
            playing: false,
            pos: 0,
            looping: None,
            metronome: Metronome::default(),
            voice: None,
            count_in_to: None,
            click_buf: vec![0.0; max_block],
            peaks: vec![(0.0, 0.0); MAX_TRACKS],
        },
    )
}

impl Engine {
    fn apply_commands(&mut self) {
        while let Ok(cmd) = self.commands.pop() {
            match cmd {
                Command::Play => self.playing = true,
                Command::CountIn { bars } => {
                    if let Some(g) = &self.graph {
                        let map = g.tempo();
                        let start = map.ticks_at(Frames(self.pos));
                        let bar = map.signature_at(start).bar_ticks();
                        let from = map.frames_at(Ticks(start.0 - bar * bars as i64));
                        if from.0 < self.pos {
                            self.count_in_to = Some(self.pos);
                            self.pos = from.0;
                        }
                    }
                    self.playing = true;
                }
                Command::Stop => {
                    self.playing = false;
                    if let Some(to) = self.count_in_to.take() {
                        self.pos = to;
                    }
                    self.voice = None;
                }
                Command::Seek(f) => {
                    self.pos = f.0;
                    self.count_in_to = None;
                    self.voice = None;
                }
                Command::SetLoop(l) => {
                    self.looping = l.filter(|(a, b)| b > a).map(|(a, b)| (a.0, b.0));
                }
                Command::SetGraph(g) => {
                    if let Some(old) = self.graph.replace(g) {
                        // The queue has room for every graph in flight;
                        // if it is somehow full, leaking beats freeing here.
                        if let Err(rtrb::PushError::Full(old)) = self.retired.push(old) {
                            std::mem::forget(old);
                        }
                    }
                }
                Command::SetMix { track, mix } => {
                    if let Some(g) = &mut self.graph {
                        g.set_mix(track, mix);
                    }
                }
                Command::SetMetronome(m) => self.metronome = m,
            }
        }
    }

    /// Fills one block of stereo output, with any clicks mixed in. Both
    /// slices must be the same length.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.process_split(left, right, None);
    }

    /// Like [`process`](Self::process), with a separate mono click output.
    /// Clicks go there when the metronome asks for a separate output and
    /// one is given; otherwise they go into the main output.
    pub fn process_split(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        mut click: Option<&mut [f32]>,
    ) {
        debug_assert_eq!(left.len(), right.len());
        self.apply_commands();
        left.fill(0.0);
        right.fill(0.0);
        if let Some(c) = click.as_deref_mut() {
            c.fill(0.0);
        }
        let n = left.len().min(right.len());
        let mut done = 0;
        while self.playing && done < n {
            let mut len = (n - done).min(self.max_block);
            let counting = self.count_in_to.is_some();
            if let Some(to) = self.count_in_to {
                len = len.min((to - self.pos) as usize);
            } else if let Some((start, end)) = self.looping {
                if self.pos >= end {
                    // Seeked past the loop, or the loop moved: back to its
                    // start. Before the loop, play runs into it.
                    self.pos = start;
                }
                len = len.min((end - self.pos) as usize);
            }
            let range = done..done + len;
            if !counting {
                self.render(&mut left[range.clone()], &mut right[range.clone()]);
            }
            if counting || self.metronome.enabled {
                self.clicks(len);
                let clicks = &self.click_buf[..len];
                match click.as_deref_mut() {
                    Some(c) if self.metronome.separate_output => {
                        for (o, x) in c[range].iter_mut().zip(clicks) {
                            *o += x;
                        }
                    }
                    _ => {
                        for ((l, r), x) in left[range.clone()]
                            .iter_mut()
                            .zip(&mut right[range])
                            .zip(clicks)
                        {
                            *l += x;
                            *r += x;
                        }
                    }
                }
            }
            self.pos += len as i64;
            done += len;
            if self.count_in_to == Some(self.pos) {
                self.count_in_to = None;
            } else if let Some((start, end)) = self.looping
                && !counting
                && self.pos == end
            {
                self.pos = start;
            }
        }
        self.status.position.store(self.pos, Ordering::Relaxed);
        self.status.playing.store(self.playing, Ordering::Relaxed);
        self.status
            .counting_in
            .store(self.count_in_to.is_some(), Ordering::Relaxed);
    }

    /// Renders `len` frames of clicks from `pos` into `click_buf`.
    fn clicks(&mut self, len: usize) {
        let buf = &mut self.click_buf[..len];
        buf.fill(0.0);
        if let Some(g) = &self.graph {
            metronome::render(
                g.tempo(),
                &g.clicks,
                &self.metronome,
                &mut self.voice,
                self.pos,
                buf,
            );
        }
    }

    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let Some(g) = &mut self.graph else {
            return;
        };
        let tracks = g.track_count();
        g.render(self.pos, [left, right], &mut self.peaks[..tracks]);
        for (slot, p) in self.status.track_peaks.iter().zip(&self.peaks[..tracks]) {
            raise(&slot[0], p.0);
            raise(&slot[1], p.1);
        }
        let ml = left.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let mr = right.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        raise(&self.status.master_peaks[0], ml);
        raise(&self.status.master_peaks[1], mr);
    }
}

/// Renders a graph offline, through the same code as real-time playback,
/// from `from` for `frames` frames. Used for export and tests.
pub fn render_offline(graph: Graph, from: Frames, frames: usize, block: usize) -> [Vec<f32>; 2] {
    let (mut ctl, mut eng) = new(block);
    let _ = ctl.send(Command::SetGraph(Box::new(graph)));
    let _ = ctl.send(Command::Seek(from));
    let _ = ctl.send(Command::Play);
    let mut out = [vec![0.0; frames], vec![0.0; frames]];
    let [l, r] = &mut out;
    for (bl, br) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        eng.process(bl, br);
    }
    out
}
