// SPDX-License-Identifier: GPL-3.0-or-later
//! Layered instruments (roadmap 3.5): several instruments on one track
//! sharing its notes, like an effect container whose chains each start
//! with an instrument.
//!
//! Every layer hears the track's notes and plays the ones in its key and
//! velocity ranges, moved by its transpose, then runs them through its
//! own effects (the track's inserts whose [`Place::container`] is the
//! layer's id) and is mixed in at its own level and pan. The mixed
//! layers then go through the track's own chain. Each layer is delayed
//! to match the slowest (its instrument's latency and its effects'), so
//! they stay lined up.
//!
//! A layer remembers which notes it started, by channel and pitch, so a
//! note-off reaches the layer whose note-on did, at the pitch it was
//! sent at, even after its ranges or transpose change.
//!
//! [`Place::container`]: crate::graph::Place::container

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use crate::chain::{self, Keys, LayerChain, Step};
use crate::graph::{InsertNode, InstrumentNode, InstrumentSpec, db_to_gain, pan_gains};
use crate::midi::{Event, Instrument, Kind, MAX_EVENTS};
use crate::monitor::CueLine;

/// The most semitones a layer's notes move by, either way.
pub const MAX_TRANSPOSE: i32 = 48;

/// A layer's settings, shared with the audio thread: they take effect
/// on the next block, without a new graph.
#[derive(Debug)]
pub struct LayerControls {
    gain_db: AtomicU32,
    pan: AtomicU32,
    mute: AtomicBool,
    /// Lowest note in the low byte, highest in the next.
    keys: AtomicU32,
    velocities: AtomicU32,
    transpose: AtomicI32,
}

impl Default for LayerControls {
    fn default() -> Self {
        LayerControls {
            gain_db: AtomicU32::new(0f32.to_bits()),
            pan: AtomicU32::new(0f32.to_bits()),
            mute: AtomicBool::new(false),
            keys: AtomicU32::new(pair(0, 127)),
            velocities: AtomicU32::new(pair(1, 127)),
            transpose: AtomicI32::new(0),
        }
    }
}

fn pair(lo: u8, hi: u8) -> u32 {
    u32::from(lo) | u32::from(hi) << 8
}

fn unpair(v: u32) -> (u8, u8) {
    (v as u8, (v >> 8) as u8)
}

impl LayerControls {
    pub fn gain_db(&self) -> f32 {
        f32::from_bits(self.gain_db.load(Ordering::Relaxed))
    }

    pub fn set_gain_db(&self, db: f32) {
        let db = if db.is_finite() { db } else { 0.0 };
        self.gain_db.store(db.to_bits(), Ordering::Relaxed);
    }

    /// −1 (left) to 1 (right).
    pub fn pan(&self) -> f32 {
        f32::from_bits(self.pan.load(Ordering::Relaxed))
    }

