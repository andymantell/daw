// SPDX-License-Identifier: GPL-3.0-or-later
//! A drum machine (roadmap 3.4): sixteen pads, one for each key from C1
//! (note 36, the kick in the standard drum key map). Each pad layers up
//! to four sounds, samples or drums made here ([`crate::drum_voice`]),
//! each played over a range of velocities, so a soft hit and a hard one
//! can sound different. A pad has its own level, pan and tuning, a
//! choke group (an open hi-hat stops when the closed one is hit), its
//! own chain of effects, an output of its own (to a bus in the mixer)
//! and two sends.

use std::sync::Arc;

use daw_engine::graph::Insert;
use daw_engine::midi::{Event, Instrument, Kind, Outputs};

use crate::drum_voice::{Drum, Hit};
use crate::dsp::{db, fall};
use crate::params::{Param, Params, Unit};
use crate::sample::{Player, Sample};

pub const PADS: usize = 16;
pub const LAYERS: usize = 4;
/// The key of the first pad.
pub const FIRST_NOTE: u8 = 36;
/// Outputs a pad can go to, besides the track: 1 to 16.
pub const OUTPUTS: usize = 16;
/// The extra outputs the two sends come out of.
pub const SEND_A: usize = 17;
pub const SEND_B: usize = 18;
/// Choke groups, from 1 (0 is none).
pub const CHOKES: usize = 8;

/// What a layer plays: nothing, its sample, or one of the drums made
/// here (from index 2, in the order of [`Drum::ALL`]).
pub const SOURCES: &[&str] = &[
    "Off",
    "Sample",
    "Kick",
    "Snare",
    "Clap",
    "Closed hi-hat",
    "Open hi-hat",
    "Tom",
    "Rim",
    "Cymbal",
    "Cowbell",
];
pub const SAMPLE: f64 = 1.0;
pub const MODES: &[&str] = &["Play to the end", "Stop with the note"];

pub mod at {
    pub const LEVEL: usize = 0;
    pub const VELOCITY: usize = 1;
    /// Where the pads' settings start; [`PAD`](super::PAD) each.
    pub const PADS: usize = 2;
}

/// A pad's settings, from its first.
pub mod pad {
    pub const LEVEL: usize = 0;
    pub const PAN: usize = 1;
    pub const TUNE: usize = 2;
    pub const CHOKE: usize = 3;
    pub const MODE: usize = 4;
    pub const OUTPUT: usize = 5;
    pub const SEND_A: usize = 6;
    pub const SEND_B: usize = 7;
    /// Where its layers' settings start; [`LAYER`](super::LAYER) each.
    pub const LAYERS: usize = 8;
}

/// A layer's settings, from its first.
pub mod layer {
    pub const SOURCE: usize = 0;
    pub const LEVEL: usize = 1;
    pub const TUNE: usize = 2;
    pub const PAN: usize = 3;
    pub const DECAY: usize = 4;
    pub const TONE: usize = 5;
    pub const VEL_LO: usize = 6;
    pub const VEL_HI: usize = 7;
    pub const START: usize = 8;
}

pub const LAYER: usize = 9;
pub const PAD: usize = pad::LAYERS + LAYERS * LAYER;
/// The longest decay; at it, a sample plays without fading.
pub const NO_FADE: f64 = 10.0;

/// The index of pad `p`'s setting `k`.
pub fn pad_at(p: usize, k: usize) -> usize {
    at::PADS + p * PAD + k
}

/// The index of pad `p`'s layer `l`'s setting `k`.
pub fn layer_at(p: usize, l: usize, k: usize) -> usize {
    pad_at(p, pad::LAYERS + l * LAYER + k)
}

/// Where a layer's sample is kept, by name, for saving.
pub fn sample_key(p: usize, l: usize) -> String {
    format!("pad{p}.layer{l}")
}

