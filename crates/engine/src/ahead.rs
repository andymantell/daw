// SPDX-License-Identifier: GPL-3.0-or-later
//! Low-latency monitoring with a big mix buffer.
//!
//! Playing through an amp wants a small device buffer, so the guitar
//! comes back from the speakers quickly; a heavy mix wants a big one, so
//! its effects have time to run. [`split`] gives both. The engine is
//! split in two:
//!
//! - A worker thread runs the transport and every track that has no live
//!   input, in big blocks ([`Worker`]). It runs ahead of the device by
//!   one or two of those blocks, so a slow block is absorbed rather than
//!   heard.
//! - The device callback runs the live tracks, in the device's own small
//!   blocks ([`Front`]). A live track is one that is armed, or monitors
//!   its input, or plays live notes; every track a live track feeds (its
//!   bus, a send's bus, the effect it is the sidechain of) is live too,
//!   so the live sound never waits for the worker, and so is the track
//!   any live effect listens to as a sidechain.
//!
//! The worker leads: everything it renders is handed over as segments,
//! each saying which timeline frames it is and how they play (stopped,
//! playing, counting in, at a practice speed), with the audio crossing
//! over: the worker's share of the master, the clicks, its share of each
//! headphone mix, and for each live track its clips' audio and whatever
//! the worker's tracks route into it. The callback plays the same frames
//! for the live tracks, adding live input, and sums the two. Both halves
//! play each frame at the same place on the timeline, through the same
//! delay compensation, so they stay in time; transport commands simply
//! reach the ears a block or two after they are given.
//!
//! Which tracks are live is worked out with each new graph
//! ([`Graph::with_twin`]). When a track becomes live, its effects and
//! instrument move to the callback with the next graph; when it stops
//! being live they come back, and until they have, the worker keeps the
//! track silent rather than playing it without them.
//!
//! If the worker falls behind, the callback keeps the live tracks
//! playing and the rest of the mix pauses until it catches up, counted
//! in [`AheadMeters::late`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::thread::{JoinHandle, Thread};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::engine::{Command, Engine, Side};
use crate::graph::{Carry, Graph, InputBlock, MAX_TRACKS, Rate, Session};
use crate::meter::Tap;
use crate::midi::OutEvent;
use crate::monitor::MAX_CUES;

/// The largest mix buffer the worker runs at, in frames.
pub const MAX_MIX_BLOCK: usize = 4096;

/// The mix buffers offered, in frames.
pub const MIX_BLOCKS: [u32; 4] = [512, 1024, 2048, 4096];

/// Where a track plays, on one side of a split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// On the worker, all of it.
    Run,
    /// On the worker, a live track: only its clips and what the worker's
    /// tracks route into it, handed to the callback as its feed.
    Feed,
    /// On the callback, a track the worker plays: nothing to do.
    Away,
    /// On the worker, a track no longer live whose effects are still on
    /// their way back from the callback: silent until they arrive.
    Hold,
    /// On the callback, a live track, from its feed.
    Live,
}

impl Mode {
    /// The track runs its effects and fader on this side.
    pub(crate) fn plays(self) -> bool {
        matches!(self, Mode::Run | Mode::Live)
    }
}

/// One side's view of a split graph.
pub(crate) struct Part {
    /// The worker's side (else the callback's).
    pub ahead: bool,
    pub mode: Vec<Mode>,
    /// The live tracks, in order.
    pub feeds: Vec<usize>,
    /// Per track, `max_block` long for live tracks (empty for others):
    /// the feed the worker makes and the callback plays.
    pub feed: Vec<[Vec<f32>; 2]>,
    /// The callback's side: the worker's share of the master, already
    /// through the master fader.
    pub master: [Vec<f32>; 2],
    /// The callback's side: live tracks that were live in the graph this
    /// one replaces, whose effects that graph holds.
    pub adopt: Vec<bool>,
}

impl Part {
    fn new(ahead: bool, live: &[bool], max_block: usize) -> Part {
        let (on, off) = if ahead {
            (Mode::Feed, Mode::Run)
        } else {
            (Mode::Live, Mode::Away)
        };
        let feeds: Vec<usize> = (0..live.len()).filter(|&i| live[i]).collect();
        Part {
            ahead,
            mode: live.iter().map(|&l| if l { on } else { off }).collect(),
            feed: live
                .iter()
                .map(|&l| {
                    let len = if l { max_block } else { 0 };
                    [vec![0.0; len], vec![0.0; len]]
                })
                .collect(),
            feeds,
            master: if ahead {
                [Vec::new(), Vec::new()]
            } else {
                [vec![0.0; max_block], vec![0.0; max_block]]
            },
            adopt: vec![false; live.len()],
        }
    }

    fn mode(&self, ti: usize) -> Mode {
        self.mode.get(ti).copied().unwrap_or(Mode::Away)
    }
}

