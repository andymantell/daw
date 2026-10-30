// SPDX-License-Identifier: GPL-3.0-or-later
//! The monitor section (roadmap 2.3): what each person hears, apart
//! from the mix itself.
//!
//! - **Headphone mixes** (cue mixes): up to [`MAX_CUES`] stereo mixes,
//!   one per performer, each with its own level and pan for every track
//!   ([`CueSend`], held on the track in the graph) and its own
//!   [`CueBus`] settings: level, mute, how loud the click is, whether
//!   talkback reaches it, and the device outputs it plays on. A mix can
//!   instead be the same as the main mix. Each track's audio is taken
//!   after its effects but before its fader, mute and solo, so changing
//!   the main mix (soloing a part to check it, say) leaves the
//!   performers' mixes alone. It is delayed by the same delay
//!   compensation the master gets, so the headphones and the speakers
//!   are in time with each other.
//! - **Control room** ([`ControlRoom`]): what the speakers play, the main
//!   mix or one of the headphone mixes (to check what a performer
//!   hears), with a level, dim, mono and cut, and up to
//!   [`MAX_SPEAKERS`] sets of speakers to switch between, each with its
//!   own trim. Switching fades the old set out for one block before the
//!   new one fades in, so it never clicks.
//! - **Talkback**: a microphone input sent to the headphone mixes that
//!   take it while held, never to the speakers, which dim meanwhile so
//!   the microphone does not pick them up.
//!
//! Everything here is applied after the mix is made, so exports, stems
//! and meters on the mix are unchanged. With the control room at its
//! defaults the main output is left exactly as the mix made it.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::graph::{Graph, db_to_gain, pan_gains};

/// Most headphone mixes at once.
pub const MAX_CUES: usize = 6;
/// Most sets of speakers to switch between.
pub const MAX_SPEAKERS: usize = 3;
/// A level that is off.
pub const OFF_DB: f32 = -144.0;

/// One track's place in one headphone mix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CueSend {
    pub gain_db: f32,
    /// −1 (left) to +1 (right).
    pub pan: f32,
    pub mute: bool,
}

impl Default for CueSend {
    /// Off: a track is in a headphone mix only once given a level.
    fn default() -> CueSend {
        CueSend::at(OFF_DB)
    }
}

impl CueSend {
    pub fn at(gain_db: f32) -> CueSend {
        CueSend {
            gain_db,
            pan: 0.0,
            mute: false,
        }
    }

    pub(crate) fn gains(&self, stereo: bool) -> (f32, f32) {
        if self.mute {
            return (0.0, 0.0);
        }
        let g = db_to_gain(self.gain_db);
        let (l, r) = pan_gains(self.pan, stereo);
        (g * l, g * r)
    }
}

/// A pair of device output channels (from 0).
pub type OutPair = (u16, u16);

/// One headphone mix's own settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CueBus {
    /// Made at all. Off costs nothing.
    pub on: bool,
    pub gain_db: f32,
    pub mute: bool,
    /// The same as the main mix, rather than its own levels.
    pub follow_main: bool,
    /// How loud the metronome is in it, when the metronome plays; `None`
    /// for no click. A mix that follows the main mix has the main mix's
    /// click instead.
    pub click_db: Option<f32>,
    /// Talkback reaches it.
    pub talkback: bool,
    /// The device outputs it plays on; `None` for nowhere (only its
    /// meter, or the control room listening to it).
    pub out: Option<OutPair>,
}

impl Default for CueBus {
    fn default() -> CueBus {
        CueBus {
            on: false,
            gain_db: 0.0,
            mute: false,
            follow_main: false,
            click_db: Some(0.0),
            talkback: true,
            out: None,
        }
    }
}

impl CueBus {
    fn gain(&self) -> f32 {
        if !self.on || self.mute {
            0.0
        } else {
            db_to_gain(self.gain_db)
        }
    }
}

/// A set of speakers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpeakerSet {
    /// Its device outputs; `None` for the device's main outputs.
    pub out: Option<OutPair>,
    /// Evens out sets that play at different loudness.
    pub trim_db: f32,
}

/// What the control room's speakers play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Listen {
    #[default]
    Main,
    /// A headphone mix, to hear what a performer hears.
    Cue(usize),
}

