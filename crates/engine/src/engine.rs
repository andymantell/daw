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
//!
//! With device input ([`Engine::process_duplex`]), tracks can monitor
//! their input at any time, and armed tracks record while the transport
//! records and plays (not during the count-in); see [`crate::record`].

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::graph::{Graph, InputBlock, MAX_TRACKS, Mix};
use crate::metronome::{self, Metronome, Voice};
use crate::record::{BLOCK_FRAMES, RecBlock, Recorder, Take};
use crate::stream::{Chunk, Placement, Playhead, Streamer, ToAudio};
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
    /// Records armed tracks whenever playing (after any count-in).
    SetRecording(bool),
}

/// Shared with the UI: where playback is, and peak levels since the UI
/// last read them.
pub struct Status {
    position: AtomicI64,
    playing: AtomicBool,
    counting_in: AtomicBool,
    loop_on: AtomicBool,
    loop_start: AtomicI64,
    loop_end: AtomicI64,
    /// Frames of streamed audio played as silence because the disk had
    /// not delivered them yet.
    underruns: AtomicU64,
    /// Peak per track, left and right, as `f32` bits. Positive floats
    /// order the same as their bits, so `fetch_max` works on them.
    track_peaks: Vec<[AtomicU32; 2]>,
    master_peaks: [AtomicU32; 2],
    /// Input level per track, before the fader, whether monitored or not.
    input_peaks: Vec<[AtomicU32; 2]>,
    /// The transport is set to record (armed tracks record while playing).
    recording: AtomicBool,
    /// Recorded frames lost because the recording thread fell behind.
    rec_dropped: AtomicU64,
}

impl Status {
    fn new() -> Status {
        Status {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            counting_in: AtomicBool::new(false),
            loop_on: AtomicBool::new(false),
            loop_start: AtomicI64::new(0),
            loop_end: AtomicI64::new(0),
            underruns: AtomicU64::new(0),
            track_peaks: (0..MAX_TRACKS)
                .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
                .collect(),
            master_peaks: [AtomicU32::new(0), AtomicU32::new(0)],
            input_peaks: (0..MAX_TRACKS)
                .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
                .collect(),
            recording: AtomicBool::new(false),
            rec_dropped: AtomicU64::new(0),
        }
    }

    /// True while set to record, including when stopped and waiting to.
    pub fn recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Frames of recorded input lost because the disk fell behind.
    pub fn rec_dropped(&self) -> u64 {
        self.rec_dropped.load(Ordering::Relaxed)
    }

    /// Input peak (left, right) on a track since the last call. Mono
    /// inputs report the same value twice.
    pub fn take_input_peak(&self, track: usize) -> (f32, f32) {
        self.input_peaks.get(track).map_or((0.0, 0.0), take_pair)
    }

    pub fn position(&self) -> Frames {
        Frames(self.position.load(Ordering::Relaxed))
    }