impl Graph {
    /// Which tracks must play in the device callback when the mix is
    /// split (see [`crate::ahead`]): those armed, monitoring their input
    /// or playing live notes; every track they feed; and every track a
    /// live track's effect listens to as a sidechain.
    pub fn live_tracks(&self) -> Vec<bool> {
        let mut live: Vec<bool> = (self.tracks.iter())
            .map(|t| t.arm || (t.monitor && t.input.is_some()) || t.notes_live)
            .collect();
        loop {
            let mut changed = false;
            for e in &self.edges {
                let Some(to) = e.to else { continue };
                if live[e.from] && !live[to] {
                    live[to] = true;
                    changed = true;
                }
                if matches!(e.carry, Carry::Sidechain(_)) && live[to] && !live[e.from] {
                    live[e.from] = true;
                    changed = true;
                }
            }
            if !changed {
                return live;
            }
        }
    }

    /// Gives a graph the copy the callback plays when the mix is split
    /// (see [`crate::ahead`]): the same session compiled again, with no
    /// new effects or instruments handed in (they are moved across as
    /// needed). An engine that isn't split ignores it.
    ///
    /// # Panics
    ///
    /// If `twin` has a different number of tracks or routes.
    pub fn with_twin(mut self, twin: Graph) -> Graph {
        assert_eq!(self.tracks.len(), twin.tracks.len(), "not the same session");
        assert_eq!(self.edges.len(), twin.edges.len(), "not the same session");
        self.twin = Some(Box::new(twin));
        self
    }

    /// Splits the work between this graph (the worker's) and its twin
    /// (the callback's). Allocates; called before the graph is sent.
    pub(crate) fn prepare_split(&mut self) {
        if self.twin.is_none() {
            return;
        }
        let live = self.live_tracks();
        let Some(twin) = self.twin.as_mut() else {
            return;
        };
        twin.part = Some(Box::new(Part::new(false, &live, twin.max_block())));
        self.part = Some(Box::new(Part::new(true, &live, self.max_block())));
    }

    /// Whether any effect or instrument is still in this graph.
    fn holds_parts(&self) -> bool {
        self.tracks.iter().any(|t| {
            t.inserts.iter().any(|i| i.effect.is_some())
                || t.instrument.as_ref().is_some_and(|i| i.inst.is_some())
                || t.layers
                    .iter()
                    .any(|l| l.instrument.as_ref().is_some_and(|i| i.inst.is_some()))
        })
    }

    fn mode(&self, ti: usize) -> Mode {
        self.part.as_ref().map_or(Mode::Run, |p| p.mode(ti))
    }
}

/// How one run of frames plays, as the worker played it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seg {
    pub len: usize,
    /// The play position at its first frame.
    pub pos: i64,
    /// At a practice speed: the same, fractional.
    pub posf: f64,
    pub looping: Option<(i64, i64)>,
    /// Clips and notes play (not stopped or counting in).
    pub play: bool,
    pub rate: Option<Rate>,
    /// Armed tracks record from here, counting in or not.
    pub record: Option<bool>,
    /// Armed tracks record at the practice speed (see
    /// [`Engine::record_stretched`]), from `posf`.
    pub slow: bool,
    pub playing: bool,
    pub counting_in: bool,
    /// The transport went round the loop just before.
    pub wrapped: bool,
}

impl Seg {
    pub(crate) fn stopped(pos: i64, len: usize, looping: Option<(i64, i64)>) -> Seg {
        Seg {
            len,
            pos,
            posf: pos as f64,
            looping,
            play: false,
            rate: None,
            record: None,
            slow: false,
            playing: false,
            counting_in: false,
            wrapped: false,
        }
    }

    /// Moves on past `k` frames, played.
    fn advance(&mut self, k: usize) {
        self.len -= k;
        self.wrapped = false;
        match &mut self.rate {
            Some(r) => {
                let step = k as f64 * r.speed;
                r.u += step;
                r.reset = false;
                self.posf += step;
                self.pos = self.posf.floor() as i64;
            }
            None => {
                self.pos += k as i64;
                self.posf = self.pos as f64;
            }
        }
    }
}

/// What the worker sends the callback, in timeline order.
pub(crate) enum Msg {
    /// Frames to play; their audio is waiting in the ring.
    Seg(Seg),
    /// A command for the callback's side.
    Cmd(Command),
    /// From here on, this graph for the live tracks (or none) and, if
    /// the audio's layout changed, this ring with its channels per frame.
    Graph {
        graph: Option<Box<Graph>>,
        ring: Option<(Consumer<f32>, usize)>,
    },
    /// Effects or instruments for live tracks that came back too late:
    /// take what is missing, and send the rest back.
    Adopt(Box<Graph>),
}

/// What the callback sends back to be freed or taken over off the audio
/// thread.
pub(crate) enum Back {
    /// The graph the callback's side played until the last
    /// [`Msg::Graph`], with the effects of tracks no longer live.
    Graph(Option<Box<Graph>>),
    /// Done with: free it.
    Spent(Box<Graph>),
    Ring(Consumer<f32>),
}