pub fn params() -> Vec<Param> {
    use Unit::*;
    let mut v = vec![
        Param::new("level", "Level", -36.0, 12.0, 0.0, Db),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.8, Amount),
    ];
    for p in 0..PADS {
        let k = |s: &str| format!("pad{p}.{s}");
        v.extend([
            Param::new(k("level"), "Level", -60.0, 12.0, 0.0, Db),
            Param::new(k("pan"), "Pan", -1.0, 1.0, 0.0, Pan),
            Param::new(k("tune"), "Tune", -24.0, 24.0, 0.0, Semitones),
            Param::new(k("choke"), "Choke group", 0.0, CHOKES as f64, 0.0, Count),
            Param::new(k("mode"), "Notes", 0.0, 1.0, 0.0, Choice).choices(MODES),
            Param::new(k("output"), "Output", 0.0, OUTPUTS as f64, 0.0, Count),
            Param::new(k("send_a"), "Send A", -60.0, 6.0, -60.0, Db),
            Param::new(k("send_b"), "Send B", -60.0, 6.0, -60.0, Db),
        ]);
        for l in 0..LAYERS {
            let k = |s: &str| format!("pad{p}.layer{l}.{s}");
            v.extend([
                Param::new(k("source"), "Sound", 0.0, 0.0, 0.0, Choice).choices(SOURCES),
                Param::new(k("level"), "Level", -60.0, 12.0, 0.0, Db),
                Param::new(k("tune"), "Tune", -24.0, 24.0, 0.0, Semitones),
                Param::new(k("pan"), "Pan", -1.0, 1.0, 0.0, Pan),
                Param::new(k("decay"), "Decay", 0.01, NO_FADE, NO_FADE, Seconds).log(),
                Param::new(k("tone"), "Tone", 0.0, 1.0, 0.5, Amount),
                Param::new(k("vel_lo"), "Softest", 1.0, 127.0, 1.0, Count),
                Param::new(k("vel_hi"), "Hardest", 1.0, 127.0, 127.0, Count),
                Param::new(k("start"), "Start", 0.0, 0.95, 0.0, Amount),
            ]);
        }
    }
    v
}

/// What each pad of the kit made here is, in the standard drum key map
/// from note 36: (name, drum, tune, decay, tone).
const KIT: [(&str, Drum, f64, f64, f64); PADS] = [
    ("Kick", Drum::Kick, 0.0, 0.6, 0.5),
    ("Rim", Drum::Rim, 0.0, 0.06, 0.5),
    ("Snare", Drum::Snare, 0.0, 0.3, 0.5),
    ("Clap", Drum::Clap, 0.0, 0.35, 0.5),
    ("Snare 2", Drum::Snare, 2.0, 0.25, 0.8),
    ("Low tom", Drum::Tom, -7.0, 0.7, 0.3),
    ("Closed hi-hat", Drum::ClosedHat, 0.0, 0.08, 0.5),
    ("Low tom 2", Drum::Tom, -4.0, 0.65, 0.3),
    ("Pedal hi-hat", Drum::ClosedHat, -2.0, 0.05, 0.3),
    ("Mid tom", Drum::Tom, 0.0, 0.6, 0.3),
    ("Open hi-hat", Drum::OpenHat, 0.0, 0.6, 0.5),
    ("Mid tom 2", Drum::Tom, 3.0, 0.55, 0.3),
    ("High tom", Drum::Tom, 6.0, 0.5, 0.3),
    ("Crash", Drum::Cymbal, 0.0, 2.0, 0.6),
    ("High tom 2", Drum::Tom, 9.0, 0.45, 0.3),
    ("Ride", Drum::Cymbal, 5.0, 3.0, 0.15),
];

/// The pads' names in the kit made here.
pub fn kit_name(p: usize) -> &'static str {
    KIT.get(p).map_or("Pad", |k| k.0)
}

/// Sets up the kit made here: a drum on each pad, the hi-hats choking
/// each other, and the snare layered with a quiet clap.
pub fn default_kit(params: &Params) {
    for (p, &(_, drum, tune, decay, tone)) in KIT.iter().enumerate() {
        params.set(layer_at(p, 0, layer::SOURCE), 2.0 + drum as usize as f64);
        params.set(layer_at(p, 0, layer::TUNE), tune);
        params.set(layer_at(p, 0, layer::DECAY), decay);
        params.set(layer_at(p, 0, layer::TONE), tone);
        if matches!(drum, Drum::ClosedHat | Drum::OpenHat) {
            params.set(pad_at(p, pad::CHOKE), 1.0);
        }
    }
    params.set(
        layer_at(2, 1, layer::SOURCE),
        2.0 + Drum::Clap as usize as f64,
    );
    params.set(layer_at(2, 1, layer::LEVEL), -12.0);
    params.set(layer_at(2, 1, layer::DECAY), 0.2);
    params.set(pad_at(13, pad::LEVEL), -4.0);
    params.set(pad_at(15, pad::LEVEL), -6.0);
}

/// Frames worked at a time: settings are read and pads mixed this often.
const CHUNK: usize = 64;
const VOICES: usize = 64;
/// How fast a choked sound fades, and one stopped with its note.
const CHOKE_SECONDS: f32 = 0.004;
const STOP_SECONDS: f32 = 0.03;

