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
//! play position, then playback starts on the exact frame. The count-in
//! keeps the tempo and time signature in force at the play position, so
//! starting just after a tempo change counts in at the new tempo.
//!
//! With device input ([`Engine::process_duplex`]), tracks can monitor
//! their input at any time, and armed tracks record while the transport
//! records and plays (not during the count-in); see [`crate::record`].
//! Every armed track records at once, each from its own inputs. A punch
//! range limits recording to part of the timeline, and playback can stop
//! by itself at a frame (post-roll), both sample-accurate.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::graph::{Graph, GroupSpec, InputBlock, Level, MAX_TRACKS, Mix, Rate};
use crate::meter::Tap;
use crate::metronome::{self, Metronome, Voice};
use crate::monitor::{self, ControlRoom, CueBus, CueSend, Monitor, OutPair};
use crate::record::{BLOCK_SAMPLES, CaptureShared, Kind, RecBlock, Recorder, Take};
use crate::stream::{Chunk, Placement, Playhead, Streamer, ToAudio};
use crate::stretch;
use crate::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

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
    /// A control group's fader and mute, without a new graph.
    SetGroup {
        group: usize,
        spec: GroupSpec,
    },
    /// A send's level and mute, without a new graph.
    SetSend {
        track: usize,
        send: usize,
        gain_db: f32,
        mute: bool,
    },
    SetMetronome(Metronome),
    /// Records armed tracks whenever playing (after any count-in).
    SetRecording(bool),
    /// Records only input captured in `[start, end)`; `None` records
    /// everything. Capture time is the timeline frame the input arrived
    /// at, so a range meant for the timeline is sent later by the
    /// interface's round trip (see [`Take::clip_start`]).
    SetPunch(Option<(Frames, Frames)>),
    /// Stops playback on reaching this frame (not during a count-in);
    /// cleared when reached or on [`Command::Stop`].
    StopAt(Option<Frames>),
    /// Sends armed tracks' input that is not being recorded to the
    /// recorder for retrospective capture. Use
    /// [`Controller::set_capture`], which also tells the recorder.
    SetCapture(bool),
    /// Sends this input channel to the tuner; `None` stops.
    SetTunerInput(Option<usize>),
    /// Sends this bus to the meters; `None` stops.
    SetMeterTap(Option<Tap>),
    /// Practice speed (1 is normal, 0.5 to 1.5) and transpose in
    /// semitones (±12, fractions for cents), keeping pitch and tempo
    /// apart. Nothing records while either is changed.
    SetPractice {
        speed: f64,
        semitones: f64,
    },
    /// One headphone mix's own settings (see [`crate::monitor`]).
    SetCueBus {
        cue: usize,
        bus: CueBus,
    },
    /// One track's level in one headphone mix, without a new graph.
    SetCueSend {
        track: usize,
        cue: usize,
        send: CueSend,
    },
    /// The speakers' controls and talkback.
    SetControlRoom(ControlRoom),
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
    /// The same for input kept by retrospective capture.
    capture_dropped: AtomicU64,
    /// RMS per track (left, right) over about 300 ms, after the fader,
    /// as `f32` bits; then the master's.
    rms: Vec<[AtomicU32; 2]>,
    /// Frames of tapped audio the meters missed because the UI fell
    /// behind.
    meter_dropped: AtomicU64,
    /// Times playback has gone round the loop.
    loops: AtomicU64,
    /// The headphone mixes' peaks.
    pub monitor: monitor::Meters,
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
            capture_dropped: AtomicU64::new(0),
            rms: (0..=MAX_TRACKS)
                .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
                .collect(),
            meter_dropped: AtomicU64::new(0),
            loops: AtomicU64::new(0),
            monitor: monitor::Meters::new(),
        }
    }

    /// True while set to record, including when stopped and waiting to.
    pub fn recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Frames of recorded input lost because the disk fell behind.
    /// Times playback has gone round the loop since the engine started.
    pub fn loops(&self) -> u64 {
        self.loops.load(Ordering::Relaxed)
    }

    pub fn rec_dropped(&self) -> u64 {
        self.rec_dropped.load(Ordering::Relaxed)
    }

    /// Frames of input lost to retrospective capture because the
    /// recording thread fell behind; recorded takes are unaffected.
    pub fn capture_dropped(&self) -> u64 {
        self.capture_dropped.load(Ordering::Relaxed)
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

    /// RMS (left, right) on a track over about the last 300 ms.
    pub fn track_rms(&self, track: usize) -> (f32, f32) {
        self.rms
            .get(track.min(MAX_TRACKS - 1))
            .map_or((0.0, 0.0), read_pair)
    }

    /// RMS (left, right) on the master bus over about the last 300 ms.
    pub fn master_rms(&self) -> (f32, f32) {
        read_pair(&self.rms[MAX_TRACKS])
    }

    /// Frames of tapped audio the meters have missed so far.
    pub fn meter_dropped(&self) -> u64 {
        self.meter_dropped.load(Ordering::Relaxed)
    }
}