/// Channels of audio per frame before the live tracks' feeds: the
/// master pair, the click, each headphone mix's pair, the tapped track's
/// pair.
const BASE: usize = 3 + 2 * MAX_CUES + 2;
const CUES_AT: usize = 3;
const TAP_AT: usize = 3 + 2 * MAX_CUES;

fn channels(feeds: usize) -> usize {
    BASE + 2 * feeds
}

/// How the split is doing, shared with the UI.
#[derive(Default)]
pub struct AheadMeters {
    /// The worker's block, 0 when the mix isn't split.
    block: AtomicU32,
    /// Frames the worker has made, and the callback has played.
    made: AtomicU64,
    played: AtomicU64,
    /// Frames the callback played without the worker's share, because
    /// the worker was late.
    late: AtomicU64,
    /// Time the worker spent, and the audio it made, in ns.
    busy_ns: AtomicU64,
    audio_ns: AtomicU64,
    /// Its heaviest block since last taken, in thousandths of the
    /// block's length.
    peak_permille: AtomicU64,
    /// Where the worker is, for the disk thread to read ahead of.
    pub(crate) pos: AtomicI64,
    pub(crate) loop_on: AtomicBool,
    pub(crate) loop_start: AtomicI64,
    pub(crate) loop_end: AtomicI64,
}

impl AheadMeters {
    /// The mix buffer the worker runs at, if the mix is split.
    pub fn block(&self) -> Option<u32> {
        Some(self.block.load(Ordering::Relaxed)).filter(|&b| b > 0)
    }

    /// Frames played without the rest of the mix because it was late.
    pub fn late(&self) -> u64 {
        self.late.load(Ordering::Relaxed)
    }

    /// Frames made ahead and not yet played.
    pub fn queued(&self) -> u64 {
        (self.made.load(Ordering::Relaxed)).saturating_sub(self.played.load(Ordering::Relaxed))
    }

    /// The worker's average load since it started: time spent over the
    /// length of the audio it made (1.0 is as slow as playing it).
    pub fn load(&self) -> f32 {
        let audio = self.audio_ns.load(Ordering::Relaxed);
        if audio == 0 {
            return 0.0;
        }
        self.busy_ns.load(Ordering::Relaxed) as f32 / audio as f32
    }

    /// The heaviest block since the last call, as a fraction of its
    /// length.
    pub fn take_peak_load(&self) -> f32 {
        self.peak_permille.swap(0, Ordering::Relaxed) as f32 / 1000.0
    }

    pub(crate) fn made_so_far(&self) -> u64 {
        self.made.load(Ordering::Relaxed)
    }

    fn note_busy(&self, busy: Duration, frames: usize, rate: u32) {
        let busy = busy.as_nanos() as u64;
        let budget = (frames as u64 * 1_000_000_000 / rate.max(1) as u64).max(1);
        self.busy_ns.fetch_add(busy, Ordering::Relaxed);
        self.audio_ns.fetch_add(budget, Ordering::Relaxed);
        self.peak_permille
            .fetch_max(busy * 1000 / budget, Ordering::Relaxed);
    }
}

/// The worker's side of a split engine.
pub(crate) struct AheadSide {
    msgs: Producer<Msg>,
    ring: Producer<f32>,
    /// Channels per frame in `ring`.
    channels: usize,
    back: Consumer<Back>,
    /// Notes for MIDI outputs, stamped with the frame they play at.
    pub(crate) midi: Producer<(u64, OutEvent)>,
    /// The live tracks of the last graph sent to the callback.
    sent: Vec<bool>,
    /// Tracks whose effects may be on the callback's side: live in a
    /// graph sent there and not yet returned.
    away: Vec<bool>,
    unreturned: usize,
    /// The next segment comes after going round the loop.
    pub(crate) wrapped: bool,
    block: usize,
}

/// The callback's side of a split engine.
pub(crate) struct LiveSide {
    msgs: Consumer<Msg>,
    ring: Consumer<f32>,
    channels: usize,
    back: Producer<Back>,
    midi: Consumer<(u64, OutEvent)>,
    seg: Option<Seg>,
    /// One frame of the ring, `BASE + 2 × MAX_TRACKS` long.
    frame: Vec<f32>,
}

/// Messages in flight: every segment of a block (one a frame, at worst,
/// for a tiny loop) and every command that could come with it.
fn msg_capacity(block: usize) -> usize {
    4 * block + 4 * QUEUE_SLACK
}

const QUEUE_SLACK: usize = 1024;

/// Frames of audio a ring holds: plenty for the worker's lead.
fn ring_frames(block: usize) -> usize {
    3 * block
}

fn ring(block: usize, channels: usize) -> (Producer<f32>, Consumer<f32>) {
    RingBuffer::new(ring_frames(block) * channels)
}

impl Engine {
    pub(crate) fn ahead(&self) -> bool {
        matches!(self.side, Side::Ahead(_))
    }