    pub fn set_pan(&self, pan: f32) {
        let pan = if pan.is_finite() {
            pan.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.pan.store(pan.to_bits(), Ordering::Relaxed);
    }

    pub fn mute(&self) -> bool {
        self.mute.load(Ordering::Relaxed)
    }

    pub fn set_mute(&self, mute: bool) {
        self.mute.store(mute, Ordering::Relaxed);
    }

    /// The lowest and highest notes it plays, 0 to 127.
    pub fn keys(&self) -> (u8, u8) {
        unpair(self.keys.load(Ordering::Relaxed))
    }

    pub fn set_keys(&self, low: u8, high: u8) {
        self.keys
            .store(pair(low.min(127), high.min(127)), Ordering::Relaxed);
    }

    /// The softest and loudest notes it plays, 1 to 127.
    pub fn velocities(&self) -> (u8, u8) {
        unpair(self.velocities.load(Ordering::Relaxed))
    }

    pub fn set_velocities(&self, low: u8, high: u8) {
        let v = |x: u8| x.clamp(1, 127);
        self.velocities
            .store(pair(v(low), v(high)), Ordering::Relaxed);
    }

    /// Semitones its notes are moved by.
    pub fn transpose(&self) -> i32 {
        self.transpose.load(Ordering::Relaxed)
    }

    pub fn set_transpose(&self, semitones: i32) {
        let t = semitones.clamp(-MAX_TRANSPOSE, MAX_TRANSPOSE);
        self.transpose.store(t, Ordering::Relaxed);
    }

    fn ranges(&self) -> Ranges {
        Ranges {
            keys: self.keys(),
            velocities: self.velocities(),
            transpose: self.transpose(),
        }
    }
}

/// One layer of a track's layered instrument.
#[derive(Clone, Debug)]
pub struct LayerSpec {
    /// Unique for the life of the process. The layer's effects name it
    /// as their container (in chain 0).
    pub id: u64,
    /// The layer's instrument, handed to the graph like a track's;
    /// `None` for a layer that plays nothing yet.
    pub instrument: Option<InstrumentSpec>,
    pub controls: Arc<LayerControls>,
}

impl LayerSpec {
    pub fn new(id: u64, instrument: Option<InstrumentSpec>) -> LayerSpec {
        LayerSpec {
            id,
            instrument,
            controls: Arc::new(LayerControls::default()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Ranges {
    keys: (u8, u8),
    velocities: (u8, u8),
    transpose: i32,
}

/// The notes a layer has started and not yet stopped: how many at each
/// channel and pitch, and the pitch they were sent at.
pub(crate) struct Held {
    count: [[u8; 128]; 16],
    sent: [[u8; 128]; 16],
}

impl Held {
    fn new() -> Box<Held> {
        Box::new(Held {
            count: [[0; 128]; 16],
            sent: [[0; 128]; 16],
        })
    }

    fn clear(&mut self) {
        self.count = [[0; 128]; 16];
    }

    /// The event as this layer plays it, or `None` if it isn't for this
    /// layer. `shift` is the practice transpose already applied to the
    /// track's notes: ranges are of the notes as written or played.
    fn filter(&mut self, e: &Event, r: Ranges, shift: i32) -> Option<Event> {
        let ch = (e.channel & 15) as usize;
        let mut out = *e;
        match &mut out.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => {
                let p = *pitch as usize & 127;
                let written = i32::from(*pitch) - shift;
                let keys = i32::from(r.keys.0)..=i32::from(r.keys.1);
                if !keys.contains(&written) || !(r.velocities.0..=r.velocities.1).contains(velocity)
                {
                    return None;
                }
                let count = &mut self.count[ch][p];
                if *count == 0 {
                    let to = i32::from(*pitch) + r.transpose;
                    if !(0..=127).contains(&to) {
                        return None;
                    }
                    self.sent[ch][p] = to as u8;
                } else if *count == u8::MAX {
                    return None;
                }
                *count += 1;
                *pitch = self.sent[ch][p];
            }
            Kind::NoteOff { pitch, .. } => {
                let p = *pitch as usize & 127;
                let count = &mut self.count[ch][p];
                if *count == 0 {
                    return None;
                }
                *count -= 1;
                *pitch = self.sent[ch][p];
            }
            Kind::KeyPressure { pitch, .. } | Kind::NoteExpression { pitch, .. } => {
                let p = *pitch as usize & 127;
                if self.count[ch][p] == 0 {
                    return None;
                }
                *pitch = self.sent[ch][p];
            }
            Kind::AllNotesOff => self.clear(),
            Kind::Control { .. } | Kind::PitchBend(_) | Kind::Pressure(_) | Kind::Program(_) => {}
        }
        Some(out)
    }
}

pub(crate) struct LayerNode {
    pub instrument: Option<InstrumentNode>,
    controls: Arc<LayerControls>,
    /// This block's events for the layer; capacity [`MAX_EVENTS`].
    events: Vec<Event>,
    pub held: Box<Held>,
    steps: Vec<Step>,
    /// Lines the layer up with the slowest one.
    line: CueLine,
    buf: [Vec<f32>; 2],
    /// Gains reached at the end of the last block, for smoothing.
    last: (f32, f32),
}

impl LayerNode {
    pub fn new(
        spec: &LayerSpec,
        chain: LayerChain,
        new_instruments: &mut Vec<(u64, Box<dyn Instrument>)>,
        max_block: usize,
    ) -> LayerNode {
        let gain = start_gains(&spec.controls);
        LayerNode {
            instrument: spec.instrument.map(|i| InstrumentNode {
                id: i.id,
                inst: new_instruments
                    .iter()
                    .position(|(id, _)| *id == i.id)
                    .map(|k| new_instruments.swap_remove(k).1),
            }),
            controls: spec.controls.clone(),
            events: Vec::with_capacity(MAX_EVENTS),
            held: Held::new(),
            steps: chain.steps,
            line: chain.line,
            buf: [vec![0.0; max_block], vec![0.0; max_block]],
            last: gain,
        }
    }
}

fn start_gains(c: &LayerControls) -> (f32, f32) {
    if c.mute() {
        return (0.0, 0.0);
    }
    let g = db_to_gain(c.gain_db());
    let (l, r) = pan_gains(c.pan(), true);
    (g * l, g * r)
}

/// Plays the block's `events` through every layer, adding each layer's
/// audio to `l` and `r`.
pub(crate) fn run(
    layers: &mut [LayerNode],
    events: &[Event],
    shift: i32,
    inserts: &mut [InsertNode],
    keys: &mut Keys,
    l: &mut [f32],
    r: &mut [f32],
) {
    let n = l.len();
    for layer in layers {
        let ranges = layer.controls.ranges();
        layer.events.clear();
        for e in events {
            if layer.events.len() < MAX_EVENTS
                && let Some(x) = layer.held.filter(e, ranges, shift)
            {
                layer.events.push(x);
            }
        }
        let [bl, br] = &mut layer.buf;
        let (bl, br) = (&mut bl[..n], &mut br[..n]);
        bl.fill(0.0);
        br.fill(0.0);
        if let Some(InstrumentNode {
            inst: Some(inst), ..
        }) = &mut layer.instrument
        {
            inst.process(&layer.events, bl, br);
        }
        chain::run(&mut layer.steps, inserts, keys, bl, br);
        layer.line.run(bl, br);
        // Ramp from last block's gains so changes don't click.
        let target = start_gains(&layer.controls);
        let (l0, r0) = layer.last;
        let step = 1.0 / n.max(1) as f32;
        for i in 0..n {
            let k = (i + 1) as f32 * step;
            l[i] += bl[i] * (l0 + (target.0 - l0) * k);
            r[i] += br[i] * (r0 + (target.1 - r0) * k);
        }
        layer.last = target;
    }
}

/// Plays `events` (notes stopping, say) through every layer without
/// audio, as a graph is taken over.
pub(crate) fn stop(layers: &mut [LayerNode], events: &[Event]) {
    for layer in layers {
        let ranges = layer.controls.ranges();
        layer.events.clear();
        for e in events {
            if layer.events.len() < MAX_EVENTS
                && let Some(x) = layer.held.filter(e, ranges, 0)
            {
                layer.events.push(x);
            }
        }
        if !layer.events.is_empty()
            && let Some(InstrumentNode {
                inst: Some(inst), ..
            }) = &mut layer.instrument
        {
            inst.process(&layer.events, &mut [], &mut []);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Ranges {
        Ranges {
            keys: (0, 127),
            velocities: (1, 127),
            transpose: 0,
        }
    }

    fn pitch(e: Option<Event>) -> Option<u8> {
        match e?.kind {
            Kind::NoteOn { pitch, .. } | Kind::NoteOff { pitch, .. } => Some(pitch),
            _ => None,
        }
    }

    #[test]
    fn notes_outside_the_ranges_are_left_out() {
        let mut h = Held::new();
        let r = Ranges {
            keys: (60, 71),
            velocities: (64, 127),
            ..all()
        };
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 59, 100, 1), r, 0)),
            None
        );
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 72, 100, 2), r, 0)),
            None
        );
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 60, 63, 3), r, 0)),
            None
        );
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 60, 64, 4), r, 0)),
            Some(60)
        );
        // Only the note it started is stopped.
        assert_eq!(pitch(h.filter(&Event::note_off(0, 0, 59, 1), r, 0)), None);
        assert_eq!(
            pitch(h.filter(&Event::note_off(0, 0, 60, 4), r, 0)),
            Some(60)
        );
        assert_eq!(pitch(h.filter(&Event::note_off(0, 0, 60, 4), r, 0)), None);
    }

    #[test]
    fn a_note_stops_at_the_pitch_it_started_at() {
        let mut h = Held::new();
        let up = Ranges {
            transpose: 12,
            ..all()
        };
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 60, 100, 1), up, 0)),
            Some(72)
        );
        // The transpose and ranges change while it sounds.
        let down = Ranges {
            keys: (0, 10),
            transpose: -12,
            ..all()
        };
        assert_eq!(
            pitch(h.filter(&Event::note_off(0, 0, 60, 1), down, 0)),
            Some(72)
        );
        // Moved past the top: not played.
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 120, 100, 2), up, 0)),
            None
        );
    }

    #[test]
    fn ranges_are_of_the_notes_as_written_under_practice_transpose() {
        let mut h = Held::new();
        let low = Ranges {
            keys: (0, 59),
            ..all()
        };
        // Written 58, played two semitones up for practice.
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 60, 100, 1), low, 2)),
            Some(60)
        );
        assert_eq!(
            pitch(h.filter(&Event::note_on(0, 0, 62, 100, 2), low, 2)),
            None
        );
    }

    #[test]
    fn all_notes_off_forgets_what_was_held() {
        let mut h = Held::new();
        h.filter(&Event::note_on(0, 3, 60, 100, 1), all(), 0);
        assert!(h.filter(&Event::all_notes_off(0), all(), 0).is_some());
        assert_eq!(
            pitch(h.filter(&Event::note_off(0, 3, 60, 1), all(), 0)),
            None
        );
    }

    #[test]
    fn controls_keep_to_their_ranges() {
        let c = LayerControls::default();
        assert_eq!(
            (c.keys(), c.velocities(), c.transpose()),
            ((0, 127), (1, 127), 0)
        );
        c.set_keys(200, 36);
        c.set_velocities(0, 90);
        c.set_transpose(99);
        c.set_pan(3.0);
        assert_eq!(c.keys(), (127, 36));
        assert_eq!(c.velocities(), (1, 90));
        assert_eq!(c.transpose(), MAX_TRANSPOSE);
        assert_eq!(c.pan(), 1.0);
    }
}