fn read_pair(p: &[AtomicU32; 2]) -> (f32, f32) {
    (
        f32::from_bits(p[0].load(Ordering::Relaxed)),
        f32::from_bits(p[1].load(Ordering::Relaxed)),
    )
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
    capture: Arc<CaptureShared>,
    status: Arc<Status>,
    max_block: usize,
    from_tuner: Consumer<f32>,
    from_meter: Consumer<f32>,
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

    /// Keeps the last `seconds` of armed tracks' input that was not
    /// recorded, for [`keep_capture`](Self::keep_capture); 0 turns
    /// retrospective capture off and forgets what was kept.
    pub fn set_capture(&mut self, seconds: u32) -> Result<(), Command> {
        self.capture.seconds.store(seconds, Ordering::Relaxed);
        self.send(Command::SetCapture(seconds > 0))
    }

    /// Asks the recorder to write what retrospective capture holds as
    /// takes; they arrive through [`finished_takes`](Self::finished_takes).
    pub fn keep_capture(&self) {
        self.capture.keep.store(true, Ordering::Relaxed);
    }

    /// Frames of input retrospective capture holds, on the track with
    /// the most.
    pub fn captured_frames(&self) -> usize {
        self.capture.held.load(Ordering::Relaxed) as usize
    }

    /// Takes finished since the last call.
    pub fn finished_takes(&mut self) -> Vec<Take> {
        std::mem::take(&mut *self.takes.lock().unwrap())
    }

    /// Moves input samples sent to the tuner (see
    /// [`Command::SetTunerInput`]) onto the end of `into`, keeping at
    /// most `keep` of the newest.
    pub fn take_tuner_samples(&mut self, into: &mut Vec<f32>, keep: usize) {
        let n = self.from_tuner.slots();
        if let Ok(chunk) = self.from_tuner.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            into.extend_from_slice(a);
            into.extend_from_slice(b);
            chunk.commit_all();
        }
        if into.len() > keep {
            into.drain(..into.len() - keep);
        }
    }

    /// Moves the tapped bus's audio (see [`Command::SetMeterTap`]),
    /// interleaved stereo, onto the end of `into`.
    pub fn take_meter_samples(&mut self, into: &mut Vec<f32>) {
        let n = self.from_meter.slots() & !1;
        if let Ok(chunk) = self.from_meter.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            into.extend_from_slice(a);
            into.extend_from_slice(b);
            chunk.commit_all();
        }
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
    /// The count-in's clicks: one steady tempo and time signature, made
    /// up front so a count-in never allocates. Timeline frame `f` is
    /// frame `f - count_offset` on it.
    count_map: TempoMap,
    count_offset: i64,
    /// Clicks for the current block, preallocated.
    click_buf: Vec<f32>,
    /// Per-block track levels, preallocated.
    levels: Vec<Level>,
    /// Mean squares behind [`Status::track_rms`], then the master's.
    mean_sq: Vec<(f32, f32)>,
    meter_tap: Option<Tap>,
    /// A tapped track's block, preallocated.
    tap_buf: [Vec<f32>; 2],
    to_meter: Producer<f32>,
    recording: bool,
    /// See [`Command::SetPunch`].
    punch: Option<(i64, i64)>,
    stop_at: Option<i64>,
    capture: bool,
    to_recorder: Producer<RecBlock>,
    tuner_input: Option<usize>,
    to_tuner: Producer<f32>,
    speed: f64,
    semitones: f64,
    /// While stretching: the play position, fractional, and the same in
    /// unwrapped play time (not sent back round the loop).
    posf: f64,
    u: f64,
    /// The stretchers must start afresh at `u`.
    stretch_reset: bool,
    /// Headphone mixes, talkback and the speakers' controls.
    monitor: Monitor,
}

