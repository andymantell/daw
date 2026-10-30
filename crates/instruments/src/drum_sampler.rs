// SPDX-License-Identifier: GPL-3.0-or-later
//! A drum sampler (roadmap 3.4): one sound played as a hit, from any
//! key. It can be tuned, started late, cut short with a fade, played
//! backwards, filtered, and made to follow the key's pitch or not. A
//! hit plays to its end unless it is set to stop with the note, and a
//! new hit can cut the last one off, as a real drum's does.

use std::sync::Arc;

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Mode, SVF_ZERO, Svf, db, fall};
use crate::params::{Param, Params, Unit};
use crate::sample::{Player, Sample};

pub const MODES: &[&str] = &["Play to the end", "Stop with the note"];
const FILTERS: &[&str] = &["Low-pass", "Band-pass", "High-pass"];

pub mod at {
    pub const TUNE: usize = 0;
    pub const FINE: usize = 1;
    pub const START: usize = 2;
    pub const DECAY: usize = 3;
    pub const MODE: usize = 4;
    pub const KEYS: usize = 5;
    pub const REVERSE: usize = 6;
    pub const CUT_SELF: usize = 7;
    pub const FILTER: usize = 8;
    pub const CUTOFF: usize = 9;
    pub const RESONANCE: usize = 10;
    pub const VELOCITY: usize = 11;
    pub const PAN: usize = 12;
    pub const LEVEL: usize = 13;
}

/// The longest decay; at it, the sound plays without fading.
pub const NO_FADE: f64 = 10.0;

pub fn params() -> Vec<Param> {
    use Unit::*;
    vec![
        Param::new("tune", "Tune", -24.0, 24.0, 0.0, Semitones),
        Param::new("fine", "Fine", -100.0, 100.0, 0.0, Cents),
        Param::new("start", "Start", 0.0, 0.95, 0.0, Amount),
        Param::new("decay", "Decay", 0.02, NO_FADE, NO_FADE, Seconds).log(),
        Param::new("mode", "Notes", 0.0, 1.0, 0.0, Choice).choices(MODES),
        Param::new("keys", "Pitch follows the key", 0.0, 1.0, 0.0, Switch),
        Param::new("reverse", "Backwards", 0.0, 1.0, 0.0, Switch),
        Param::new("cut_self", "A hit cuts the last", 0.0, 1.0, 0.0, Switch),
        Param::new("filter", "Filter", 0.0, 2.0, 0.0, Choice).choices(FILTERS),
        Param::new("cutoff", "Cutoff", 30.0, 20_000.0, 20_000.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.0, Amount),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.8, Amount),
        Param::new("pan", "Pan", -1.0, 1.0, 0.0, Pan),
        Param::new("level", "Level", -36.0, 12.0, 0.0, Db),
    ]
}

const VOICES: usize = 16;
const CHUNK: usize = 32;
/// How long a cut-off hit takes to fade, in seconds.
const CUT: f32 = 0.004;

#[derive(Clone, Copy)]
struct Voice {
    on: bool,
    note: u8,
    channel: u8,
    held: bool,
    player: Player,
    level: f32,
    /// Fading out fast: cut off by a new hit or stopped with its note.
    cut: bool,
    gain: f32,
    filter: [Svf; 2],
    age: u64,
}

const OFF: Voice = Voice {
    on: false,
    note: 0,
    channel: 0,
    held: false,
    player: Player {
        pos: 0.0,
        step: 1.0,
        start: 0.0,
        end: 0.0,
        looping: None,
        done: true,
    },
    level: 0.0,
    cut: false,
    gain: 0.0,
    filter: [SVF_ZERO; 2],
    age: 0,
};

pub struct DrumSampler {
    params: Arc<Params>,
    sample: Option<Arc<Sample>>,
    rate: f32,
    voices: [Voice; VOICES],
    clock: u64,
}

impl DrumSampler {
    pub fn new(rate: u32, params: Arc<Params>, sample: Option<Arc<Sample>>) -> DrumSampler {
        DrumSampler {
            params,
            sample,
            rate: rate.max(1) as f32,
            voices: [OFF; VOICES],
            clock: 0,
        }
    }