/// The talkback microphone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Talkback {
    /// The device input channel it is plugged into.
    pub input: Option<u16>,
    pub gain_db: f32,
    /// Held down (or latched) now.
    pub on: bool,
    /// How far the speakers drop while it is on, in dB (positive).
    pub dim_db: f32,
}

impl Default for Talkback {
    fn default() -> Talkback {
        Talkback {
            input: None,
            gain_db: 0.0,
            on: false,
            dim_db: 20.0,
        }
    }
}

/// The speakers' controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlRoom {
    pub listen: Listen,
    pub gain_db: f32,
    /// Drops the speakers by `dim_db` (positive), to talk over them.
    pub dim: bool,
    pub dim_db: f32,
    /// Left and right summed, to check the mix holds up in mono.
    pub mono: bool,
    /// The speakers silenced.
    pub cut: bool,
    pub speakers: [SpeakerSet; MAX_SPEAKERS],
    /// The set playing, by index in `speakers`.
    pub active: usize,
    pub talkback: Talkback,
}

impl Default for ControlRoom {
    fn default() -> ControlRoom {
        ControlRoom {
            listen: Listen::Main,
            gain_db: 0.0,
            dim: false,
            dim_db: 20.0,
            mono: false,
            cut: false,
            speakers: [SpeakerSet::default(); MAX_SPEAKERS],
            active: 0,
            talkback: Talkback::default(),
        }
    }
}

/// Left and right out from left and right in: `[ll, rl, lr, rr]`, where
/// `rl` is how much of the right input goes to the left output.
type Matrix = [f32; 4];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0];

impl ControlRoom {
    /// The matrix the controls ask for, with the speakers at `shown`
    /// (fading to silence while switching away from them).
    fn matrix(&self, shown: usize) -> Matrix {
        let set = self.speakers.get(self.active).copied().unwrap_or_default();
        if self.cut || shown != self.active {
            return [0.0; 4];
        }
        let mut db = self.gain_db + set.trim_db;
        if self.dim {
            db -= self.dim_db.max(0.0);
        }
        if self.talkback.on && self.talkback.input.is_some() {
            db -= self.talkback.dim_db.max(0.0);
        }
        // Exactly unity at 0 dB, so the defaults leave the mix untouched.
        let g = if db == 0.0 { 1.0 } else { db_to_gain(db) };
        if self.mono {
            let h = 0.5 * g;
            [h, h, h, h]
        } else {
            [g, 0.0, 0.0, g]
        }
    }
}

/// Peak levels of each headphone mix since the UI last read them, as
/// `f32` bits, shared with the UI through [`crate::engine::Status`].
pub struct Meters {
    cues: [[AtomicU32; 2]; MAX_CUES],
}

impl Meters {
    pub(crate) fn new() -> Meters {
        Meters {
            cues: std::array::from_fn(|_| [AtomicU32::new(0), AtomicU32::new(0)]),
        }
    }

    /// Peak (left, right) of headphone mix `cue` since the last call.
    pub fn take_cue_peak(&self, cue: usize) -> (f32, f32) {
        self.cues.get(cue).map_or((0.0, 0.0), |p| {
            (
                f32::from_bits(p[0].swap(0, Ordering::Relaxed)),
                f32::from_bits(p[1].swap(0, Ordering::Relaxed)),
            )
        })
    }