/// Commands that fit in the queue before `send` refuses more.
const QUEUE: usize = 1024;
/// Chunk messages between the disk thread and the audio thread.
const DISK_QUEUE: usize = 4096;
/// Recorded blocks in flight, shared between armed tracks, before input
/// is lost: about 85 seconds of one stereo track at 48 kHz, or 10 seconds
/// each of 16 mono tracks. Preallocated, about 34 MB.
const REC_QUEUE: usize = 32_768;
/// Tuner samples in flight: about a third of a second at 48 kHz, more
/// than the UI needs between frames.
const TUNER_QUEUE: usize = 16_384;
/// Tapped samples in flight, stereo: over two seconds at 48 kHz, so the
/// meters miss nothing while the UI is slow to draw.
const METER_QUEUE: usize = 1 << 18;
/// How long the RMS meters average over, in seconds.
const RMS_TIME: f32 = 0.3;

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
    let capture = Arc::new(CaptureShared::default());
    let (tuner_tx, tuner_rx) = RingBuffer::new(TUNER_QUEUE);
    let (meter_tx, meter_rx) = RingBuffer::new(METER_QUEUE);
    let recorder = Recorder::new(
        rec_rx,
        status.clone(),
        record_dir.clone(),
        record_rate.clone(),
        takes.clone(),
        capture.clone(),
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
            capture,
            status: status.clone(),
            max_block,
            from_tuner: tuner_rx,
            from_meter: meter_rx,
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
            count_map: TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON),
            count_offset: 0,
            click_buf: vec![0.0; max_block],
            levels: vec![Level::default(); MAX_TRACKS],
            mean_sq: vec![(0.0, 0.0); MAX_TRACKS + 1],
            meter_tap: None,
            tap_buf: [vec![0.0; max_block], vec![0.0; max_block]],
            to_meter: meter_tx,
            recording: false,
            punch: None,
            stop_at: None,
            capture: false,
            to_recorder: rec_tx,
            tuner_input: None,
            to_tuner: tuner_tx,
            speed: 1.0,
            semitones: 0.0,
            posf: 0.0,
            u: 0.0,
            stretch_reset: true,
            monitor: Monitor::new(max_block),
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
                        let sig = map.signature_at(start);
                        // Steady at the play position's tempo, with its
                        // bar lines where the song's are.
                        let into_bar = start.0 - map.bar_start(map.bar_beat(start).bar).0;
                        let c = &mut self.count_map;
                        c.reset(map.sample_rate(), map.tempo_at(start), sig);
                        self.count_offset = self.pos - c.frames_at(Ticks(into_bar)).0;
                        let back = Ticks(into_bar - sig.bar_ticks() * bars as i64);
                        let from = Frames(c.frames_at(back).0 + self.count_offset);
                        if from.0 < self.pos {
                            self.count_in_to = Some(self.pos);
                            self.pos = from.0;
                        }
                    }
                    self.playing = true;
                }
                Command::Stop => {
                    self.playing = false;
                    self.stop_at = None;
                    if let Some(to) = self.count_in_to.take() {
                        self.pos = to;
                    }
                    self.voice = None;
                }
                Command::Seek(f) => {
                    self.pos = f.0;
                    self.stretch_reset = true;
                    self.count_in_to = None;
                    self.voice = None;
                }
                Command::SetLoop(l) => {
                    self.looping = l.filter(|(a, b)| b > a).map(|(a, b)| (a.0, b.0));
                    self.stretch_reset = true;
                }
                Command::SetGraph(mut g) => {
                    if let Some(old) = &mut self.graph {
                        let to_disk = &mut self.to_disk;
                        g.adopt_chunks(old, |c| give_back(to_disk, c));
                        g.adopt_inserts(old);
                        g.adopt_stretchers(old);
                        g.adopt_edges(old);
                        g.adopt_cue_lines(old);
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
                Command::SetGroup { group, spec } => {
                    if let Some(g) = &mut self.graph {
                        g.set_group(group, spec);
                    }
                }
                Command::SetSend {
                    track,
                    send,
                    gain_db,
                    mute,
                } => {
                    if let Some(g) = &mut self.graph {
                        g.set_send(track, send, gain_db, mute);
                    }
                }
                Command::SetMetronome(m) => self.metronome = m,
                Command::SetRecording(on) => self.recording = on,
                Command::SetPunch(p) => {
                    self.punch = p.filter(|(a, b)| b > a).map(|(a, b)| (a.0, b.0));
                }
                Command::StopAt(f) => self.stop_at = f.map(|f| f.0),
                Command::SetCapture(on) => self.capture = on,
                Command::SetTunerInput(ch) => self.tuner_input = ch,
                Command::SetMeterTap(t) => self.meter_tap = t,
                Command::SetPractice { speed, semitones } => {
                    self.speed = speed.clamp(stretch::SPEED_RANGE.0, stretch::SPEED_RANGE.1);
                    self.semitones = semitones.clamp(-stretch::SEMITONES, stretch::SEMITONES);
                }
                Command::SetCueBus { cue, bus } => {
                    if let Some(b) = self.monitor.buses.get_mut(cue) {
                        *b = bus;
                    }
                }
                Command::SetCueSend { track, cue, send } => {
                    if let Some(g) = &mut self.graph {
                        g.set_cue(track, cue, send);
                    }
                }
                Command::SetControlRoom(room) => self.monitor.room = room,
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
        self.monitor.begin(n);
        if let Some(ch) = self.tuner_input.filter(|&c| c < channels) {
            // Whatever doesn't fit is dropped: the tuner only wants the
            // newest audio.
            let frames = (input.len() / channels).min(n);
            let take = frames.min(self.to_tuner.slots());
            if let Ok(mut chunk) = self.to_tuner.write_chunk(take) {
                let (a, b) = chunk.as_mut_slices();
                for (k, slot) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                    *slot = input[(frames - take + k) * channels + ch];
                }
                chunk.commit_all();
            }
        }
        let inp = |range: std::ops::Range<usize>| InputBlock {
            data: input
                .get(range.start * channels..(range.end * channels).min(input.len()))
                .unwrap_or(&[]),
            channels,
        };
        self.input_meters(inp(0..n));
        if !self.playing {
            self.render_stopped(input, channels, left, right, 0);
        }
        if !self.playing || !self.stretching() {
            self.stretch_reset = true;
        }
        if self.playing && self.stretching() {
            self.play_stretched(input, channels, left, right, click);
            self.monitor
                .finish(input, channels, left, right, &self.status.monitor);
            self.publish();
            return;
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
            if let Some(stop) = self.stop_at.filter(|&s| s > self.pos && !counting) {
                len = len.min((stop - self.pos) as usize);
            }
            let range = done..done + len;
            let block = inp(range.clone());
            self.render(
                block,
                range.start,
                &mut left[range.clone()],
                &mut right[range.clone()],
                !counting,
                None,
            );
            self.record(block, len, counting);
            if counting || self.metronome.enabled {
                self.clicks(len);
                let clicks = &self.click_buf[..len];
                self.monitor.add_click(range.start, clicks);
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
            } else if !counting && self.stop_at == Some(self.pos) {
                // Post-roll done: the rest of the block is as if stopped.
                self.playing = false;
                self.stop_at = None;
                self.voice = None;
                self.render_stopped(input, channels, left, right, done);
            } else if let Some((start, end)) = self.looping
                && !counting
                && self.pos == end
            {
                self.pos = start;
                self.status.loops.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.monitor
            .finish(input, channels, left, right, &self.status.monitor);
        self.publish();
    }

    /// The device outputs the main pair should play on for the block
    /// just made, when the speakers chosen in the monitor section are
    /// not the device's main outputs.
    pub fn main_out(&self) -> Option<OutPair> {
        self.monitor.main_out()
    }

    /// Calls `f` with each device output channel a headphone mix plays
    /// on, and its samples for the block just made.
    pub fn each_cue_out(&self, f: &mut dyn FnMut(usize, &[f32])) {
        self.monitor.each_cue_out(f);
    }

    /// Renders frames `from..` of the block while stopped: monitored
    /// inputs still sound.
    fn render_stopped(
        &mut self,
        input: &[f32],
        channels: usize,
        left: &mut [f32],
        right: &mut [f32],
        from: usize,
    ) {
        let n = left.len().min(right.len());
        let mut done = from;
        while done < n {
            let r = done..(done + self.max_block).min(n);
            let block = InputBlock {
                data: input
                    .get(r.start * channels..(r.end * channels).min(input.len()))
                    .unwrap_or(&[]),
                channels,
            };
            self.render(
                block,
                r.start,
                &mut left[r.clone()],
                &mut right[r.clone()],
                false,
                None,
            );
            done = r.end;
        }
    }

    /// Sends the `len` frames of armed tracks' input played from `pos`
    /// to the recording thread: what is inside the punch range (all of
    /// it without one) to be recorded, unless counting in, and the rest
    /// for retrospective capture if that is on.
    fn record(&mut self, input: InputBlock, len: usize, counting: bool) {
        let (pos, end) = (self.pos, self.pos + len as i64);
        let (lo, hi) = match self.punch {
            _ if counting || !self.recording => (end, end),
            Some((a, b)) => (a.clamp(pos, end), b.clamp(pos, end).max(a.clamp(pos, end))),
            None => (pos, end),
        };
        if self.capture {
            self.send_input(input, 0, (lo - pos) as usize, pos, Kind::Capture);
        }
        self.send_input(
            input,
            (lo - pos) as usize,
            (hi - lo) as usize,
            lo,
            Kind::Record,
        );
        if let Some((_, out)) = self.punch
            && self.recording
            && !counting
            && pos < out
            && out <= end
        {
            // Punched out: the takes end here, not when playback stops.
            self.send_input(input, 0, 0, out, Kind::End);
        }
        if self.capture {
            self.send_input(
                input,
                (hi - pos) as usize,
                (end - hi) as usize,
                hi,
                Kind::Capture,
            );
        }
    }

    /// Sends `len` frames of each armed track's input, from frame `from`
    /// of the block, to the recording thread as `kind`, the first at
    /// timeline frame `at`. With `Kind::End`, sends one empty block per
    /// track.
    fn send_input(&mut self, input: InputBlock, from: usize, len: usize, at: i64, kind: Kind) {
        if len == 0 && kind != Kind::End {
            return;
        }
        let Some(g) = &self.graph else { return };
        for (ti, t) in g.tracks.iter().enumerate() {
            let Some(sel) = t.input.filter(|_| t.arm) else {
                continue;
            };
            let ch = sel.channels();
            let mut done = 0;
            loop {
                let run = (len - done).min(BLOCK_SAMPLES / ch);
                let mut b = RecBlock::EMPTY;
                b.track = ti as u16;
                b.channels = ch as u8;
                b.len = run as u16;
                b.kind = kind;
                b.at = at + done as i64;
                for i in 0..run {
                    for c in 0..ch {
                        b.samples[i * ch + c] = input.get(from + done + i, sel.first + c);
                    }
                }
                if self.to_recorder.push(b).is_err() {
                    let lost = match kind {
                        Kind::Record | Kind::End => &self.status.rec_dropped,
                        Kind::Capture => &self.status.capture_dropped,
                    };
                    lost.fetch_add(run as u64, Ordering::Relaxed);
                }
                done += run;
                if done >= len {
                    break;
                }
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
    fn stretching(&self) -> bool {
        self.speed != 1.0 || self.semitones != 0.0
    }

    /// Shares the transport state with the UI.
    fn publish(&self) {
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

    /// Plays one callback's block at the practice speed and transpose.
    /// Output frame `i` plays timeline position `posf + i × speed`; tracks
    /// read through their stretchers, clicks follow the slowed beat, and
    /// nothing records.
    fn play_stretched(
        &mut self,
        input: &[f32],
        channels: usize,
        left: &mut [f32],
        right: &mut [f32],
        mut click: Option<&mut [f32]>,
    ) {
        let n = left.len().min(right.len());
        let speed = self.speed;
        if self.stretch_reset {
            self.posf = self.pos as f64;
            self.u = self.posf;
        }
        let mut done = 0;
        while self.playing && done < n {
            let mut len = (n - done).min(self.max_block);
            let counting = self.count_in_to.is_some();
            // Output frames until `to` on the timeline, at least one.
            let until = |to: i64, at: f64| (((to as f64 - at) / speed).ceil() as usize).max(1);
            if let Some(to) = self.count_in_to {
                len = len.min(until(to, self.posf));
            } else if let Some((start, end)) = self.looping {
                if self.posf >= end as f64 {
                    self.posf = start as f64;
                    self.u = self.posf;
                    self.stretch_reset = true;
                }
                len = len.min(until(end, self.posf));
            }
            let range = done..done + len;
            let block = InputBlock {
                data: input
                    .get(range.start * channels..(range.end * channels).min(input.len()))
                    .unwrap_or(&[]),
                channels,
            };
            let rate = (!counting).then(|| Rate {
                u: self.u,
                speed,
                semitones: self.semitones,
                reset: std::mem::take(&mut self.stretch_reset),
            });
            self.render(
                block,
                range.start,
                &mut left[range.clone()],
                &mut right[range.clone()],
                !counting,
                rate,
            );
            if counting || self.metronome.enabled {
                let buf = &mut self.click_buf[..len];
                buf.fill(0.0);
                if let Some(g) = &self.graph {
                    let (map, from) = match self.count_in_to {
                        Some(_) => (&self.count_map, self.posf - self.count_offset as f64),
                        None => (g.tempo(), self.posf),
                    };
                    metronome::render_at(
                        map,
                        &g.clicks,
                        &self.metronome,
                        &mut self.voice,
                        from,
                        speed,
                        buf,
                    );
                }
                let clicks = &self.click_buf[..len];
                self.monitor.add_click(range.start, clicks);
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
            let step = len as f64 * speed;
            self.posf += step;
            self.u += step;
            done += len;
            if let Some(to) = self.count_in_to
                && self.posf >= to as f64
            {
                self.posf = to as f64;
                self.u = self.posf;
                self.count_in_to = None;
                self.stretch_reset = true;
            } else if let Some((start, end)) = self.looping
                && !counting
                && self.posf >= end as f64
            {
                // The stretchers carry on round the loop by themselves.
                self.posf = start as f64 + (self.posf - end as f64);
                self.status.loops.fetch_add(1, Ordering::Relaxed);
            }
            self.pos = self.posf.floor() as i64;
        }
    }

    fn clicks(&mut self, len: usize) {
        let buf = &mut self.click_buf[..len];
        buf.fill(0.0);
        if let Some(g) = &self.graph {
            let (map, from) = match self.count_in_to {
                Some(_) => (&self.count_map, self.pos - self.count_offset),
                None => (g.tempo(), self.pos),
            };
            metronome::render(map, &g.clicks, &self.metronome, &mut self.voice, from, buf);
        }
    }

    /// Renders one block, `at` frames into the callback's.
    fn render(
        &mut self,
        input: InputBlock,
        at: usize,
        left: &mut [f32],
        right: &mut [f32],
        play: bool,
        rate: Option<Rate>,
    ) {
        let Some(g) = &mut self.graph else {
            return;
        };
        let tracks = g.track_count();
        let n = left.len();
        let [tl, tr] = &mut self.tap_buf;
        let tap = match self.meter_tap {
            Some(Tap::Track(t)) if t < tracks => Some((t, &mut tl[..n], &mut tr[..n])),
            _ => None,
        };
        let missing = g.render(
            self.pos,
            self.looping,
            [left, right],
            &mut self.levels[..tracks],
            input,
            play,
            rate,
            tap,
        );
        self.monitor.add_tracks(g, at, n);
        if missing > 0 {
            self.status
                .underruns
                .fetch_add(missing as u64, Ordering::Relaxed);
        }
        // Each block moves the RMS this far towards the block's own.
        let k = 1.0 - (-(n as f32) / (RMS_TIME * g.tempo().sample_rate() as f32)).exp();
        let per = 1.0 / n.max(1) as f32;
        let status = &self.status;
        let rms = |i: usize, sq: (f32, f32), ms: &mut (f32, f32)| {
            ms.0 += k * (sq.0 * per - ms.0);
            ms.1 += k * (sq.1 * per - ms.1);
            status.rms[i][0].store(ms.0.max(0.0).sqrt().to_bits(), Ordering::Relaxed);
            status.rms[i][1].store(ms.1.max(0.0).sqrt().to_bits(), Ordering::Relaxed);
        };
        for (i, l) in self.levels[..tracks].iter().enumerate() {
            raise(&status.track_peaks[i][0], l.peak.0);
            raise(&status.track_peaks[i][1], l.peak.1);
            rms(i, l.sq, &mut self.mean_sq[i]);
        }
        let (mut ml, mut mr, mut sl, mut sr) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (a, b) in left.iter().zip(right.iter()) {
            (ml, mr) = (ml.max(a.abs()), mr.max(b.abs()));
            (sl, sr) = (sl + a * a, sr + b * b);
        }
        raise(&status.master_peaks[0], ml);
        raise(&status.master_peaks[1], mr);
        rms(MAX_TRACKS, (sl, sr), &mut self.mean_sq[MAX_TRACKS]);
        let tapped = match self.meter_tap {
            Some(Tap::Master) => Some((&*left, &*right)),
            Some(Tap::Track(t)) if t < tracks => Some((&tl[..n], &tr[..n])),
            _ => None,
        };
        if let Some((l, r)) = tapped {
            // Whatever doesn't fit is dropped, and counted.
            let frames = n.min(self.to_meter.slots() / 2);
            if let Ok(mut chunk) = self.to_meter.write_chunk(2 * frames) {
                let (a, b) = chunk.as_mut_slices();
                for (k, slot) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                    *slot = if k % 2 == 0 { l[k / 2] } else { r[k / 2] };
                }
                chunk.commit_all();
            }
            if frames < n {
                status
                    .meter_dropped
                    .fetch_add((n - frames) as u64, Ordering::Relaxed);
            }
        }
    }
}

/// Renders a graph offline, through the same code as real-time playback,
/// from `from` for `frames` frames. Used for tests; export streams with
/// [`render_with`]. Streamed files are read before each block, so nothing
/// is missing.
pub fn render_offline(graph: Graph, from: Frames, frames: usize, block: usize) -> [Vec<f32>; 2] {
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    render_with(graph, from, frames, block, |l, r| {
        out[0].extend_from_slice(l);
        out[1].extend_from_slice(r);
        true
    });
    out
}

/// Renders as [`render_offline`] does, handing each block to `sink`
/// instead of keeping it. `sink` returns false to stop early. Returns the
/// frames rendered.
pub fn render_with(
    graph: Graph,
    from: Frames,
    frames: usize,
    block: usize,
    mut sink: impl FnMut(&[f32], &[f32]) -> bool,
) -> usize {
    let block = block.max(1);
    let (mut ctl, mut eng) = new(block);
    let mut disk = ctl
        .take_streamer()
        .expect("a new controller has its streamer");
    let _ = ctl.set_graph(graph);
    let _ = ctl.send(Command::Seek(from));
    // Apply the commands so the disk thread sees the start position.
    eng.process(&mut [], &mut []);
    let _ = ctl.send(Command::Play);
    let (mut l, mut r) = (vec![0.0; block], vec![0.0; block]);
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        disk.service();
        eng.process(&mut l[..n], &mut r[..n]);
        done += n;
        if !sink(&l[..n], &r[..n]) {
            break;
        }
    }
    done
}