    /// True while playing, including during a count-in.
    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// Frames of streamed audio that were missing when played.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    pub(crate) fn playhead(&self) -> Playhead {
        Playhead {
            position: self.position.load(Ordering::Relaxed),
            looping: self.loop_on.load(Ordering::Relaxed).then(|| {
                (
                    self.loop_start.load(Ordering::Relaxed),
                    self.loop_end.load(Ordering::Relaxed),
                )
            }),
        }
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

/// Returns a chunk to the disk thread to be freed there. The queue is
/// sized for every chunk in flight; if it is somehow full, leaking beats
/// freeing on the audio thread.
fn give_back(q: &mut Producer<Box<Chunk>>, c: Box<Chunk>) {
    if let Err(rtrb::PushError::Full(c)) = q.push(c) {
        std::mem::forget(c);
    }
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
    placements: Arc<Mutex<Vec<Placement>>>,
    streamer: Option<Streamer>,
    recorder: Option<Recorder>,
    record_dir: Arc<Mutex<std::path::PathBuf>>,
    record_rate: Arc<Mutex<u32>>,
    takes: Arc<Mutex<Vec<Take>>>,
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

    /// Sends a new graph, first telling the disk thread which files it
    /// streams.
    pub fn set_graph(&mut self, graph: Graph) -> Result<(), Command> {
        *self.placements.lock().unwrap() = graph.placements();
        *self.record_rate.lock().unwrap() = graph.tempo().sample_rate();
        self.send(Command::SetGraph(Box::new(graph)))
    }

    /// The disk thread's side, to run with [`Streamer::spawn`] or drive
    /// by hand with [`Streamer::service`]. Can be taken once.
    pub fn take_streamer(&mut self) -> Option<Streamer> {
        self.streamer.take()
    }

    /// The recording side, to run with [`Recorder::spawn`] or drive by
    /// hand with [`Recorder::service`]. Can be taken once.
    pub fn take_recorder(&mut self) -> Option<Recorder> {
        self.recorder.take()
    }

    /// The folder new takes are written to.
    pub fn set_record_dir(&mut self, dir: impl Into<std::path::PathBuf>) {
        *self.record_dir.lock().unwrap() = dir.into();
    }

    /// Takes finished since the last call.
    pub fn finished_takes(&mut self) -> Vec<Take> {
        std::mem::take(&mut *self.takes.lock().unwrap())
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
    from_disk: Consumer<ToAudio>,
    to_disk: Producer<Box<Chunk>>,
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
    recording: bool,
    to_recorder: Producer<RecBlock>,
}

/// Commands that fit in the queue before `send` refuses more.
const QUEUE: usize = 1024;
/// Chunk messages between the disk thread and the audio thread.
const DISK_QUEUE: usize = 4096;
/// Recorded blocks in flight: about 20 seconds of one stereo track at
/// 48 kHz, shared between armed tracks, before input is lost.
const REC_QUEUE: usize = 8192;

/// Creates a connected controller and engine. `max_block` is the largest
/// block the audio callback will ask for; larger ones are split.
pub fn new(max_block: usize) -> (Controller, Engine) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(QUEUE);
    // Room for every graph that could be in flight at once.
    let (ret_tx, ret_rx) = RingBuffer::new(QUEUE + 1);
    let status = Arc::new(Status::new());
    let max_block = max_block.max(1);
    let (disk_tx, disk_rx) = RingBuffer::new(DISK_QUEUE);
    let (back_tx, back_rx) = RingBuffer::new(DISK_QUEUE * 2);
    let placements = Arc::new(Mutex::new(Vec::new()));
    let streamer = Streamer::new(disk_tx, back_rx, status.clone(), placements.clone());
    let (rec_tx, rec_rx) = RingBuffer::new(REC_QUEUE);
    let record_dir = Arc::new(Mutex::new(std::env::temp_dir().join("daw-takes")));
    let record_rate = Arc::new(Mutex::new(48_000));
    let takes = Arc::new(Mutex::new(Vec::new()));
    let recorder = Recorder::new(
        rec_rx,
        status.clone(),
        record_dir.clone(),
        record_rate.clone(),
        takes.clone(),
    );
    (
        Controller {
            commands: cmd_tx,
            retired: ret_rx,
            placements,
            streamer: Some(streamer),
            recorder: Some(recorder),
            record_dir,
            record_rate,
            takes,
            status: status.clone(),
            max_block,
        },
        Engine {
            commands: cmd_rx,
            retired: ret_tx,
            from_disk: disk_rx,
            to_disk: back_tx,
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
            recording: false,
            to_recorder: rec_tx,
        },
    )
}

impl Engine {
    /// Takes in chunks from the disk thread, and gives back the ones it
    /// asks for.
    fn apply_disk(&mut self) {
        while let Ok(msg) = self.from_disk.pop() {
            match msg {
                ToAudio::Loaded(c) => {
                    let rejected = match &mut self.graph {
                        Some(g) => g.insert_chunk(c),
                        None => Some(c),
                    };
                    if let Some(c) = rejected {
                        give_back(&mut self.to_disk, c);
                    }
                }
                ToAudio::Drop { source, index } => {
                    if let Some(c) = self
                        .graph
                        .as_mut()
                        .and_then(|g| g.take_chunk(source, index))
                    {
                        give_back(&mut self.to_disk, c);
                    }
                }
            }
        }
    }

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
                Command::SetGraph(mut g) => {
                    if let Some(old) = &mut self.graph {
                        let to_disk = &mut self.to_disk;
                        g.adopt_chunks(old, |c| give_back(to_disk, c));
                        g.adopt_inserts(old);
                    }
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
                Command::SetRecording(on) => self.recording = on,
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
        click: Option<&mut [f32]>,
    ) {
        self.process_duplex(&[], 0, left, right, click);
    }