    /// The worker's side of [`Command::SetGraph`]: keeps the tracks it
    /// plays, moves the live ones' effects to the twin, and sends it on.
    pub(crate) fn set_graph_ahead(&mut self, mut g: Box<Graph>) {
        let Side::Ahead(a) = &mut self.side else {
            return;
        };
        let n = g.tracks.len();
        let mut twin = match g.twin.take() {
            Some(t) => t,
            None => {
                // Nothing for the callback to play but the master.
                let mut t = Session::new(g.tempo.clone())
                    .compile(g.max_block())
                    .expect("an empty session compiles");
                t.master = g.master;
                t.part = Some(Box::new(Part::new(false, &[], g.max_block())));
                g.part = Some(Box::new(Part::new(true, &vec![false; n], g.max_block())));
                Box::new(t)
            }
        };
        if let Some(old) = &mut self.graph {
            let to_disk = &mut self.to_disk;
            g.adopt_chunks(old, |c| crate::engine::give_back(to_disk, c));
            g.adopt_inserts(old);
            g.adopt_instruments(old);
            g.adopt_stretchers(old);
            g.adopt_edges(old);
            g.adopt_cue_lines(old);
        }
        let live: Vec<bool> = (0..n).map(|ti| g.mode(ti) == Mode::Feed).collect();
        a.sent.resize(n, false);
        a.away.resize(n, false);
        for ti in (0..n).filter(|&ti| live[ti]) {
            // The whole track goes across, effects and all; the worker
            // keeps the twin's copy, with the stretcher its clips play
            // through.
            std::mem::swap(&mut g.tracks[ti], &mut twin.tracks[ti]);
            std::mem::swap(&mut g.tracks[ti].stretch, &mut twin.tracks[ti].stretch);
        }
        for (e, t) in g.edges.iter_mut().zip(&mut twin.edges) {
            // Routes out of a newly live track carry on from here; those
            // already live carry on from the callback's own graph.
            if live[e.from] && !a.away[e.from] {
                std::mem::swap(&mut e.line, &mut t.line);
                t.at = e.at;
                t.last = e.last;
            }
        }
        if let Some(p) = &mut twin.part {
            for ((k, &l), &s) in p.adopt.iter_mut().zip(&live).zip(&a.sent) {
                *k = l && s;
            }
        }
        if let Some(p) = &mut g.part {
            for ((m, &l), &away) in p.mode.iter_mut().zip(&live).zip(&a.away) {
                if !l && away {
                    *m = Mode::Hold;
                }
            }
        }
        for (away, &l) in a.away.iter_mut().zip(&live) {
            *away |= l;
        }
        a.sent.copy_from_slice(&live);
        a.unreturned += 1;
        let feeds = g.part.as_ref().map_or(0, |p| p.feeds.len());
        let ring = (channels(feeds) != a.channels).then(|| {
            let (tx, rx) = ring(a.block, channels(feeds));
            a.ring = tx;
            a.channels = channels(feeds);
            (rx, a.channels)
        });
        if a.msgs
            .push(Msg::Graph {
                graph: Some(twin),
                ring,
            })
            .is_err()
        {
            // Never full: there is room checked before each block.
            debug_assert!(false, "no room for a graph");
        }
        g.settle_at(self.pos);
        if let Some(old) = self.graph.replace(g)
            && let Err(rtrb::PushError::Full(old)) = self.retired.push(old)
        {
            std::mem::forget(old);
        }
    }

    /// Commands the callback's side needs too, or instead: copies of the
    /// first sort are sent on and `cmd` is kept; the second sort goes.
    pub(crate) fn forward(&mut self, cmd: Command) -> Option<Command> {
        let Side::Ahead(a) = &mut self.side else {
            return Some(cmd);
        };
        let copy = match &cmd {
            Command::SetMix { track, mix } => Some(Command::SetMix {
                track: *track,
                mix: *mix,
            }),
            Command::SetGroup { group, spec } => Some(Command::SetGroup {
                group: *group,
                spec: *spec,
            }),
            Command::SetSend {
                track,
                send,
                gain_db,
                mute,
            } => Some(Command::SetSend {
                track: *track,
                send: *send,
                gain_db: *gain_db,
                mute: *mute,
            }),
            Command::SetAutoLive {
                track,
                target,
                live,
            } => Some(Command::SetAutoLive {
                track: *track,
                target: *target,
                live: *live,
            }),
            Command::SetMetronome(m) => Some(Command::SetMetronome(*m)),
            Command::SetMeterTap(t) => Some(Command::SetMeterTap(*t)),
            Command::SetCueBus { cue, bus } => Some(Command::SetCueBus {
                cue: *cue,
                bus: *bus,
            }),
            Command::SetCueSend { track, cue, send } => Some(Command::SetCueSend {
                track: *track,
                cue: *cue,
                send: *send,
            }),
            // Input, recording, the speakers and the preview are the
            // callback's alone.
            Command::SetRecording(_)
            | Command::SetPunch(_)
            | Command::SetCapture(_)
            | Command::SetTunerInput(_)
            | Command::SetControlRoom(_)
            | Command::Preview(_) => {
                if let Err(rtrb::PushError::Full(m)) = a.msgs.push(Msg::Cmd(cmd)) {
                    // Never full (see `has_room`).
                    drop(m);
                }
                return None;
            }
            _ => None,
        };
        if let Some(c) = copy {
            let _ = a.msgs.push(Msg::Cmd(c));
        }
        Some(cmd)
    }