    fn hit(&mut self, channel: u8, note: u8, velocity: u8) {
        let Some(s) = &self.sample else { return };
        let p = &self.params;
        if p.read(at::CUT_SELF) >= 0.5 {
            for v in self.voices.iter_mut().filter(|v| v.on) {
                v.cut = true;
            }
        }
        let len = s.len() as f64;
        let start = p.read(at::START) as f64 * len;
        let (a, b) = if p.read(at::REVERSE) >= 0.5 {
            (len - 1.0 - start, 0.0)
        } else {
            (start, len)
        };
        let keys = if p.read(at::KEYS) >= 0.5 {
            note as f32 - 60.0
        } else {
            0.0
        };
        let semis = p.read(at::TUNE) + p.read(at::FINE) / 100.0 + keys;
        let player = Player::new(a, b, (semis as f64 / 12.0).exp2());
        let sens = p.read(at::VELOCITY);
        let gain = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
        self.clock += 1;
        let slot = (self.voices.iter().position(|v| !v.on))
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap_or(0));
        self.voices[slot] = Voice {
            on: true,
            note,
            channel,
            held: true,
            player,
            level: 1.0,
            cut: false,
            gain,
            filter: [SVF_ZERO; 2],
            age: self.clock,
        };
    }

    fn event(&mut self, e: &Event) {
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.hit(e.channel, pitch, velocity),
            Kind::NoteOff { pitch, .. } => {
                let gate = self.params.read(at::MODE) >= 0.5;
                for v in &mut self.voices {
                    if v.on && v.held && v.note == pitch && v.channel == e.channel {
                        v.held = false;
                        v.cut |= gate;
                    }
                }
            }
            Kind::AllNotesOff | Kind::Control { number: 120, .. } => {
                for v in self.voices.iter_mut().filter(|v| v.on) {
                    v.cut = true;
                }
            }
            _ => {}
        }
    }

    fn run(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let Some(s) = self.sample.clone() else { return };
        let p = &self.params;
        let r = self.rate;
        let decay = p.read(at::DECAY);
        let fade = if decay as f64 >= NO_FADE {
            1.0
        } else {
            fall(decay, r)
        };
        let cut = fall(CUT, r);
        let mode = Mode::from_index(p.read(at::FILTER));
        let cutoff = p.read(at::CUTOFF);
        let filtering = !(mode == Mode::Low && cutoff >= 19_999.0);
        let res = p.read(at::RESONANCE);
        let (pl, pr) = crate::dsp::pan(p.read(at::PAN));
        let gain = db(p.read(at::LEVEL));
        let mut at = from;
        while at < to {
            let end = (at + CHUNK).min(to);
            for v in self.voices.iter_mut().filter(|v| v.on) {
                if filtering {
                    for f in &mut v.filter {
                        f.set(cutoff, res, r);
                    }
                }
                for i in at..end {
                    if v.player.done || v.level < 1e-4 {
                        v.on = false;
                        break;
                    }
                    let (mut a, mut b) = v.player.next(&s);
                    if filtering {
                        a = v.filter[0].run(a, mode);
                        b = v.filter[1].run(b, mode);
                    }
                    let g = v.level * v.gain * gain;
                    left[i] += a * g * pl;
                    right[i] += b * g * pr;
                    v.level *= if v.cut { cut } else { fade };
                }
            }
            at = end;
        }
    }
}

impl Instrument for DrumSampler {
    fn process(&mut self, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut at = 0;
        for e in events {
            let f = (e.frame as usize).min(n);
            self.run(at, f, left, right);
            at = f;
            self.event(e);
        }
        self.run(at, n, left, right);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> Arc<Sample> {
        Arc::new(Sample::mono(
            (0..4800).map(|i| i as f32 / 4800.0).collect(),
            48_000,
        ))
    }

    #[test]
    fn a_hit_plays_to_its_end_whatever_the_note_does() {
        let mut d = DrumSampler::new(48_000, Arc::new(Params::new(params())), Some(ramp()));
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        d.process(
            &[
                Event::note_on(0, 0, 36, 127, 0),
                Event::note_off(10, 0, 36, 0),
            ],
            &mut l,
            &mut r,
        );
        assert!(l[4000] > 0.8, "{}", l[4000]);
        assert_eq!(l[5000], 0.0);
    }

    #[test]
    fn stopping_with_the_note_backwards_and_cutting_the_last_hit() {
        let p = Arc::new(Params::new(params()));
        p.set(at::MODE, 1.0);
        let mut d = DrumSampler::new(48_000, p.clone(), Some(ramp()));
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        d.process(
            &[
                Event::note_on(0, 0, 36, 127, 0),
                Event::note_off(1000, 0, 36, 0),
            ],
            &mut l,
            &mut r,
        );
        assert!(l[999] > 0.1);
        assert!(l[2000].abs() < 1e-3);

        p.set(at::MODE, 0.0);
        p.set(at::REVERSE, 1.0);
        let mut d = DrumSampler::new(48_000, p.clone(), Some(ramp()));
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        d.process(&[Event::note_on(0, 0, 36, 127, 0)], &mut l, &mut r);
        assert!(l[100] > l[3000], "backwards it starts loud");

        p.set(at::REVERSE, 0.0);
        p.set(at::CUT_SELF, 1.0);
        let mut d = DrumSampler::new(48_000, p, Some(ramp()));
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        d.process(
            &[
                Event::note_on(0, 0, 36, 127, 0),
                Event::note_on(2000, 0, 36, 127, 1),
            ],
            &mut l,
            &mut r,
        );
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        d.process(&[], &mut l, &mut r);
        assert_eq!(d.voices.iter().filter(|v| v.on).count(), 1);
    }
}