    fn raise(&self, cue: usize, l: f32, r: f32) {
        for (slot, v) in self.cues[cue].iter().zip([l, r]) {
            if v.is_finite() && v > 0.0 {
                slot.fetch_max(v.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

/// A delay line lining one track's headphone-mix audio up with the
/// master: as long as the delay compensation after the track's fader.
#[derive(Default)]
pub(crate) struct CueLine {
    line: [Vec<f32>; 2],
    at: usize,
}

impl CueLine {
    pub(crate) fn new(delay: usize) -> CueLine {
        CueLine {
            line: [vec![0.0; delay], vec![0.0; delay]],
            at: 0,
        }
    }

    fn delay(&self) -> usize {
        self.line[0].len()
    }

    /// Delays the block in place.
    pub(crate) fn run(&mut self, l: &mut [f32], r: &mut [f32]) {
        let d = self.delay();
        if d == 0 {
            return;
        }
        for i in 0..l.len() {
            l[i] = std::mem::replace(&mut self.line[0][self.at], l[i]);
            r[i] = std::mem::replace(&mut self.line[1][self.at], r[i]);
            self.at = (self.at + 1) % d;
        }
    }
}

impl Graph {
    /// Adds every track's share of each headphone mix that has its own
    /// levels into `cues`, at frame `at`, for the block [`Graph::render`]
    /// has just made. Uses up the tracks' scratch buffers.
    pub(crate) fn mix_cues(&mut self, n: usize, at: usize, buses: &[CueBus], cues: &mut [Stereo]) {
        let step = 1.0 / n.max(1) as f32;
        for ti in 0..self.tracks.len() {
            let t = &mut self.tracks[ti];
            let [sl, sr] = &mut self.scratch[ti];
            let (sl, sr) = (&mut sl[..n], &mut sr[..n]);
            // After its effects, before its fader: the master hears it
            // this much later.
            t.cue_line.run(sl, sr);
            for (k, (bus, out)) in buses.iter().zip(cues.iter_mut()).enumerate() {
                if !bus.on || bus.follow_main {
                    continue;
                }
                let (l0, r0) = t.cue_last[k];
                let (l1, r1) = t.cues[k].gains(t.stereo);
                t.cue_last[k] = (l1, r1);
                if l0 == 0.0 && r0 == 0.0 && l1 == 0.0 && r1 == 0.0 {
                    continue;
                }
                let [ol, or] = out;
                let (ol, or) = (&mut ol[at..at + n], &mut or[at..at + n]);
                for i in 0..n {
                    let k = (i + 1) as f32 * step;
                    let (a, b) = if t.stereo {
                        (sl[i], sr[i])
                    } else {
                        (sl[i], sl[i])
                    };
                    ol[i] += a * (l0 + (l1 - l0) * k);
                    or[i] += b * (r0 + (r1 - r0) * k);
                }
            }
        }
    }

    /// Changes one track's level in one headphone mix without a new
    /// graph.
    pub(crate) fn set_cue(&mut self, track: usize, cue: usize, send: CueSend) {
        if let Some(t) = self.tracks.get_mut(track)
            && cue < MAX_CUES
        {
            t.cues[cue] = send;
        }
    }

    /// Takes over the headphone-mix delay lines of `old` where a track
    /// keeps the same delay, so a graph change does not drop the audio
    /// in flight on them.
    pub(crate) fn adopt_cue_lines(&mut self, old: &mut Graph) {
        for (t, o) in self.tracks.iter_mut().zip(&mut old.tracks) {
            if t.cue_line.delay() == o.cue_line.delay() {
                std::mem::swap(&mut t.cue_line, &mut o.cue_line);
                t.cue_last = o.cue_last;
            }
        }
    }
}

type Stereo = [Vec<f32>; 2];

/// The monitor section's state on the audio thread. Everything is
/// allocated up front.
pub(crate) struct Monitor {
    pub buses: [CueBus; MAX_CUES],
    pub room: ControlRoom,
    /// Each headphone mix's block, `max_block` long.
    cues: Vec<Stereo>,
    /// Frames of `cues` made this callback.
    len: usize,
    bus_last: [f32; MAX_CUES],
    talk_last: [f32; MAX_CUES],
    matrix: Matrix,
    /// The speakers playing now: `room.active` once the old set has
    /// faded out.
    shown: usize,
    max_block: usize,
}

impl Monitor {
    pub(crate) fn new(max_block: usize) -> Monitor {
        Monitor {
            buses: [CueBus::default(); MAX_CUES],
            room: ControlRoom::default(),
            cues: (0..MAX_CUES)
                .map(|_| [vec![0.0; max_block], vec![0.0; max_block]])
                .collect(),
            len: 0,
            bus_last: [0.0; MAX_CUES],
            talk_last: [0.0; MAX_CUES],
            matrix: IDENTITY,
            shown: 0,
            max_block,
        }
    }

    fn any_cue(&self) -> bool {
        self.buses.iter().any(|b| b.on) || self.bus_last.iter().any(|&g| g != 0.0)
    }

    /// Starts a callback of `n` frames: the headphone mixes start silent.
    pub(crate) fn begin(&mut self, n: usize) {
        self.len = n.min(self.max_block);
        if self.any_cue() {
            for [l, r] in &mut self.cues {
                l[..self.len].fill(0.0);
                r[..self.len].fill(0.0);
            }
        }
    }

    /// Adds the block the graph has just made, `n` frames at `at` of the
    /// callback, to the headphone mixes.
    pub(crate) fn add_tracks(&mut self, g: &mut Graph, at: usize, n: usize) {
        if !self.any_cue() || at + n > self.len {
            return;
        }
        g.mix_cues(n, at, &self.buses, &mut self.cues);
    }

    /// Adds clicks for frames `at..` to the headphone mixes that want
    /// them.
    pub(crate) fn add_click(&mut self, at: usize, clicks: &[f32]) {
        if at + clicks.len() > self.len {
            return;
        }
        for (bus, [l, r]) in self.buses.iter().zip(&mut self.cues) {
            let Some(db) = bus.click_db.filter(|_| bus.on && !bus.follow_main) else {
                continue;
            };
            let g = db_to_gain(db);
            for (i, x) in clicks.iter().enumerate() {
                l[at + i] += x * g;
                r[at + i] += x * g;
            }
        }
    }

    /// Finishes the callback: headphone mixes that follow the main mix
    /// copy it, talkback goes in, each mix gets its own level, and the
    /// main output becomes what the control room's speakers play.
    pub(crate) fn finish(
        &mut self,
        input: &[f32],
        channels: usize,
        left: &mut [f32],
        right: &mut [f32],
        meters: &Meters,
    ) {
        let n = left.len().min(right.len());
        let step = 1.0 / n.max(1) as f32;
        if self.any_cue() && n <= self.len {
            let tb = self.room.talkback;
            let mic = tb.input.map(usize::from).filter(|&c| c < channels);
            for k in 0..MAX_CUES {
                let bus = self.buses[k];
                let [cl, cr] = &mut self.cues[k];
                let (cl, cr) = (&mut cl[..n], &mut cr[..n]);
                if bus.on && bus.follow_main {
                    cl.copy_from_slice(&left[..n]);
                    cr.copy_from_slice(&right[..n]);
                }
                // Talkback, ramped in and out.
                let t1 = match mic {
                    Some(_) if bus.on && bus.talkback && tb.on => db_to_gain(tb.gain_db),
                    _ => 0.0,
                };
                let t0 = std::mem::replace(&mut self.talk_last[k], t1);
                if let Some(ch) = mic
                    && (t0 != 0.0 || t1 != 0.0)
                {
                    for i in 0..n {
                        let x = input.get(i * channels + ch).copied().unwrap_or(0.0);
                        let x = x * (t0 + (t1 - t0) * (i + 1) as f32 * step);
                        cl[i] += x;
                        cr[i] += x;
                    }
                }
                let g1 = bus.gain();
                let g0 = std::mem::replace(&mut self.bus_last[k], g1);
                let (mut pl, mut pr) = (0.0f32, 0.0f32);
                for i in 0..n {
                    let g = g0 + (g1 - g0) * (i + 1) as f32 * step;
                    cl[i] *= g;
                    cr[i] *= g;
                    pl = pl.max(cl[i].abs());
                    pr = pr.max(cr[i].abs());
                }
                meters.raise(k, pl, pr);
            }
        }
        self.control_room(left, right, n);
    }

    fn control_room(&mut self, left: &mut [f32], right: &mut [f32], n: usize) {
        let m1 = self.room.matrix(self.shown);
        let m0 = std::mem::replace(&mut self.matrix, m1);
        let source = match self.room.listen {
            Listen::Cue(k) if k < MAX_CUES && n <= self.len => Some(k),
            _ => None,
        };
        if let Some(k) = source {
            left[..n].copy_from_slice(&self.cues[k][0][..n]);
            right[..n].copy_from_slice(&self.cues[k][1][..n]);
        }
        if m0 != IDENTITY || m1 != IDENTITY {
            let step = 1.0 / n.max(1) as f32;
            for i in 0..n {
                let k = (i + 1) as f32 * step;
                let m: [f32; 4] = std::array::from_fn(|j| m0[j] + (m1[j] - m0[j]) * k);
                let (a, b) = (left[i], right[i]);
                left[i] = a * m[0] + b * m[1];
                right[i] = a * m[2] + b * m[3];
            }
        }
        // Faded out: switch to the new speakers, which fade in next.
        if self.shown != self.room.active && self.matrix == [0.0; 4] {
            self.shown = self.room.active;
        }
    }

    /// The device outputs the main pair plays on now, if the speakers
    /// chosen are not the device's main outputs.
    pub(crate) fn main_out(&self) -> Option<OutPair> {
        self.room.speakers.get(self.shown).and_then(|s| s.out)
    }

    /// Calls `f` with each device output channel a headphone mix plays
    /// on, and its samples for the callback just made.
    pub(crate) fn each_cue_out(&self, f: &mut dyn FnMut(usize, &[f32])) {
        for (bus, [l, r]) in self.buses.iter().zip(&self.cues) {
            if let Some((a, b)) = bus.out.filter(|_| bus.on) {
                f(a as usize, &l[..self.len]);
                f(b as usize, &r[..self.len]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_the_main_output_alone() {
        let mut m = Monitor::new(64);
        let meters = Meters::new();
        let mut l: Vec<f32> = (0..64).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut r: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).cos()).collect();
        let (l0, r0) = (l.clone(), r.clone());
        m.begin(64);
        m.finish(&[], 0, &mut l, &mut r, &meters);
        assert_eq!((l, r), (l0, r0));
    }

    #[test]
    fn dim_mono_and_cut() {
        let mut m = Monitor::new(64);
        let meters = Meters::new();
        let run = |m: &mut Monitor| {
            // Twice, so the second block has settled after the ramp.
            let mut out = (vec![0.0; 64], vec![0.0; 64]);
            for _ in 0..2 {
                out = (vec![1.0f32; 64], vec![0.0f32; 64]);
                m.begin(64);
                m.finish(&[], 0, &mut out.0, &mut out.1, &meters);
            }
            (out.0[63], out.1[63])
        };
        m.room.dim = true;
        let (l, r) = run(&mut m);
        assert!((l - db_to_gain(-20.0)).abs() < 1e-6 && r == 0.0);
        m.room.dim = false;
        m.room.mono = true;
        assert_eq!(run(&mut m), (0.5, 0.5));
        m.room.cut = true;
        assert_eq!(run(&mut m), (0.0, 0.0));
    }

    #[test]
    fn switching_speakers_fades_out_before_moving() {
        let mut m = Monitor::new(16);
        let meters = Meters::new();
        m.room.speakers[1].out = Some((2, 3));
        m.room.active = 1;
        let (mut l, mut r) = (vec![1.0f32; 16], vec![1.0f32; 16]);
        m.begin(16);
        m.finish(&[], 0, &mut l, &mut r, &meters);
        // The first block fades out on the old speakers ...
        assert!(l[0] > 0.9 && l[15] == 0.0);
        assert_eq!(m.main_out(), Some((2, 3)));
        // ... and the next fades in on the new ones.
        let (mut l, mut r) = (vec![1.0f32; 16], vec![1.0f32; 16]);
        m.begin(16);
        m.finish(&[], 0, &mut l, &mut r, &meters);
        assert!(l[0] < 0.1 && l[15] == 1.0);
    }

    #[test]
    fn talkback_reaches_the_headphones_not_the_speakers() {
        let mut m = Monitor::new(32);
        let meters = Meters::new();
        m.buses[0] = CueBus {
            on: true,
            ..CueBus::default()
        };
        m.buses[1] = CueBus {
            on: true,
            talkback: false,
            ..CueBus::default()
        };
        m.room.talkback = Talkback {
            input: Some(1),
            on: true,
            ..Talkback::default()
        };
        // Input channel 1 is the microphone, at 0.5.
        let input: Vec<f32> = (0..32).flat_map(|_| [0.0, 0.5]).collect();
        let mut last = (0.0, 0.0, 0.0);
        for _ in 0..2 {
            let (mut l, mut r) = (vec![1.0f32; 32], vec![1.0f32; 32]);
            m.begin(32);
            m.finish(&input, 2, &mut l, &mut r, &meters);
            last = (m.cues[0][0][31], m.cues[1][0][31], l[31]);
        }
        assert_eq!(last.0, 0.5);
        assert_eq!(last.1, 0.0);
        // The speakers drop by the talkback dim.
        assert!((last.2 - db_to_gain(-20.0)).abs() < 1e-6);
    }
}