    /// Hands the frames `at..at + seg.len` of the block just rendered to
    /// the callback.
    pub(crate) fn emit(
        &mut self,
        at: usize,
        mut seg: Seg,
        left: &[f32],
        right: &[f32],
        click: Option<&[f32]>,
    ) {
        let Side::Ahead(a) = &mut self.side else {
            return;
        };
        seg.wrapped = std::mem::take(&mut a.wrapped);
        let len = seg.len;
        let c = a.channels;
        let tap = matches!(
            (self.meter_tap, &self.graph),
            (Some(Tap::Track(t)), Some(g)) if g.mode(t) == Mode::Run
        );
        let part = self.graph.as_ref().and_then(|g| g.part.as_deref());
        let feeds = part.map_or(&[][..], |p| &p.feeds[..]);
        debug_assert_eq!(c, channels(feeds.len()));
        let Ok(mut chunk) = a.ring.write_chunk(len * c) else {
            debug_assert!(false, "no room in the ring");
            return;
        };
        let (s1, s2) = chunk.as_mut_slices();
        let mut out = s1.iter_mut().chain(s2.iter_mut());
        let mut put = |v: f32| {
            if let Some(o) = out.next() {
                *o = v;
            }
        };
        for i in 0..len {
            let j = at + i;
            put(left[j]);
            put(right[j]);
            put(click.map_or(0.0, |c| c[j]));
            for k in 0..MAX_CUES {
                let (l, r) = self.monitor.cue_at(k, j);
                put(l);
                put(r);
            }
            if tap {
                put(self.tap_buf[0][i]);
                put(self.tap_buf[1][i]);
            } else {
                put(0.0);
                put(0.0);
            }
            if let Some(p) = part {
                for &ti in feeds {
                    put(p.feed[ti][0][i]);
                    put(p.feed[ti][1][i]);
                }
            }
        }
        chunk.commit_all();
        let _ = a.msgs.push(Msg::Seg(seg));
        self.status
            .ahead
            .made
            .fetch_add(len as u64, Ordering::Relaxed);
    }

    /// Tells the disk thread where the worker is.
    pub(crate) fn publish_ahead(&self) {
        let m = &self.status.ahead;
        m.pos.store(self.pos, Ordering::Relaxed);
        if let Some((a, b)) = self.looping {
            m.loop_start.store(a, Ordering::Relaxed);
            m.loop_end.store(b, Ordering::Relaxed);
        }
        m.loop_on.store(self.looping.is_some(), Ordering::Relaxed);
    }

