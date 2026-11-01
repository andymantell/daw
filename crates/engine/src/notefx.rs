// SPDX-License-Identifier: GPL-3.0-or-later
//! Note effects (Phase 4 plan A6): effects on notes before they reach
//! an instrument. They sit at the head of an instrument track's chain,
//! so the notes from clips and those played live both go through them,
//! and the track's MIDI output gets what they make.
//!
//! - An **arpeggiator** plays the notes held down one after another, in
//!   step with the song (or with the first key pressed when stopped).
//! - **Chord** plays a chord on each note: a fixed shape, or one built
//!   from the song's key and scale.
//! - **Note length** makes notes a set length, at least a length, or
//!   longer by a length.
//! - **Velocity** scales, shifts and limits how hard notes are played.
//! - **Randomise** moves notes' pitch and velocity a little at random,
//!   and can leave some out.
//! - **Scale** moves notes onto the song's key and scale, or leaves out
//!   the ones off it.
//!
//! A chain runs on the audio thread, so like an instrument it never
//! allocates, locks or blocks: everything it keeps is in lists made to
//! size when the graph is built. Every note an effect makes gets an id
//! of its own, and an effect remembers what each note it was given
//! became, so the note's end stops exactly the notes it started.
//!
//! The same chain renders notes offline ([`render`]): with one frame to
//! a tick, the events it makes are the notes to keep.

use crate::midi::{Event, Kind, MAX_EVENTS};

/// A key and scale as the effects use it: the root's pitch class (0 for
/// C) and the scale's notes as bits, bit `k` for `k` semitones above
/// the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleSpec {
    pub root: u8,
    pub mask: u16,
}

impl ScaleSpec {
    /// Every note: the effects that use a scale leave notes as they are.
    pub const CHROMATIC: ScaleSpec = ScaleSpec {
        root: 0,
        mask: 0x0fff,
    };

    /// A scale from its root and the semitones above it of each note.
    pub fn new(root: u8, steps: &[u8]) -> ScaleSpec {
        let mask = steps.iter().fold(0u16, |m, &s| m | 1 << (s % 12));
        ScaleSpec {
            root: root % 12,
            mask: if mask == 0 { 1 } else { mask },
        }
    }

    pub fn contains(self, pitch: i32) -> bool {
        let k = (pitch - self.root as i32).rem_euclid(12);
        self.mask >> k & 1 == 1
    }

    /// The note of the scale nearest `pitch` in the direction asked
    /// (between two equally near, the lower), on the keyboard.
    pub fn snap(self, pitch: u8, to: Snap) -> Option<u8> {
        let p = pitch as i32;
        if self.contains(p) {
            return Some(pitch);
        }
        let ok = |q: i32| (0..=127).contains(&q) && self.contains(q);
        for d in 1..12 {
            let found = match to {
                Snap::Nearest => [p - d, p + d].into_iter().find(|&q| ok(q)),
                Snap::Up => Some(p + d).filter(|&q| ok(q)),
                Snap::Down => Some(p - d).filter(|&q| ok(q)),
                Snap::Leave => return None,
            };
            if let Some(q) = found {
                return Some(q as u8);
            }
        }
        None
    }

    /// The note `by` scale steps from `pitch` (moved down onto the
    /// scale first), on the keyboard.
    pub fn step(self, pitch: u8, by: i32) -> Option<u8> {
        let mut p = self
            .snap(pitch, Snap::Down)
            .or(self.snap(pitch, Snap::Up))? as i32;
        let dir = by.signum();
        for _ in 0..by.unsigned_abs() {
            p += dir;
            while (0..=127).contains(&p) && !self.contains(p) {
                p += dir;
            }
        }
        u8::try_from(p).ok().filter(|&p| p <= 127)
    }
}

/// Where the scale effect moves a note that is off the scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snap {
    Nearest,
    Up,
    Down,
    /// Not played at all.
    Leave,
}

/// The order an arpeggiator plays the held notes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArpMode {
    Up,
    Down,
    UpDown,
    DownUp,
    /// In the order the keys were pressed.
    AsPlayed,
    Random,
    /// Every held note at once, on each step.
    Chord,
}

/// What a chord effect adds to each note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordShape {
    /// Semitones above the note, up to four (the note itself is 0 and
    /// always plays); `count` of them are used.
    Fixed { intervals: [i8; 4], count: u8 },
    /// Notes `count` thirds up the key's scale from the note: a triad
    /// for 2, a seventh chord for 3.
    InKey { count: u8 },
}

/// How a note-length effect changes notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthMode {
    /// Every note lasts the length, however long the key is held.
    Fixed,
    /// Notes shorter than the length are made up to it.
    AtLeast,
    /// Every note lasts the length longer than it was held.
    LongerBy,
}

/// One note effect and its settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteFxKind {
    Arpeggiator {
        mode: ArpMode,
        /// Ticks between steps.
        rate: f64,
        /// How many octaves up the pattern goes, 1 to 4.
        octaves: u8,
        /// How long each step sounds, as a share of the step: up to 1,
        /// where each note lasts until the next.
        gate: f32,
        /// Keep playing after the keys are let go, until new ones are
        /// pressed.
        latch: bool,
        /// 0 plays each note as hard as it was played; 1 to 127 plays
        /// every step at that velocity.
        velocity: u8,
    },
    Chord {
        shape: ChordShape,
    },
    Length {
        mode: LengthMode,
        ticks: f64,
    },
    Velocity {
        /// Multiplies velocity (1 leaves it), then `offset` is added,
        /// and the result kept between `min` and `max`.
        scale: f32,
        offset: i32,
        min: u8,
        max: u8,
    },
    Randomise {
        /// Up to this many semitones up or down.
        pitch: u8,
        /// Up to this much velocity up or down.
        velocity: u8,
        /// 0 to 1: the chance each note plays.
        chance: f32,
        /// Moved pitches stay on the key's scale.
        in_scale: bool,
    },
    Scale {
        snap: Snap,
    },
}