#[derive(Clone, Copy)]
enum Src {
    Sample(Player),
    Drum(Hit),
}

#[derive(Clone, Copy)]
struct Voice {
    on: bool,
    pad: usize,
    layer: usize,
    note: u8,
    held: bool,
    src: Src,
    gain: (f32, f32),
    level: f32,
    /// How fast the level falls each sample: a sample's decay, or a
    /// cut-off's.
    fade: f32,
    age: u64,
}

/// The drum machine on the audio thread.
pub struct DrumMachine {
    params: Arc<Params>,
    /// Each layer's sample at the output's rate, `pad * LAYERS + layer`.
    samples: Vec<Option<Arc<Sample>>>,
    /// Each pad's effects, in order.
    fx: Vec<Vec<Box<dyn Insert>>>,
    rate: f32,
    voices: Vec<Voice>,
    /// Each pad's sound this chunk.
    bufs: Vec<[[f32; CHUNK]; 2]>,
    clock: u64,
    seed: u32,
}

impl DrumMachine {
    /// `fx` is each pad's effects, in order; there must be [`PADS`]
    /// lists, or fewer for pads with none.
    pub fn new(
        rate: u32,
        params: Arc<Params>,
        samples: Vec<Option<Arc<Sample>>>,
        mut fx: Vec<Vec<Box<dyn Insert>>>,
    ) -> DrumMachine {
        fx.resize_with(PADS, Vec::new);
        DrumMachine {
            params,
            samples,
            fx,
            rate: rate.max(1) as f32,
            voices: Vec::with_capacity(VOICES),
            bufs: vec![[[0.0; CHUNK]; 2]; PADS],
            clock: 0,
            seed: 1,
        }
    }

    fn hit(&mut self, note: u8, velocity: u8) {
        let Some(p) = (note as usize)
            .checked_sub(FIRST_NOTE as usize)
            .filter(|&p| p < PADS)
        else {
            return;
        };
        let pr = &self.params;
        let r = self.rate;
        // Choke: the other pads in its group stop.
        let group = pr.read(pad_at(p, pad::CHOKE)) as u32;
        if group > 0 {
            let cut = fall(CHOKE_SECONDS, r);
            for v in self.voices.iter_mut() {
                if v.pad != p && pr.read(pad_at(v.pad, pad::CHOKE)) as u32 == group {
                    v.fade = v.fade.min(cut);
                }
            }
        }
        let sens = pr.read(at::VELOCITY);
        let vel = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
        let pad_tune = pr.read(pad_at(p, pad::TUNE));
        for l in 0..LAYERS {
            let k = |x| pr.read(layer_at(p, l, x));
            let source = k(layer::SOURCE) as usize;
            let v = velocity as f32;
            if source == 0 || v < k(layer::VEL_LO) || v > k(layer::VEL_HI) {
                continue;
            }
            let tune = pad_tune + k(layer::TUNE);
            let decay = k(layer::DECAY);
            let src = if source == SAMPLE as usize {
                let Some(s) = self.samples.get(p * LAYERS + l).and_then(|s| s.as_deref()) else {
                    continue;
                };
                let len = s.len() as f64;
                let start = k(layer::START) as f64 * len;
                Src::Sample(Player::new(start, len, (tune as f64 / 12.0).exp2()))
            } else {
                let Some(d) = Drum::from_index(source - 2) else {
                    continue;
                };
                self.seed = self.seed.wrapping_add(1);
                Src::Drum(Hit::new(d, r, tune, decay, k(layer::TONE), self.seed))
            };
            let fade = if source == SAMPLE as usize && (decay as f64) < NO_FADE {
                fall(decay, r)
            } else {
                1.0
            };
            let g = db(k(layer::LEVEL)) * vel;
            let (gl, gr) = crate::dsp::pan(k(layer::PAN));
            self.clock += 1;
            let voice = Voice {
                on: true,
                pad: p,
                layer: l,
                note,
                held: true,
                src,
                gain: (g * gl, g * gr),
                level: 1.0,
                fade,
                age: self.clock,
            };
            if self.voices.len() < VOICES {
                self.voices.push(voice);
            } else if let Some(old) = (0..self.voices.len()).min_by_key(|&i| self.voices[i].age) {
                self.voices[old] = voice;
            }
        }
    }