    /// Takes in what the callback sent back.
    fn take_back(&mut self) {
        loop {
            let Side::Ahead(a) = &mut self.side else {
                return;
            };
            let Ok(b) = a.back.pop() else { return };
            match b {
                Back::Ring(r) => drop(r),
                Back::Spent(g) => self.retire(g),
                Back::Graph(old) => {
                    a.unreturned = a.unreturned.saturating_sub(1);
                    let done = a.unreturned == 0;
                    if done {
                        a.away.copy_from_slice(&a.sent);
                    }
                    if let Some(old) = old {
                        self.adopt_returned(old);
                    }
                    if done && let Some(p) = self.graph.as_mut().and_then(|g| g.part.as_mut()) {
                        for m in &mut p.mode {
                            if *m == Mode::Hold {
                                *m = Mode::Run;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Takes back the effects of tracks no longer live from the graph
    /// the callback has finished with.
    fn adopt_returned(&mut self, mut old: Box<Graph>) {
        if let Some(g) = &mut self.graph {
            let n = g.tracks.len();
            let moved: Vec<bool> = (0..n)
                .map(|ti| g.mode(ti) == Mode::Hold && old.mode(ti) == Mode::Live)
                .collect();
            let mine: Vec<bool> = (0..n).map(|ti| g.mode(ti) != Mode::Feed).collect();
            g.adopt_inserts_where(&mut old, &|ti| mine[ti]);
            g.adopt_instruments_where(&mut old, &|ti| mine[ti]);
            g.adopt_edges_where(&mut old, &|ti| moved[ti]);
            g.adopt_cue_lines_where(&mut old, &|ti| moved[ti]);
            if let Some(p) = &mut g.part {
                for ti in (0..n).filter(|&ti| moved[ti]) {
                    p.mode[ti] = Mode::Run;
                }
            }
            if old.holds_parts() && g.part.as_ref().is_some_and(|p| !p.feeds.is_empty()) {
                // Something live now may want what is left.
                if let Side::Ahead(a) = &mut self.side {
                    match a.msgs.push(Msg::Adopt(old)) {
                        Ok(()) => return,
                        Err(rtrb::PushError::Full(Msg::Adopt(o))) => old = o,
                        Err(_) => return,
                    }
                }
            }
        }
        self.retire(old);
    }

    fn retire(&mut self, g: Box<Graph>) {
        if let Err(rtrb::PushError::Full(g)) = self.retired.push(g) {
            // Off the audio thread, freeing is fine.
            drop(g);
        }
    }

    /// Room for a whole block's segments and audio.
    fn has_room(&self, block: usize) -> bool {
        let Side::Ahead(a) = &self.side else {
            return false;
        };
        a.msgs.slots() >= block + QUEUE_SLACK + 16
            && a.ring.slots() >= block * a.channels
            && a.midi.slots() > 0
    }

    /// The callback's side of [`Engine::process_duplex`]: plays the live
    /// tracks for the frames the worker has made, with its share of the
    /// mix added.
    pub(crate) fn process_live(
        &mut self,
        input: &[f32],
        channels: usize,
        left: &mut [f32],
        right: &mut [f32],
        mut click: Option<&mut [f32]>,
    ) {
        let n = left.len().min(right.len());
        self.take_live();
        left.fill(0.0);
        right.fill(0.0);
        if let Some(c) = click.as_deref_mut() {
            c.fill(0.0);
        }
        self.monitor.begin(n);
        self.send_tuner(input, channels, n);
        let inp = |range: std::ops::Range<usize>| InputBlock {
            data: input
                .get(range.start * channels..(range.end * channels).min(input.len()))
                .unwrap_or(&[]),
            channels,
        };
        self.input_meters(inp(0..n));
        let mut done = 0;
        let mut played = 0;
        while done < n {
            let Side::Live(l) = &mut self.side else {
                return;
            };
            let Some(mut seg) = l.seg else {
                match l.msgs.pop() {
                    Ok(Msg::Seg(s)) => {
                        if s.wrapped {
                            self.status.loops.fetch_add(1, Ordering::Relaxed);
                        }
                        l.seg = Some(s).filter(|s| s.len > 0);
                    }
                    Ok(Msg::Cmd(c)) => self.apply_command(c),
                    Ok(Msg::Graph { graph, ring }) => self.swap_live(graph, ring),
                    Ok(Msg::Adopt(g)) => self.adopt_late(g),
                    Err(_) => {
                        // The worker is late: the live tracks play on,
                        // as if stopped, without the rest of the mix.
                        let k = (n - done).min(self.max_block);
                        self.status
                            .ahead
                            .late
                            .fetch_add(k as u64, Ordering::Relaxed);
                        self.clear_returns(k);
                        self.render(
                            inp(done..done + k),
                            done,
                            &mut left[done..done + k],
                            &mut right[done..done + k],
                            false,
                            None,
                        );
                        done += k;
                    }
                }
                continue;
            };
            let k = seg.len.min(n - done).min(self.max_block);
            self.take_returns(done, k);
            self.pos = seg.pos;
            self.looping = seg.looping;
            self.playing = seg.playing;
            self.count_in_to = seg.counting_in.then_some(seg.pos);
            let block = inp(done..done + k);
            self.render(
                block,
                done,
                &mut left[done..done + k],
                &mut right[done..done + k],
                seg.play,
                seg.rate,
            );
            if seg.slow {
                if seg.wrapped || seg.rate.is_some_and(|r| r.reset) {
                    self.slow_at = None;
                }
                self.posf = seg.posf;
                if let Some(r) = seg.rate {
                    self.speed = r.speed;
                }
                self.record_stretched(block, k, seg.counting_in);
            } else if let Some(counting) = seg.record {
                self.record(block, k, counting);
            }
            let clicks = &self.click_buf[..k];
            self.monitor.add_click(done, clicks);
            match click.as_deref_mut() {
                Some(c) if self.metronome.separate_output => {
                    for (o, x) in c[done..done + k].iter_mut().zip(clicks) {
                        *o += x;
                    }
                }
                _ => {
                    for ((l, r), x) in left[done..done + k]
                        .iter_mut()
                        .zip(&mut right[done..done + k])
                        .zip(clicks)
                    {
                        *l += x;
                        *r += x;
                    }
                }
            }
            seg.advance(k);
            self.pos = seg.pos;
            if let Side::Live(l) = &mut self.side {
                l.seg = Some(seg).filter(|s| s.len > 0);
            }
            done += k;
            played += k;
        }
        let start = (self.status.ahead.played).fetch_add(played as u64, Ordering::Relaxed);
        let end = start + played as u64;
        if let Side::Live(l) = &mut self.side {
            // Notes from the worker's tracks, for the frames just played.
            while let Ok(&(at, _)) = l.midi.peek() {
                if at >= end {
                    break;
                }
                if let Ok((_, mut e)) = l.midi.pop() {
                    e.event.frame =
                        (at.saturating_sub(start) as usize).min(n.saturating_sub(1)) as u32;
                    let _ = self.to_midi.push(e);
                }
            }
        }
        self.mix_preview(left, right);
        self.monitor
            .finish(input, channels, left, right, &self.status.monitor);
        self.publish();
    }

    /// Reads `k` frames of the worker's audio into where they play.
    fn take_returns(&mut self, at: usize, k: usize) {
        let Side::Live(l) = &mut self.side else {
            return;
        };
        let c = l.channels;
        let tap = match (self.meter_tap, &self.graph) {
            (Some(Tap::Track(t)), Some(g)) => g.mode(t) == Mode::Away,
            _ => false,
        };
        let Ok(chunk) = l.ring.read_chunk(k * c) else {
            // Never short: a segment's audio goes in before it.
            debug_assert!(false, "the worker's audio is missing");
            self.clear_returns(k);
            return;
        };
        let mut samples = chunk.into_iter();
        let frame = &mut l.frame[..c];
        let mut part = self.graph.as_mut().and_then(|g| g.part.as_deref_mut());
        for i in 0..k {
            for s in frame.iter_mut() {
                *s = samples.next().unwrap_or(0.0);
            }
            if let Some(p) = part.as_deref_mut() {
                p.master[0][i] = frame[0];
                p.master[1][i] = frame[1];
                for (f, &ti) in p.feeds.iter().enumerate() {
                    let [fl, fr] = &mut p.feed[ti];
                    fl[i] = frame[BASE + 2 * f];
                    fr[i] = frame[BASE + 2 * f + 1];
                }
            }
            self.click_buf[i] = frame[2];
            for cue in 0..MAX_CUES {
                let s = CUES_AT + 2 * cue;
                self.monitor.add_cue(cue, at + i, (frame[s], frame[s + 1]));
            }
            if tap {
                self.tap_buf[0][i] = frame[TAP_AT];
                self.tap_buf[1][i] = frame[TAP_AT + 1];
            }
        }
    }

    /// Silence from the worker for `k` frames.
    fn clear_returns(&mut self, k: usize) {
        self.click_buf[..k].fill(0.0);
        if let Some(p) = self.graph.as_mut().and_then(|g| g.part.as_deref_mut()) {
            for b in &mut p.master {
                b[..k].fill(0.0);
            }
            for &ti in &p.feeds {
                for b in &mut p.feed[ti] {
                    b[..k].fill(0.0);
                }
            }
        }
    }

    /// Switches the live tracks to a new graph, taking over the effects
    /// of those that stay live from the old one, which goes back.
    fn swap_live(&mut self, graph: Option<Box<Graph>>, ring: Option<(Consumer<f32>, usize)>) {
        let Side::Live(l) = &mut self.side else {
            return;
        };
        if let Some((r, c)) = ring {
            l.channels = c;
            let old = std::mem::replace(&mut l.ring, r);
            push_back(&mut l.back, Back::Ring(old));
        }
        let old = self.graph.take();
        let old = match graph {
            Some(mut t) => {
                if let Some(mut old) = old {
                    let keep = t
                        .part
                        .as_mut()
                        .map_or(Vec::new(), |p| std::mem::take(&mut p.adopt));
                    let k = |ti: usize| keep.get(ti).copied().unwrap_or(false);
                    t.adopt_inserts_where(&mut old, &k);
                    t.adopt_instruments_where(&mut old, &k);
                    t.adopt_edges_where(&mut old, &k);
                    t.adopt_cue_lines_where(&mut old, &k);
                    if let Some(p) = &mut t.part {
                        p.adopt = keep;
                    }
                    t.settle_at(self.pos);
                    self.graph = Some(t);
                    Some(old)
                } else {
                    t.settle_at(self.pos);
                    self.graph = Some(t);
                    None
                }
            }
            None => old,
        };
        if let Side::Live(l) = &mut self.side {
            push_back(&mut l.back, Back::Graph(old));
        }
    }

    /// Takes what the live tracks are missing from `g`, then sends it
    /// back.
    fn adopt_late(&mut self, mut g: Box<Graph>) {
        if let Some(t) = &mut self.graph {
            let live = |ti: usize| t.part.as_ref().is_some_and(|p| p.mode(ti) == Mode::Live);
            let keep: [bool; MAX_TRACKS] = std::array::from_fn(live);
            t.adopt_inserts_where(&mut g, &|ti| keep.get(ti).copied().unwrap_or(false));
            t.adopt_instruments_where(&mut g, &|ti| keep.get(ti).copied().unwrap_or(false));
        }
        if let Side::Live(l) = &mut self.side {
            push_back(&mut l.back, Back::Spent(g));
        }
    }
}

fn push_back(q: &mut Producer<Back>, b: Back) {
    if let Err(rtrb::PushError::Full(b)) = q.push(b) {
        // Leaking beats freeing on the audio thread.
        std::mem::forget(b);
    }
}

/// The worker: renders the rest of the mix ahead, in big blocks. Run it
/// on a thread of its own with [`Worker::run`], or by hand with
/// [`Worker::fill`].
pub struct Worker {
    engine: Engine,
    block: usize,
    rate: u32,
    left: Vec<f32>,
    right: Vec<f32>,
    click: Vec<f32>,
}

impl Worker {
    /// Renders one more block if the callback has less than a block
    /// waiting and there is room. Returns whether it did.
    pub fn fill(&mut self) -> bool {
        self.engine.take_back();
        let m = &self.engine.status.ahead;
        if m.queued() >= self.block as u64 || !self.engine.has_room(self.block) {
            return false;
        }
        let t0 = std::time::Instant::now();
        let b = self.block;
        self.engine.process_duplex(
            &[],
            0,
            &mut self.left[..b],
            &mut self.right[..b],
            Some(&mut self.click[..b]),
        );
        if let Some(g) = &self.engine.graph {
            self.rate = g.tempo().sample_rate();
        }
        self.engine
            .status
            .ahead
            .note_busy(t0.elapsed(), b, self.rate);
        true
    }

    /// Fills until stopped, waiting for the callback in between.
    pub fn run(mut self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            if !self.fill() {
                std::thread::park_timeout(Duration::from_millis(1));
            }
        }
    }
}

/// The callback's half of a split engine: call
/// [`process_duplex`](Front::process_duplex) from the device callback,
/// as with an unsplit [`Engine`].
pub struct Front {
    live: Engine,
    worker: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    waker: Option<Thread>,
}

impl Front {
    pub fn process_duplex(
        &mut self,
        input: &[f32],
        channels: usize,
        left: &mut [f32],
        right: &mut [f32],
        click: Option<&mut [f32]>,
    ) {
        self.live
            .process_duplex(input, channels, left, right, click);
        if let Some(w) = &self.waker {
            w.unpark();
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.process_duplex(&[], 0, left, right, None);
    }

    /// See [`Engine::main_out`].
    pub fn main_out(&self) -> Option<crate::monitor::OutPair> {
        self.live.main_out()
    }

    /// See [`Engine::each_cue_out`].
    pub fn each_cue_out(&self, f: &mut dyn FnMut(usize, &[f32])) {
        self.live.each_cue_out(f);
    }
}

impl Drop for Front {
    fn drop(&mut self) {
        if let Some((stop, handle)) = self.worker.take() {
            stop.store(true, Ordering::Relaxed);
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// Splits a new engine (one that has not yet run) so the mix renders
/// ahead in blocks of `block` frames on a worker thread, while live
/// tracks play in the device callback. `on_start` runs first on the
/// worker thread, to raise its priority.
pub fn split(engine: Engine, block: usize, on_start: impl FnOnce() + Send + 'static) -> Front {
    let (mut front, worker) = split_by_hand(engine, block);
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let handle = std::thread::Builder::new()
        .name("mix ahead".into())
        .spawn(move || {
            on_start();
            worker.run(&s);
        })
        .expect("could not start the mix thread");
    front.waker = Some(handle.thread().clone());
    front.worker = Some((stop, handle));
    front
}

/// [`split`], leaving the worker to be run by hand (for tests and
/// offline use).
pub fn split_by_hand(mut engine: Engine, block: usize) -> (Front, Worker) {
    let block = block.clamp(1, engine.max_block.min(MAX_MIX_BLOCK));
    let status = engine.status.clone();
    let mut w = crate::engine::blank(engine.max_block, status.clone());
    // The worker takes the commands, the graphs and the disk; the rest
    // (input, recording, meters, MIDI, the preview) stays with the
    // callback.
    std::mem::swap(&mut w.commands, &mut engine.commands);
    std::mem::swap(&mut w.retired, &mut engine.retired);
    std::mem::swap(&mut w.from_disk, &mut engine.from_disk);
    std::mem::swap(&mut w.to_disk, &mut engine.to_disk);
    let (msg_tx, msg_rx) = RingBuffer::new(msg_capacity(block));
    let (ring_tx, ring_rx) = ring(block, BASE);
    let (back_tx, back_rx) = RingBuffer::new(4 * QUEUE_SLACK);
    let (midi_tx, midi_rx) = RingBuffer::new(crate::engine::MIDI_QUEUE);
    w.side = Side::Ahead(Box::new(AheadSide {
        msgs: msg_tx,
        ring: ring_tx,
        channels: BASE,
        back: back_rx,
        midi: midi_tx,
        sent: Vec::with_capacity(MAX_TRACKS),
        away: Vec::with_capacity(MAX_TRACKS),
        unreturned: 0,
        wrapped: false,
        block,
    }));
    engine.side = Side::Live(Box::new(LiveSide {
        msgs: msg_rx,
        ring: ring_rx,
        channels: BASE,
        back: back_tx,
        midi: midi_rx,
        seg: None,
        frame: vec![0.0; channels(MAX_TRACKS)],
    }));
    status.ahead.block.store(block as u32, Ordering::Relaxed);
    let worker = Worker {
        engine: w,
        block,
        rate: 48_000,
        left: vec![0.0; block],
        right: vec![0.0; block],
        click: vec![0.0; block],
    };
    (
        Front {
            live: engine,
            worker: None,
            waker: None,
        },
        worker,
    )
}