/// A note effect on a track, under an id unique on the track so it
/// keeps what it has sounding when the graph is built again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteFxSpec {
    pub id: u64,
    pub bypass: bool,
    pub kind: NoteFxKind,
}

/// Where a block is in musical time, for the effects that keep time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// The tick the block starts at: on the timeline while playing.
    pub at: f64,
    /// Ticks each frame lasts, above 0.
    pub per_frame: f64,
    /// The transport is playing, so `at` is the song's position; when
    /// stopped the chain keeps time of its own from the first note.
    pub playing: bool,
}

/// The most notes one effect keeps track of at once; past it, notes
/// pass through unchanged.
const MAX_NOTES: usize = 128;
/// The most notes one chord can have.
const MAX_CHORD: usize = 5;
/// The most note ends an effect can have waiting.
const MAX_PENDING: usize = 256;
/// The most keys an arpeggiator holds.
const MAX_HELD: usize = 32;
/// Ids the effects give the notes they make start here, above those of
/// notes from clips and live notes.
const FIRST_ID: u32 = 0x8000_0000;

/// What a note given to an effect became.
#[derive(Clone, Copy, Debug)]
struct Mapped {
    channel: u8,
    pitch: u8,
    id: u32,
    /// The notes it started: pitch and id; `count` of them.
    outs: [(u8, u32); MAX_CHORD],
    count: u8,
    /// The frame (on the chain's own count) it started at.
    born: f64,
}

/// A note-off waiting for its frame (on the chain's own count).
#[derive(Clone, Copy, Debug)]
struct Pending {
    due: f64,
    channel: u8,
    pitch: u8,
    id: u32,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    channel: u8,
    pitch: u8,
    velocity: u8,
    /// When pressed, counted, for playing in the order played.
    order: u32,
    /// Still held (not only latched).
    down: bool,
}

struct Stage {
    id: u64,
    bypass: bool,
    kind: NoteFxKind,
    notes: Vec<Mapped>,
    pending: Vec<Pending>,
    // The arpeggiator's keys, step and when the next step is, in ticks.
    held: Vec<Held>,
    pressed: u32,
    step: u64,
    next: Option<f64>,
    /// The next step is the first after keys were pressed: the ones
    /// after it fall on the grid.
    restart: bool,
}

impl Stage {
    fn new(spec: &NoteFxSpec) -> Stage {
        let arp = matches!(spec.kind, NoteFxKind::Arpeggiator { .. });
        Stage {
            id: spec.id,
            bypass: spec.bypass,
            kind: spec.kind,
            notes: Vec::with_capacity(MAX_NOTES),
            pending: Vec::with_capacity(MAX_PENDING),
            held: Vec::with_capacity(if arp { MAX_HELD } else { 0 }),
            pressed: 0,
            step: 0,
            next: None,
            restart: false,
        }
    }
}

/// A track's note effects, in order, with what each has sounding.
pub struct Chain {
    stages: Vec<Stage>,
    scale: ScaleSpec,
    /// Frames since the chain started, for note ends and for keeping
    /// time when stopped.
    now: f64,
    /// Ticks counted while stopped.
    free: f64,
    next_id: u32,
    rng: u64,
    /// The last block's clock.
    last: Clock,
    /// The block's events between effects; capacity [`MAX_EVENTS`].
    scratch: Vec<Event>,
}

impl Chain {
    pub fn new(specs: &[NoteFxSpec], scale: ScaleSpec) -> Chain {
        Chain {
            stages: specs.iter().map(Stage::new).collect(),
            scale,
            now: 0.0,
            free: 0.0,
            next_id: FIRST_ID,
            rng: 0x2545_f491_4f6c_dd1d,
            last: Clock {
                at: 0.0,
                per_frame: 1.0,
                playing: false,
            },
            scratch: Vec::with_capacity(MAX_EVENTS),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Takes over what `old` has sounding, where it has the same effects
    /// in the same order (with settings that may have changed), so a
    /// change while notes play cuts nothing off. Otherwise `old`'s notes
    /// are stopped, into `stop`. Says whether it took them over.
    pub fn adopt(&mut self, old: &mut Chain, stop: &mut Vec<Event>) -> bool {
        let same = self.stages.len() == old.stages.len()
            && self.stages.iter().zip(&old.stages).all(|(a, b)| {
                a.id == b.id && std::mem::discriminant(&a.kind) == std::mem::discriminant(&b.kind)
            });
        if same {
            for (s, o) in self.stages.iter_mut().zip(&mut old.stages) {
                std::mem::swap(&mut s.notes, &mut o.notes);
                std::mem::swap(&mut s.pending, &mut o.pending);
                std::mem::swap(&mut s.held, &mut o.held);
                s.pressed = o.pressed;
                s.step = o.step;
                s.next = o.next;
                s.restart = o.restart;
                if let NoteFxKind::Arpeggiator { latch: false, .. } = s.kind {
                    s.held.retain(|h| h.down);
                }
            }
            self.now = old.now;
            self.free = old.free;
            self.next_id = old.next_id;
            self.rng = old.rng;
            self.last = old.last;
            return true;
        }
        old.stop(stop);
        false
    }

    /// Runs note ends (and other events at frame 0) through the effects
    /// between blocks, with time standing still.
    pub fn ends(&mut self, events: &mut Vec<Event>) {
        let clock = self.last;
        self.process(events, 0, clock);
    }

    /// Note-offs, at frame 0, for every note the chain has sounding,
    /// which it then forgets.
    pub fn stop(&mut self, out: &mut Vec<Event>) {
        for s in &mut self.stages {
            for m in &s.notes {
                for &(pitch, id) in &m.outs[..m.count as usize] {
                    push(out, Event::note_off(0, m.channel, pitch, id));
                }
            }
            for p in &s.pending {
                push(out, Event::note_off(0, p.channel, p.pitch, p.id));
            }
            s.notes.clear();
            s.pending.clear();
            s.held.clear();
            s.next = None;
        }
    }

    /// Runs `events` (one block of `n` frames, in frame order) through
    /// the effects, leaving what they make in `events`, in frame order.
    pub fn process(&mut self, events: &mut Vec<Event>, n: usize, clock: Clock) {
        let mut clock = Clock {
            per_frame: clock.per_frame.max(1e-9),
            ..clock
        };
        if !clock.playing {
            clock.at = self.free;
        }
        self.last = clock;
        for i in 0..self.stages.len() {
            self.scratch.clear();
            let mut cx = Cx {
                scale: self.scale,
                now: self.now,
                n,
                clock,
                next_id: &mut self.next_id,
                rng: &mut self.rng,
            };
            run(&mut self.stages[i], events, &mut self.scratch, &mut cx);
            sort(&mut self.scratch);
            std::mem::swap(events, &mut self.scratch);
        }
        self.now += n as f64;
        self.free += n as f64 * clock.per_frame;
    }
}

/// What every effect is given for a block.
struct Cx<'a> {
    scale: ScaleSpec,
    now: f64,
    n: usize,
    clock: Clock,
    next_id: &'a mut u32,
    rng: &'a mut u64,
}

impl Cx<'_> {
    fn id(&mut self) -> u32 {
        let id = *self.next_id;
        *self.next_id = self.next_id.wrapping_add(1).max(FIRST_ID);
        id
    }