    fn event(&mut self, e: &Event) {
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.hit(pitch, velocity),
            Kind::NoteOff { pitch, .. } => {
                let stop = fall(STOP_SECONDS, self.rate);
                for v in self.voices.iter_mut().filter(|v| v.note == pitch && v.held) {
                    v.held = false;
                    if self.params.read(pad_at(v.pad, pad::MODE)) >= 0.5 {
                        v.fade = v.fade.min(stop);
                    }
                }
            }
            Kind::AllNotesOff | Kind::Control { number: 120, .. } => {
                let stop = fall(STOP_SECONDS, self.rate);
                for v in &mut self.voices {
                    v.fade = v.fade.min(stop);
                }
            }
            _ => {}
        }
    }

    /// Plays frames `[from, to)`, at most [`CHUNK`].
    fn chunk(
        &mut self,
        from: usize,
        to: usize,
        left: &mut [f32],
        right: &mut [f32],
        outs: &mut Outputs,
    ) {
        let n = to - from;
        let mut busy = [false; PADS];
        for (p, fx) in self.fx.iter().enumerate() {
            busy[p] = !fx.is_empty();
        }
        for v in &self.voices {
            busy[v.pad] = true;
        }
        for (p, b) in self.bufs.iter_mut().enumerate() {
            if busy[p] {
                b[0][..n].fill(0.0);
                b[1][..n].fill(0.0);
            }
        }
        for v in &mut self.voices {
            let [bl, br] = &mut self.bufs[v.pad];
            let sample = self.samples.get(v.pad * LAYERS + v.layer);
            for i in 0..n {
                let (a, b) = match &mut v.src {
                    Src::Sample(pl) => match sample.and_then(|s| s.as_deref()) {
                        Some(s) if !pl.done => pl.next(s),
                        _ => {
                            v.on = false;
                            break;
                        }
                    },
                    Src::Drum(h) => {
                        if h.done {
                            v.on = false;
                            break;
                        }
                        let x = h.tick();
                        (x, x)
                    }
                };
                bl[i] += a * v.gain.0 * v.level;
                br[i] += b * v.gain.1 * v.level;
                v.level *= v.fade;
                if v.level < 1e-4 {
                    v.on = false;
                    break;
                }
            }
        }
        self.voices.retain(|v| v.on);
        let pr = &self.params;
        let master = db(pr.read(at::LEVEL));
        for (p, &b) in busy.iter().enumerate() {
            if !b {
                continue;
            }
            let [bl, br] = &mut self.bufs[p];
            for fx in &mut self.fx[p] {
                fx.process(&mut bl[..n], &mut br[..n]);
            }
            let g = db(pr.read(pad_at(p, pad::LEVEL))) * master;
            let (pl, pp) = crate::dsp::pan(pr.read(pad_at(p, pad::PAN)));
            let (gl, gr) = (g * pl, g * pp);
            let out = pr.read(pad_at(p, pad::OUTPUT)) as usize;
            match outs.get(out).filter(|_| out > 0) {
                Some((l, r)) => add(&mut l[from..to], &mut r[from..to], bl, br, gl, gr),
                None => add(&mut left[from..to], &mut right[from..to], bl, br, gl, gr),
            }
            for (k, o) in [(pad::SEND_A, SEND_A), (pad::SEND_B, SEND_B)] {
                let s = db(pr.read(pad_at(p, k)));
                if s > 0.0
                    && let Some((l, r)) = outs.get(o)
                {
                    add(&mut l[from..to], &mut r[from..to], bl, br, gl * s, gr * s);
                }
            }
        }
    }
}

fn add(l: &mut [f32], r: &mut [f32], bl: &[f32], br: &[f32], gl: f32, gr: f32) {
    for i in 0..l.len() {
        l[i] += bl[i] * gl;
        r[i] += br[i] * gr;
    }
}

impl Instrument for DrumMachine {
    fn process(&mut self, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        self.process_outputs(events, left, right, &mut Outputs::none());
    }