    /// Like [`process_split`](Self::process_split), with this block's
    /// device input: `input` holds `channels` interleaved channels for the
    /// same frames as the outputs (missing frames are silence).
    pub fn process_duplex(
        &mut self,
        input: &[f32],
        channels: usize,
        left: &mut [f32],
        right: &mut [f32],
        mut click: Option<&mut [f32]>,
    ) {
        debug_assert_eq!(left.len(), right.len());
        self.apply_commands();
        self.apply_disk();
        left.fill(0.0);
        right.fill(0.0);
        if let Some(c) = click.as_deref_mut() {
            c.fill(0.0);
        }
        let n = left.len().min(right.len());
        let inp = |range: std::ops::Range<usize>| InputBlock {
            data: input
                .get(range.start * channels..(range.end * channels).min(input.len()))
                .unwrap_or(&[]),
            channels,
        };
        self.input_meters(inp(0..n));
        if !self.playing {
            // Stopped: monitored inputs still sound.
            let mut done = 0;
            while done < n {
                let r = done..(done + self.max_block).min(n);
                self.render(
                    inp(r.clone()),
                    &mut left[r.clone()],
                    &mut right[r.clone()],
                    false,
                );
                done = r.end;
            }
        }
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
            let block = inp(range.clone());
            self.render(
                block,
                &mut left[range.clone()],
                &mut right[range.clone()],
                !counting,
            );
            if !counting && self.recording {
                self.record(block, len);
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
        if let Some((a, b)) = self.looping {
            self.status.loop_start.store(a, Ordering::Relaxed);
            self.status.loop_end.store(b, Ordering::Relaxed);
        }
        self.status
            .loop_on
            .store(self.looping.is_some(), Ordering::Relaxed);
        self.status
            .counting_in
            .store(self.count_in_to.is_some(), Ordering::Relaxed);
        self.status
            .recording
            .store(self.recording, Ordering::Relaxed);
    }

    /// Sends `len` frames of each armed track's input, from `pos`, to the
    /// recording thread.
    fn record(&mut self, input: InputBlock, len: usize) {
        let Some(g) = &self.graph else { return };
        for (ti, t) in g.tracks.iter().enumerate() {
            let Some(sel) = t.input.filter(|_| t.arm) else {
                continue;
            };
            let ch = sel.channels();
            let mut done = 0;
            while done < len {
                let run = (len - done).min(BLOCK_FRAMES);
                let mut b = RecBlock::EMPTY;
                b.track = ti as u16;
                b.channels = ch as u8;
                b.len = run as u16;
                b.at = self.pos + done as i64;
                for i in 0..run {
                    for c in 0..ch {
                        b.samples[i * ch + c] = input.get(done + i, sel.first + c);
                    }
                }
                if self.to_recorder.push(b).is_err() {
                    self.status
                        .rec_dropped
                        .fetch_add(run as u64, Ordering::Relaxed);
                }
                done += run;
            }
        }
    }

    fn input_meters(&mut self, input: InputBlock) {
        let Some(g) = &self.graph else { return };
        if input.channels == 0 {
            return;
        }
        let frames = input.data.len() / input.channels;
        for (ti, t) in g.tracks.iter().enumerate() {
            let Some(sel) = t.input else { continue };
            let right = if sel.stereo { sel.first + 1 } else { sel.first };
            let mut p = (0.0f32, 0.0f32);
            for i in 0..frames {
                p.0 = p.0.max(input.get(i, sel.first).abs());
                p.1 = p.1.max(input.get(i, right).abs());
            }
            raise(&self.status.input_peaks[ti][0], p.0);
            raise(&self.status.input_peaks[ti][1], p.1);
        }
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

    fn render(&mut self, input: InputBlock, left: &mut [f32], right: &mut [f32], play: bool) {
        let Some(g) = &mut self.graph else {
            return;
        };
        let tracks = g.track_count();
        let missing = g.render(
            self.pos,
            [left, right],
            &mut self.peaks[..tracks],
            input,
            play,
        );
        if missing > 0 {
            self.status
                .underruns
                .fetch_add(missing as u64, Ordering::Relaxed);
        }
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
/// from `from` for `frames` frames. Used for export and tests. Streamed
/// files are read before each block, so nothing is missing.
pub fn render_offline(graph: Graph, from: Frames, frames: usize, block: usize) -> [Vec<f32>; 2] {
    let (mut ctl, mut eng) = new(block);
    let mut disk = ctl
        .take_streamer()
        .expect("a new controller has its streamer");
    let _ = ctl.set_graph(graph);
    let _ = ctl.send(Command::Seek(from));
    // Apply the commands so the disk thread sees the start position.
    eng.process(&mut [], &mut []);
    let _ = ctl.send(Command::Play);
    let mut out = [vec![0.0; frames], vec![0.0; frames]];
    let [l, r] = &mut out;
    for (bl, br) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        disk.service();
        eng.process(bl, br);
    }
    out
}