    /// 0 to 1.
    fn random(&mut self) -> f64 {
        // xorshift64*
        let mut x = *self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *self.rng = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// −`range` to `range`, every whole number as likely.
    fn spread(&mut self, range: u8) -> i32 {
        if range == 0 {
            return 0;
        }
        let k = 2 * range as i32 + 1;
        ((self.random() * k as f64) as i32).min(k - 1) - range as i32
    }

    /// The frame, from the block's start, that tick `t` falls in (a
    /// hair's breadth early counts as on it).
    fn frame_of(&self, t: f64) -> f64 {
        on_frame((t - self.clock.at) / self.clock.per_frame)
    }
}

/// The frame a time in frames falls in, rounding error forgiven.
fn on_frame(f: f64) -> f64 {
    (f + 1e-6).floor()
}

/// Adds an event if there is room: lists are made to size, and a block
/// with more events than that drops the rest.
fn push(out: &mut Vec<Event>, e: Event) {
    if out.len() < out.capacity() {
        out.push(e);
    }
}

/// A stable sort by frame that never allocates: the events come nearly
/// in order.
fn sort(events: &mut [Event]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events[j - 1].frame > events[j].frame {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn run(s: &mut Stage, input: &[Event], out: &mut Vec<Event>, cx: &mut Cx) {
    match s.kind {
        NoteFxKind::Arpeggiator { .. } => arpeggiate(s, input, out, cx),
        NoteFxKind::Length { .. } => lengthen(s, input, out, cx),
        _ => map_notes(s, input, out, cx),
    }
}

/// The note an end is for: by its id, or else (a graph built again
/// numbers notes from clips afresh) the oldest at its pitch.
fn find(notes: &[Mapped], channel: u8, pitch: u8, id: u32) -> Option<usize> {
    let at = |m: &Mapped| (m.channel, m.pitch) == (channel, pitch);
    (notes.iter().position(|m| at(m) && m.id == id)).or_else(|| {
        (notes.iter().enumerate())
            .filter(|(_, m)| at(m))
            .min_by(|a, b| a.1.born.total_cmp(&b.1.born))
            .map(|(k, _)| k)
    })
}

/// Where a note's end goes: the notes it started, forgotten.
fn end(s: &mut Stage, e: &Event, out: &mut Vec<Event>) -> bool {
    let Kind::NoteOff {
        pitch,
        velocity,
        id,
    } = e.kind
    else {
        return false;
    };
    let Some(k) = find(&s.notes, e.channel, pitch, id) else {
        return false;
    };
    let m = s.notes.swap_remove(k);
    for &(pitch, id) in &m.outs[..m.count as usize] {
        push(
            out,
            Event {
                kind: Kind::NoteOff {
                    pitch,
                    velocity,
                    id,
                },
                ..*e
            },
        );
    }
    true
}

/// Pressure or expression on a note goes to the notes it became.
fn follow(s: &Stage, e: &Event, out: &mut Vec<Event>) -> bool {
    let (pitch, id) = match e.kind {
        Kind::KeyPressure { pitch, .. } => (pitch, None),
        Kind::NoteExpression { pitch, id, .. } => (pitch, Some(id)),
        _ => return false,
    };
    let Some(m) = (s.notes.iter())
        .find(|m| (m.channel, m.pitch) == (e.channel, pitch) && id.is_none_or(|id| id == m.id))
    else {
        return false;
    };
    for &(p, i) in &m.outs[..m.count as usize] {
        let kind = match e.kind {
            Kind::KeyPressure { value, .. } => Kind::KeyPressure { pitch: p, value },
            Kind::NoteExpression { kind, value, .. } => Kind::NoteExpression {
                id: i,
                pitch: p,
                kind,
                value,
            },
            k => k,
        };
        push(out, Event { kind, ..*e });
    }
    true
}

/// The effects that turn each note into notes of their own as it
/// starts: chord, velocity, randomise and scale.
fn map_notes(s: &mut Stage, input: &[Event], out: &mut Vec<Event>, cx: &mut Cx) {
    for e in input {
        if let Kind::AllNotesOff = e.kind {
            s.notes.clear();
            push(out, *e);
            continue;
        }
        if end(s, e, out) || follow(s, e, out) {
            continue;
        }
        let Kind::NoteOn {
            pitch,
            velocity,
            id,
        } = e.kind
        else {
            push(out, *e);
            continue;
        };
        if s.bypass || s.notes.len() >= s.notes.capacity() {
            push(out, *e);
            continue;
        }
        let mut m = Mapped {
            channel: e.channel,
            pitch,
            id,
            outs: [(0, 0); MAX_CHORD],
            count: 0,
            born: cx.now + e.frame as f64,
        };
        let mut add = |m: &mut Mapped, p: i32, v: i32, cx: &mut Cx| {
            let Ok(p) = u8::try_from(p) else { return };
            if p > 127 || m.count as usize >= MAX_CHORD {
                return;
            }
            // A chord's notes that land on one another play once.
            if m.outs[..m.count as usize].iter().any(|o| o.0 == p) {
                return;
            }
            let id = cx.id();
            m.outs[m.count as usize] = (p, id);
            m.count += 1;
            push(
                out,
                Event::note_on(e.frame, e.channel, p, v.clamp(1, 127) as u8, id),
            );
        };
        let (p, v) = (pitch as i32, velocity as i32);
        match s.kind {
            NoteFxKind::Chord { shape } => {
                add(&mut m, p, v, cx);
                match shape {
                    ChordShape::Fixed { intervals, count } => {
                        for &k in &intervals[..(count as usize).min(4)] {
                            add(&mut m, p + k as i32, v, cx);
                        }
                    }
                    ChordShape::InKey { count } => {
                        for k in 1..=count.min(4) as i32 {
                            if let Some(q) = cx.scale.step(pitch, 2 * k) {
                                add(&mut m, q as i32, v, cx);
                            }
                        }
                    }
                }
            }
            NoteFxKind::Velocity {
                scale,
                offset,
                min,
                max,
            } => {
                let lo = min.clamp(1, 127) as i32;
                let hi = (max.clamp(1, 127) as i32).max(lo);
                let v = (v as f32 * scale).round() as i32 + offset;
                add(&mut m, p, v.clamp(lo, hi), cx);
            }
            NoteFxKind::Randomise {
                pitch: range,
                velocity: vrange,
                chance,
                in_scale,
            } => {
                if cx.random() < chance as f64 {
                    let mut q = (p + cx.spread(range)).clamp(0, 127);
                    if in_scale && range > 0 {
                        q = cx
                            .scale
                            .snap(q as u8, Snap::Nearest)
                            .map_or(q, |q| q as i32);
                    }
                    let v = v + cx.spread(vrange);
                    add(&mut m, q, v, cx);
                }
            }
            NoteFxKind::Scale { snap } => {
                if let Some(q) = cx.scale.snap(pitch, snap) {
                    add(&mut m, q as i32, v, cx);
                }
            }
            NoteFxKind::Arpeggiator { .. } | NoteFxKind::Length { .. } => unreachable!(),
        }
        s.notes.push(m);
    }
}

/// Note ends that are due before `before` (frames from the block's
/// start), at their frames.
fn due(s: &mut Stage, before: f64, out: &mut Vec<Event>, cx: &Cx) {
    let mut k = 0;
    while k < s.pending.len() {
        let p = s.pending[k];
        let f = on_frame(p.due - cx.now);
        if f < before {
            let frame = f.clamp(0.0, cx.n.saturating_sub(1) as f64) as u32;
            push(out, Event::note_off(frame, p.channel, p.pitch, p.id));
            s.pending.swap_remove(k);
        } else {
            k += 1;
        }
    }
}

fn wait(s: &mut Stage, at: f64, channel: u8, pitch: u8, id: u32) {
    if s.pending.len() < s.pending.capacity() {
        s.pending.push(Pending {
            due: at,
            channel,
            pitch,
            id,
        });
    }
}

/// Note length: notes start as they are, and end when the length says.
fn lengthen(s: &mut Stage, input: &[Event], out: &mut Vec<Event>, cx: &mut Cx) {
    let NoteFxKind::Length { mode, ticks } = s.kind else {
        return;
    };
    let frames = ticks.max(1.0) / cx.clock.per_frame;
    for e in input {
        due(s, e.frame as f64, out, cx);
        let at = cx.now + e.frame as f64;
        match e.kind {
            Kind::AllNotesOff => {
                s.notes.clear();
                s.pending.clear();
                push(out, *e);
            }
            Kind::NoteOn {
                pitch,
                velocity,
                id,
            } if !s.bypass && s.notes.len() < s.notes.capacity() => {
                let new = cx.id();
                let mut m = Mapped {
                    channel: e.channel,
                    pitch,
                    id,
                    outs: [(0, 0); MAX_CHORD],
                    count: 1,
                    born: at,
                };
                m.outs[0] = (pitch, new);
                s.notes.push(m);
                if mode == LengthMode::Fixed {
                    wait(s, at + frames, e.channel, pitch, new);
                }
                push(
                    out,
                    Event::note_on(e.frame, e.channel, pitch, velocity, new),
                );
            }
            Kind::NoteOff { pitch, id, .. } => {
                let Some(k) = find(&s.notes, e.channel, pitch, id) else {
                    push(out, *e);
                    continue;
                };
                let m = s.notes.swap_remove(k);
                let (p, new) = m.outs[0];
                let end = match mode {
                    // Its end is already waiting.
                    LengthMode::Fixed => continue,
                    LengthMode::AtLeast => m.born + frames,
                    LengthMode::LongerBy => at + frames,
                };
                if end <= at || s.bypass {
                    push(out, Event::note_off(e.frame, e.channel, p, new));
                } else {
                    wait(s, end, e.channel, p, new);
                }
            }
            _ => {
                if !follow(s, e, out) {
                    push(out, *e);
                }
            }
        }
    }
    due(s, cx.n as f64, out, cx);
}

/// The arpeggiator: keys pressed go into the held set, and each step
/// plays the next of them.
fn arpeggiate(s: &mut Stage, input: &[Event], out: &mut Vec<Event>, cx: &mut Cx) {
    let NoteFxKind::Arpeggiator {
        mode,
        rate,
        octaves,
        gate,
        latch,
        velocity,
    } = s.kind
    else {
        return;
    };
    let rate = rate.max(1.0);
    if s.bypass {
        // Let what it was playing end, and pass notes through.
        for p in s.pending.drain(..) {
            push(out, Event::note_off(0, p.channel, p.pitch, p.id));
        }
        s.held.clear();
        s.next = None;
        out.extend_from_slice(&input[..input.len().min(out.capacity() - out.len())]);
        return;
    }
    // A jump in time (a seek, round the loop, play or stop) puts the
    // next step back on the grid.
    if let Some(next) = s.next {
        let t = cx.clock.at;
        if next < t - rate * 0.5 || next > t + rate * 2.0 {
            s.next = Some((t / rate).ceil() * rate);
        }
    }
    let n = cx.n as f64;
    let mut i = 0;
    loop {
        let step_at = match s.next {
            Some(t) if !s.held.is_empty() => cx.frame_of(t),
            _ => f64::INFINITY,
        };
        let event_at = input.get(i).map_or(f64::INFINITY, |e| e.frame as f64);
        // Events at a step's frame come first, so keys pressed together
        // all play in it.
        if step_at < n && step_at < event_at {
            let frame = step_at.max(0.0);
            due(s, frame + 1e-6, out, cx);
            play_step(s, frame, mode, rate, octaves, gate, velocity, out, cx);
            let t = cx.clock.at + frame * cx.clock.per_frame;
            s.next = Some(if std::mem::take(&mut s.restart) {
                let mut next = ((t / rate) + 1e-3).floor() * rate + rate;
                if next - t < rate * 0.25 {
                    next += rate;
                }
                next
            } else {
                s.next.unwrap_or(t) + rate
            });
            continue;
        }
        let Some(e) = input.get(i) else { break };
        i += 1;
        due(s, e.frame as f64, out, cx);
        match e.kind {
            Kind::AllNotesOff => {
                s.held.clear();
                s.pending.clear();
                s.next = None;
                push(out, *e);
            }
            Kind::NoteOn {
                pitch, velocity: v, ..
            } => {
                // Pressing keys after letting all go starts a new set.
                if s.held.iter().all(|h| !h.down) {
                    s.held.clear();
                }
                let first = s.held.is_empty();
                s.held
                    .retain(|h| (h.channel, h.pitch) != (e.channel, pitch));
                if s.held.len() < s.held.capacity() {
                    s.pressed = s.pressed.wrapping_add(1);
                    s.held.push(Held {
                        channel: e.channel,
                        pitch,
                        velocity: v,
                        order: s.pressed,
                        down: true,
                    });
                }
                if first {
                    // The first keys play at once, and the steps after
                    // them fall on the grid.
                    s.step = 0;
                    s.restart = true;
                    s.next = Some(cx.clock.at + e.frame as f64 * cx.clock.per_frame);
                }
            }
            Kind::NoteOff { pitch, .. } => {
                if latch {
                    for h in s.held.iter_mut() {
                        if (h.channel, h.pitch) == (e.channel, pitch) {
                            h.down = false;
                        }
                    }
                } else {
                    s.held
                        .retain(|h| (h.channel, h.pitch) != (e.channel, pitch));
                }
            }
            // Per-note expression has no one note to go to.
            Kind::KeyPressure { .. } | Kind::NoteExpression { .. } => {}
            _ => push(out, *e),
        }
    }
    if s.held.is_empty() {
        s.next = None;
    }
    due(s, n, out, cx);
}

#[allow(clippy::too_many_arguments)]
fn play_step(
    s: &mut Stage,
    frame: f64,
    mode: ArpMode,
    rate: f64,
    octaves: u8,
    gate: f32,
    velocity: u8,
    out: &mut Vec<Event>,
    cx: &mut Cx,
) {
    let h = s.held.len();
    if h == 0 {
        return;
    }
    // The held notes in the order the mode walks them, lowest first.
    let mut keys = [0u8; MAX_HELD];
    let mut idx: [usize; MAX_HELD] = std::array::from_fn(|i| i);
    let idx = &mut idx[..h];
    if mode == ArpMode::AsPlayed {
        idx.sort_unstable_by_key(|&k| s.held[k].order);
    } else {
        idx.sort_unstable_by_key(|&k| s.held[k].pitch);
    }
    for (k, &j) in idx.iter().enumerate() {
        keys[k] = j as u8;
    }
    let oct = octaves.clamp(1, 4) as usize;
    let len = h * oct;
    let at = |k: usize| -> (usize, i32) { (keys[k % h] as usize, 12 * (k / h) as i32) };
    let step = s.step as usize;
    s.step = s.step.wrapping_add(1);
    let mut picks = [(0usize, 0i32); MAX_HELD];
    let count = match mode {
        ArpMode::Up | ArpMode::AsPlayed => {
            picks[0] = at(step % len);
            1
        }
        ArpMode::Down => {
            picks[0] = at(len - 1 - step % len);
            1
        }
        ArpMode::UpDown | ArpMode::DownUp => {
            let period = if len > 1 { 2 * len - 2 } else { 1 };
            let k = step % period;
            let k = if k < len { k } else { period - k };
            picks[0] = at(if mode == ArpMode::UpDown {
                k
            } else {
                len - 1 - k
            });
            1
        }
        ArpMode::Random => {
            let k = ((cx.random() * len as f64) as usize).min(len - 1);
            picks[0] = at(k);
            1
        }
        ArpMode::Chord => {
            let o = 12 * (step % oct) as i32;
            for (k, &j) in idx.iter().enumerate() {
                picks[k] = (j, o);
            }
            h
        }
    };
    let frames = (rate * gate.clamp(0.05, 1.0) as f64) / cx.clock.per_frame;
    let start = cx.now + frame;
    for &(j, o) in &picks[..count] {
        let held = s.held[j];
        let Some(pitch) = u8::try_from(held.pitch as i32 + o)
            .ok()
            .filter(|&p| p <= 127)
        else {
            continue;
        };
        let id = cx.id();
        let v = if velocity == 0 {
            held.velocity
        } else {
            velocity
        };
        push(
            out,
            Event::note_on(frame as u32, held.channel, pitch, v, id),
        );
        wait(s, start + frames, held.channel, pitch, id);
    }
}

/// A note as [`render`] gives and takes it: ticks from wherever the
/// caller counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderNote {
    pub start: i64,
    pub len: i64,
    pub pitch: u8,
    pub velocity: u8,
    pub channel: u8,
}

/// The notes `notes` become through the effects, for keeping as notes:
/// the arpeggiator in step with ticks counted from 0, time running
/// from `from` to `to`, where anything still sounding is ended.
pub fn render(
    specs: &[NoteFxSpec],
    scale: ScaleSpec,
    notes: &[RenderNote],
    from: i64,
    to: i64,
) -> Vec<RenderNote> {
    // One frame to a tick, in blocks of a sixteenth.
    const BLOCK: i64 = 240;
    let mut chain = Chain::new(specs, scale);
    let mut edges: Vec<(i64, u8, Event)> = Vec::new();
    for (k, n) in notes.iter().enumerate() {
        let id = k as u32;
        let end = (n.start + n.len.max(1)).min(to);
        if n.start < from || n.start >= to {
            continue;
        }
        let on = Event::note_on(0, n.channel, n.pitch, n.velocity, id);
        // Ends before starts at the same tick.
        edges.push((n.start, 1, on));
        edges.push((end, 0, Event::note_off(0, n.channel, n.pitch, id)));
    }
    edges.sort_by_key(|e| (e.0, e.1));
    let mut out: Vec<RenderNote> = Vec::new();
    let mut open: Vec<(u32, i64, u8, u8, u8)> = Vec::new();
    let mut events = Vec::with_capacity(MAX_EVENTS);
    let mut k = 0;
    let mut at = from;
    let mut close = |open: &mut Vec<(u32, i64, u8, u8, u8)>, id: u32, end: i64| {
        if let Some(i) = open.iter().position(|o| o.0 == id) {
            let (_, start, pitch, velocity, channel) = open.swap_remove(i);
            out.push(RenderNote {
                start,
                len: (end - start).max(1),
                pitch,
                velocity,
                channel,
            });
        }
    };
    while at < to {
        let n = BLOCK.min(to - at);
        events.clear();
        while k < edges.len() && edges[k].0 < at + n {
            let mut e = edges[k].2;
            e.frame = (edges[k].0 - at) as u32;
            if events.len() < events.capacity() {
                events.push(e);
            }
            k += 1;
        }
        chain.process(
            &mut events,
            n as usize,
            Clock {
                at: at as f64,
                per_frame: 1.0,
                playing: true,
            },
        );
        for e in &events {
            let t = at + e.frame as i64;
            match e.kind {
                Kind::NoteOn {
                    pitch,
                    velocity,
                    id,
                } => open.push((id, t, pitch, velocity, e.channel)),
                Kind::NoteOff { id, .. } => close(&mut open, id, t),
                _ => {}
            }
        }
        at += n;
    }
    for id in open.iter().map(|o| o.0).collect::<Vec<_>>() {
        close(&mut open, id, to);
    }
    out.sort_by_key(|n| (n.start, n.pitch));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: f64 = 960.0;

    fn arp(mode: ArpMode, octaves: u8, latch: bool) -> NoteFxSpec {
        NoteFxSpec {
            id: 1,
            bypass: false,
            kind: NoteFxKind::Arpeggiator {
                mode,
                rate: Q / 4.0,
                octaves,
                gate: 0.5,
                latch,
                velocity: 0,
            },
        }
    }

    fn note(start: i64, len: i64, pitch: u8) -> RenderNote {
        RenderNote {
            start,
            len,
            pitch,
            velocity: 100,
            channel: 0,
        }
    }

    fn pitches(notes: &[RenderNote]) -> Vec<u8> {
        notes.iter().map(|n| n.pitch).collect()
    }

    const C_MAJOR: ScaleSpec = ScaleSpec {
        root: 0,
        mask: 0b1010_1011_0101,
    };

    #[test]
    fn scales_snap_and_step() {
        assert_eq!(C_MAJOR, ScaleSpec::new(0, &[0, 2, 4, 5, 7, 9, 11]));
        assert_eq!(C_MAJOR.snap(61, Snap::Nearest), Some(60));
        assert_eq!(C_MAJOR.snap(61, Snap::Up), Some(62));
        assert_eq!(C_MAJOR.snap(61, Snap::Down), Some(60));
        assert_eq!(C_MAJOR.snap(61, Snap::Leave), None);
        assert_eq!(C_MAJOR.snap(62, Snap::Leave), Some(62));
        // Thirds up from D: F and A.
        assert_eq!(C_MAJOR.step(62, 2), Some(65));
        assert_eq!(C_MAJOR.step(62, 4), Some(69));
        assert_eq!(C_MAJOR.step(127, 4), None);
    }

    #[test]
    fn an_arpeggiator_walks_the_held_chord_in_step() {
        // C E G held for a bar: sixteenths up, then back round.
        let chord = [note(0, 3840, 60), note(0, 3840, 64), note(0, 3840, 67)];
        let got = render(
            &[arp(ArpMode::Up, 1, false)],
            ScaleSpec::CHROMATIC,
            &chord,
            0,
            3840,
        );
        assert_eq!(got.len(), 16);
        assert_eq!(&pitches(&got)[..6], &[60, 64, 67, 60, 64, 67]);
        for (k, n) in got.iter().enumerate() {
            assert_eq!(n.start, k as i64 * 240, "on the sixteenths");
            assert_eq!(n.len, 120, "half a step long");
        }
        let got = render(
            &[arp(ArpMode::Down, 2, false)],
            ScaleSpec::CHROMATIC,
            &chord,
            0,
            1440,
        );
        assert_eq!(pitches(&got), [79, 76, 72, 67, 64, 60]);
        let got = render(
            &[arp(ArpMode::UpDown, 1, false)],
            ScaleSpec::CHROMATIC,
            &chord,
            0,
            1440,
        );
        assert_eq!(pitches(&got), [60, 64, 67, 64, 60, 64]);
        let got = render(
            &[arp(ArpMode::Chord, 1, false)],
            ScaleSpec::CHROMATIC,
            &chord,
            0,
            480,
        );
        assert_eq!(pitches(&got), [60, 64, 67, 60, 64, 67]);
    }

    #[test]
    fn an_arpeggiator_plays_in_the_order_played_and_stops_when_let_go() {
        let notes = [note(0, 960, 67), note(10, 950, 60)];
        let got = render(
            &[arp(ArpMode::AsPlayed, 1, false)],
            ScaleSpec::CHROMATIC,
            &notes,
            0,
            3840,
        );
        // Off the grid, the first key plays at once.
        assert_eq!(pitches(&got), [67, 60, 67, 60]);
        assert_eq!(got[1].start, 240);
        assert!(got.iter().all(|n| n.start < 960));
        // Latched, it carries on after the keys are let go.
        let got = render(
            &[arp(ArpMode::AsPlayed, 1, true)],
            ScaleSpec::CHROMATIC,
            &notes,
            0,
            1920,
        );
        assert_eq!(got.len(), 8);
    }

    #[test]
    fn a_full_gate_ties_each_step_to_the_next() {
        let mut a = arp(ArpMode::Up, 1, false);
        if let NoteFxKind::Arpeggiator { gate, .. } = &mut a.kind {
            *gate = 1.0;
        }
        let got = render(
            &[a],
            ScaleSpec::CHROMATIC,
            &[note(0, 960, 60), note(0, 960, 62)],
            0,
            960,
        );
        assert!(got.iter().all(|n| n.len == 240), "{got:?}");
    }

    #[test]
    fn chords_play_on_each_note_and_end_with_it() {
        let minor = NoteFxSpec {
            id: 2,
            bypass: false,
            kind: NoteFxKind::Chord {
                shape: ChordShape::Fixed {
                    intervals: [3, 7, 0, 0],
                    count: 2,
                },
            },
        };
        let got = render(&[minor], ScaleSpec::CHROMATIC, &[note(0, 480, 57)], 0, 960);
        assert_eq!(pitches(&got), [57, 60, 64]);
        assert!(got.iter().all(|n| n.len == 480));
        let in_key = NoteFxSpec {
            kind: NoteFxKind::Chord {
                shape: ChordShape::InKey { count: 3 },
            },
            ..minor
        };
        // On D in C major: D minor seven.
        let got = render(&[in_key], C_MAJOR, &[note(0, 480, 62)], 0, 960);
        assert_eq!(pitches(&got), [62, 65, 69, 72]);
    }

    #[test]
    fn effects_chain_in_order() {
        // A chord, then arpeggiated: one key plays the chord's notes in
        // turn.
        let chord = NoteFxSpec {
            id: 2,
            bypass: false,
            kind: NoteFxKind::Chord {
                shape: ChordShape::Fixed {
                    intervals: [4, 7, 0, 0],
                    count: 2,
                },
            },
        };
        let got = render(
            &[chord, arp(ArpMode::Up, 1, false)],
            ScaleSpec::CHROMATIC,
            &[note(0, 960, 60)],
            0,
            960,
        );
        assert_eq!(pitches(&got), [60, 64, 67, 60]);
    }

    #[test]
    fn lengths_fixed_at_least_and_longer() {
        let len = |mode| NoteFxSpec {
            id: 3,
            bypass: false,
            kind: NoteFxKind::Length { mode, ticks: 480.0 },
        };
        let notes = [note(0, 100, 60), note(960, 900, 62)];
        let lens = |mode| -> Vec<i64> {
            render(&[len(mode)], ScaleSpec::CHROMATIC, &notes, 0, 3840)
                .iter()
                .map(|n| n.len)
                .collect()
        };
        assert_eq!(lens(LengthMode::Fixed), [480, 480]);
        assert_eq!(lens(LengthMode::AtLeast), [480, 900]);
        assert_eq!(lens(LengthMode::LongerBy), [580, 1380]);
    }

    #[test]
    fn velocity_and_scale_change_notes() {
        let vel = NoteFxSpec {
            id: 4,
            bypass: false,
            kind: NoteFxKind::Velocity {
                scale: 0.5,
                offset: 10,
                min: 1,
                max: 55,
            },
        };
        let got = render(&[vel], ScaleSpec::CHROMATIC, &[note(0, 10, 60)], 0, 100);
        assert_eq!(got[0].velocity, 55);
        let snap = |snap| NoteFxSpec {
            id: 5,
            bypass: false,
            kind: NoteFxKind::Scale { snap },
        };
        let notes = [note(0, 10, 61), note(20, 10, 64)];
        assert_eq!(
            pitches(&render(&[snap(Snap::Up)], C_MAJOR, &notes, 0, 100)),
            [62, 64]
        );
        assert_eq!(
            pitches(&render(&[snap(Snap::Leave)], C_MAJOR, &notes, 0, 100)),
            [64]
        );
    }

    #[test]
    fn randomise_stays_in_range_and_can_leave_notes_out() {
        let rnd = |chance| NoteFxSpec {
            id: 6,
            bypass: false,
            kind: NoteFxKind::Randomise {
                pitch: 2,
                velocity: 10,
                chance,
                in_scale: true,
            },
        };
        let notes: Vec<RenderNote> = (0..200).map(|k| note(k * 10, 5, 64)).collect();
        let got = render(&[rnd(1.0)], C_MAJOR, &notes, 0, 2000);
        assert_eq!(got.len(), 200);
        assert!(
            got.iter()
                .all(|n| (62..=66).contains(&n.pitch) && C_MAJOR.contains(n.pitch as i32))
        );
        assert!(got.iter().all(|n| (90..=110).contains(&n.velocity)));
        assert!(got.iter().any(|n| n.pitch != 64));
        let half = render(&[rnd(0.5)], C_MAJOR, &notes, 0, 2000).len();
        assert!((60..140).contains(&half), "{half}");
        assert!(render(&[rnd(0.0)], C_MAJOR, &notes, 0, 2000).is_empty());
    }

    #[test]
    fn bypassed_effects_pass_notes_through() {
        let mut a = arp(ArpMode::Up, 1, false);
        a.bypass = true;
        let notes = [note(0, 960, 60), note(0, 960, 64)];
        assert_eq!(render(&[a], ScaleSpec::CHROMATIC, &notes, 0, 1920), notes);
    }

    #[test]
    fn every_note_started_is_ended() {
        // Through every effect at once, live-style: each on has its off.
        let specs = [
            NoteFxSpec {
                id: 1,
                bypass: false,
                kind: NoteFxKind::Scale {
                    snap: Snap::Nearest,
                },
            },
            NoteFxSpec {
                id: 2,
                bypass: false,
                kind: NoteFxKind::Chord {
                    shape: ChordShape::InKey { count: 2 },
                },
            },
            arp(ArpMode::Random, 3, false),
            NoteFxSpec {
                id: 4,
                bypass: false,
                kind: NoteFxKind::Length {
                    mode: LengthMode::LongerBy,
                    ticks: 100.0,
                },
            },
        ];
        let mut chain = Chain::new(&specs, C_MAJOR);
        let mut sounding: Vec<u32> = Vec::new();
        let mut events = Vec::with_capacity(MAX_EVENTS);
        for block in 0..400u32 {
            events.clear();
            if block % 7 == 0 && block < 350 {
                events.push(Event::note_on(3, 0, 60 + (block % 12) as u8, 90, block));
            }
            if block % 7 == 3 && (3..=353).contains(&block) {
                let b = block - 3;
                events.push(Event::note_off(5, 0, 60 + (b % 12) as u8, b));
            }
            chain.process(
                &mut events,
                64,
                Clock {
                    at: block as f64 * 64.0 * 0.5,
                    per_frame: 0.5,
                    playing: block < 200,
                },
            );
            assert!(events.windows(2).all(|w| w[0].frame <= w[1].frame));
            for e in &events {
                match e.kind {
                    Kind::NoteOn { id, .. } => sounding.push(id),
                    Kind::NoteOff { id, .. } => {
                        let k = sounding.iter().position(|&s| s == id);
                        assert!(k.is_some(), "an end for a note never started: {e:?}");
                        sounding.swap_remove(k.unwrap());
                    }
                    _ => {}
                }
            }
        }
        assert!(sounding.is_empty(), "left sounding: {sounding:?}");
    }

    #[test]
    fn a_new_chain_takes_over_what_is_sounding() {
        let mut old = Chain::new(&[arp(ArpMode::Up, 1, false)], ScaleSpec::CHROMATIC);
        let mut events = Vec::with_capacity(MAX_EVENTS);
        events.push(Event::note_on(0, 0, 60, 100, 1));
        let clock = Clock {
            at: 0.0,
            per_frame: 1.0,
            playing: true,
        };
        old.process(&mut events, 100, clock);
        assert_eq!(events.len(), 1);
        // A faster rate, same effect: the held key keeps playing.
        let mut faster = arp(ArpMode::Up, 1, false);
        if let NoteFxKind::Arpeggiator { rate, .. } = &mut faster.kind {
            *rate = 120.0;
        }
        let mut new = Chain::new(&[faster], ScaleSpec::CHROMATIC);
        let mut stop = Vec::new();
        assert!(new.adopt(&mut old, &mut stop));
        assert!(stop.is_empty());
        events.clear();
        new.process(&mut events, 400, Clock { at: 100.0, ..clock });
        assert!(events.iter().any(|e| matches!(e.kind, Kind::NoteOn { .. })));
        // A different effect: what was sounding stops.
        let mut other = Chain::new(&[], ScaleSpec::CHROMATIC);
        let mut stop = Vec::with_capacity(16);
        assert!(!other.adopt(&mut new, &mut stop));
        assert!(stop.iter().all(|e| matches!(e.kind, Kind::NoteOff { .. })));
        assert!(!stop.is_empty());
    }
}