    fn process_outputs(
        &mut self,
        events: &[Event],
        left: &mut [f32],
        right: &mut [f32],
        outputs: &mut Outputs,
    ) {
        let n = left.len().min(right.len());
        let mut at = 0;
        let mut next = 0;
        while at < n {
            while next < events.len() && (events[next].frame as usize) <= at {
                self.event(&events[next]);
                next += 1;
            }
            // Up to the next event, a chunk at most.
            let until = events
                .get(next)
                .map_or(n, |e| (e.frame as usize).min(n))
                .max(at + 1);
            let end = until.min(at + CHUNK).min(n);
            self.chunk(at, end, left, right, outputs);
            at = end;
        }
        for e in &events[next..] {
            self.event(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> DrumMachine {
        let p = Arc::new(Params::new(params()));
        default_kit(&p);
        DrumMachine::new(48_000, p, Vec::new(), Vec::new())
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn every_pad_of_the_kit_sounds_on_its_key() {
        for p in 0..PADS {
            let mut d = machine();
            let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
            d.process(
                &[Event::note_on(100, 9, FIRST_NOTE + p as u8, 110, 0)],
                &mut l,
                &mut r,
            );
            assert_eq!(peak(&l[..100]), 0.0, "pad {p} early");
            assert!(peak(&l) > 0.05, "pad {p} silent");
        }
        // Keys outside the pads play nothing.
        let mut d = machine();
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        d.process(&[Event::note_on(0, 0, 80, 110, 0)], &mut l, &mut r);
        assert_eq!(peak(&l), 0.0);
    }

    #[test]
    fn the_closed_hat_chokes_the_open_one() {
        let mut d = machine();
        let open = FIRST_NOTE + 10;
        let closed = FIRST_NOTE + 6;
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        d.process(&[Event::note_on(0, 0, open, 100, 0)], &mut l, &mut r);
        assert!(d.voices.iter().any(|v| v.pad == 10));
        d.process(&[Event::note_on(0, 0, closed, 100, 1)], &mut l, &mut r);
        assert!(d.voices.iter().all(|v| v.pad != 10), "open hat still rings");
    }

    #[test]
    fn velocity_picks_the_layers() {
        let mut d = machine();
        let p = &d.params;
        // Pad 0: the kick below 64, a cowbell above.
        p.set(layer_at(0, 0, layer::VEL_HI), 63.0);
        p.set(
            layer_at(0, 1, layer::SOURCE),
            2.0 + Drum::Cowbell as usize as f64,
        );
        p.set(layer_at(0, 1, layer::VEL_LO), 64.0);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        d.process(&[Event::note_on(0, 0, FIRST_NOTE, 40, 0)], &mut l, &mut r);
        let layers: Vec<usize> = d.voices.iter().map(|v| v.layer).collect();
        assert_eq!(layers, [0]);
        d.process(&[Event::note_on(0, 0, FIRST_NOTE, 100, 1)], &mut l, &mut r);
        let layers: Vec<usize> = d.voices.iter().map(|v| v.layer).collect();
        assert_eq!(layers, [0, 1]);
    }

    #[test]
    fn a_pad_goes_to_its_own_output_and_its_sends() {
        let mut d = machine();
        d.params.set(pad_at(0, pad::OUTPUT), 3.0);
        d.params.set(pad_at(0, pad::SEND_A), 0.0);
        let mut bufs = vec![[vec![0.0; 4800], vec![0.0; 4800]]; SEND_A];
        let mut routed = vec![false; SEND_A];
        routed[2] = true;
        routed[SEND_A - 1] = true;
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        {
            let mut outs = Outputs::new(&mut bufs, &routed, 4800);
            d.process_outputs(
                &[Event::note_on(0, 0, FIRST_NOTE, 120, 0)],
                &mut l,
                &mut r,
                &mut outs,
            );
        }
        assert_eq!(peak(&l), 0.0, "the kick left the track");
        assert!(peak(&bufs[2][0]) > 0.05);
        assert_eq!(
            bufs[2][0],
            bufs[SEND_A - 1][0],
            "the send at 0 dB is the same"
        );
        // Unrouted, the output falls back to the track.
        let mut d = machine();
        d.params.set(pad_at(0, pad::OUTPUT), 3.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        d.process(&[Event::note_on(0, 0, FIRST_NOTE, 120, 0)], &mut l, &mut r);
        assert!(peak(&l) > 0.05);
    }

    #[test]
    fn stop_with_the_note_cuts_a_sample_short() {
        let p = Arc::new(Params::new(params()));
        p.set(layer_at(0, 0, layer::SOURCE), SAMPLE);
        p.set(pad_at(0, pad::MODE), 1.0);
        let s = Arc::new(Sample::mono(vec![0.5; 48_000], 48_000));
        let mut d = DrumMachine::new(48_000, p, vec![Some(s)], Vec::new());
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        d.process(
            &[
                Event::note_on(0, 0, FIRST_NOTE, 127, 0),
                Event::note_off(1000, 0, FIRST_NOTE, 0),
            ],
            &mut l,
            &mut r,
        );
        assert!(l[900] > 0.3);
        assert!(l[4000].abs() < 1e-3);
    }
}
