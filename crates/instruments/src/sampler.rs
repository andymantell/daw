// SPDX-License-Identifier: GPL-3.0-or-later
//! A multi-zone sampler (roadmap 3.4): sounds spread over the keyboard
//! and over how hard a key is played, each zone with its own root note,
//! key and velocity range, tuning, level, pan and loop. Every zone a
//! note falls in plays, so zones can be layered. A level envelope and a
//! filter shape every voice.

use std::sync::Arc;

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Adsr, Env, Mode, SVF_ZERO, Svf, db};
use crate::params::{Param, Params, Unit};
use crate::sample::{Player, Sample};

const FILTERS: &[&str] = &["Low-pass", "Band-pass", "High-pass"];

/// The settings for the whole instrument, by index.
pub mod at {
    pub const ATTACK: usize = 0;
    pub const DECAY: usize = 1;
    pub const SUSTAIN: usize = 2;
    pub const RELEASE: usize = 3;
    pub const FILTER: usize = 4;
    pub const CUTOFF: usize = 5;
    pub const RESONANCE: usize = 6;
    pub const VELOCITY: usize = 7;
    pub const BEND: usize = 8;
    pub const LEVEL: usize = 9;
    /// Where the zones' settings start; [`ZONE`] each.
    pub const ZONES: usize = 10;
}

/// A zone's settings, from its first.
pub mod zone {
    pub const ROOT: usize = 0;
    pub const LO: usize = 1;
    pub const HI: usize = 2;
    pub const VEL_LO: usize = 3;
    pub const VEL_HI: usize = 4;
    pub const TUNE: usize = 5;
    pub const LEVEL: usize = 6;
    pub const PAN: usize = 7;
    pub const LOOP: usize = 8;
    pub const LOOP_START: usize = 9;
    pub const LOOP_END: usize = 10;
}

/// Settings per zone.
pub const ZONE: usize = 11;

/// The settings for `zones` zones, the whole instrument's first.
pub fn params(zones: usize) -> Vec<Param> {
    use Unit::*;
    let mut v = vec![
        Param::new("attack", "Attack", 0.001, 4.0, 0.002, Seconds).log(),
        Param::new("decay", "Decay", 0.01, 20.0, 2.0, Seconds).log(),
        Param::new("sustain", "Sustain", 0.0, 1.0, 1.0, Amount),
        Param::new("release", "Release", 0.01, 8.0, 0.3, Seconds).log(),
        Param::new("filter", "Filter", 0.0, 2.0, 0.0, Choice).choices(FILTERS),
        Param::new("cutoff", "Cutoff", 30.0, 20_000.0, 20_000.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.0, Amount),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.8, Amount),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("level", "Level", -36.0, 12.0, 0.0, Db),
    ];
    for z in 0..zones {
        let k = |s: &str| format!("zone{z}.{s}");
        v.extend([
            Param::new(k("root"), "Root note", 0.0, 127.0, 60.0, Note),
            Param::new(k("lo"), "Lowest key", 0.0, 127.0, 0.0, Note),
            Param::new(k("hi"), "Highest key", 0.0, 127.0, 127.0, Note),
            Param::new(k("vel_lo"), "Softest", 1.0, 127.0, 1.0, Count),
            Param::new(k("vel_hi"), "Hardest", 1.0, 127.0, 127.0, Count),
            Param::new(k("tune"), "Tune", -100.0, 100.0, 0.0, Cents),
            Param::new(k("level"), "Level", -36.0, 12.0, 0.0, Db),
            Param::new(k("pan"), "Pan", -1.0, 1.0, 0.0, Pan),
            Param::new(k("loop"), "Loop", 0.0, 1.0, 0.0, Switch),
            Param::new(k("loop_start"), "Loop start", 0.0, 1.0, 0.0, Amount),
            Param::new(k("loop_end"), "Loop end", 0.0, 1.0, 1.0, Amount),
        ]);
    }
    v
}

/// A note name in a file name ("Piano C#4.wav", "bass_f2"), as a note
/// number, if there is one; 60 is C4.
pub fn note_in_name(name: &str) -> Option<u8> {
    let b: Vec<char> = name.chars().collect();
    let mut found = None;
    for i in 0..b.len() {
        let letter = b[i].to_ascii_uppercase();
        let base = match letter {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => continue,
        };
        // A note stands apart from the word before it.
        if i > 0 && b[i - 1].is_ascii_alphabetic() {
            continue;
        }
        let mut j = i + 1;
        let mut n: i32 = base;
        match b.get(j) {
            Some('#') | Some('s') => {
                n += 1;
                j += 1;
            }
            Some('b')
                if b.get(j + 1)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '-') =>
            {
                n -= 1;
                j += 1;
            }
            _ => {}
        }
        let neg = b.get(j) == Some(&'-');
        if neg {
            j += 1;
        }
        let Some(d) = b.get(j).and_then(|c| c.to_digit(10)) else {
            continue;
        };
        if b.get(j + 1).is_some_and(|c| c.is_ascii_digit()) {
            continue;
        }
        let octave = if neg { -(d as i32) } else { d as i32 };
        let note = n + (octave + 1) * 12;
        if (0..=127).contains(&note) {
            found = Some(note as u8);
        }
    }
    found
}

/// Spreads zones with these root notes over the keyboard: each from
/// halfway to the one below to halfway to the one above, the lowest
/// down to the bottom key and the highest to the top. Returns
/// (lowest, highest) for each, in the order given.
pub fn spread(roots: &[u8]) -> Vec<(u8, u8)> {
    let mut order: Vec<usize> = (0..roots.len()).collect();
    order.sort_by_key(|&i| roots[i]);
    let mut out = vec![(0u8, 127u8); roots.len()];
    for (k, &i) in order.iter().enumerate() {
        let lo = if k == 0 {
            0
        } else {
            let below = roots[order[k - 1]];
            (below as u16 + roots[i] as u16).div_ceil(2) as u8
        };
        let hi = if k + 1 == order.len() {
            127
        } else {
            let above = roots[order[k + 1]];
            ((roots[i] as u16 + above as u16).div_ceil(2) - 1) as u8
        };
        out[i] = (lo, hi.max(lo));
    }
    out
}

const VOICES: usize = 32;
const CHUNK: usize = 32;

#[derive(Clone, Copy)]
struct Voice {
    on: bool,
    zone: usize,
    channel: u8,
    note: u8,
    held: bool,
    player: Player,
    /// Semitones from the root, without the bend.
    shift: f32,
    gain: (f32, f32),
    env: Env,
    filter: [Svf; 2],
    age: u64,
}

impl Voice {
    const OFF: Voice = Voice {
        on: false,
        zone: 0,
        channel: 0,
        note: 0,
        held: false,
        player: Player {
            pos: 0.0,
            step: 1.0,
            start: 0.0,
            end: 0.0,
            looping: None,
            done: true,
        },
        shift: 0.0,
        gain: (0.0, 0.0),
        env: Env::IDLE,
        filter: [SVF_ZERO; 2],
        age: 0,
    };
}

/// The sampler on the audio thread.
pub struct Sampler {
    params: Arc<Params>,
    /// Each zone's sound at the output's rate, if it has one.
    zones: Vec<Option<Arc<Sample>>>,
    rate: f32,
    voices: [Voice; VOICES],
    bend: f32,
    pedal: bool,
    clock: u64,
}

impl Sampler {
    pub fn new(rate: u32, params: Arc<Params>, zones: Vec<Option<Arc<Sample>>>) -> Sampler {
        Sampler {
            params,
            zones,
            rate: rate.max(1) as f32,
            voices: [Voice::OFF; VOICES],
            bend: 0.0,
            pedal: false,
            clock: 0,
        }
    }

    fn z(&self, zone: usize, k: usize) -> f32 {
        self.params.read(at::ZONES + zone * ZONE + k)
    }

    fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        let sens = self.params.read(at::VELOCITY);
        let vel = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
        for zi in 0..self.zones.len() {
            let Some(s) = self.zones[zi].clone() else {
                continue;
            };
            let (lo, hi) = (self.z(zi, zone::LO), self.z(zi, zone::HI));
            let (vlo, vhi) = (self.z(zi, zone::VEL_LO), self.z(zi, zone::VEL_HI));
            let (n, v) = (note as f32, velocity as f32);
            if n < lo || n > hi || v < vlo || v > vhi || s.is_empty() {
                continue;
            }
            self.clock += 1;
            let len = s.len() as f64;
            let mut player = Player::new(0.0, len, 1.0);
            if self.z(zi, zone::LOOP) >= 0.5 {
                let a = self.z(zi, zone::LOOP_START) as f64 * len;
                let b = self.z(zi, zone::LOOP_END) as f64 * len;
                if b - a >= 16.0 {
                    player.looping = Some((a, b));
                    player.end = f64::INFINITY;
                }
            }
            let shift = n - self.z(zi, zone::ROOT) + self.z(zi, zone::TUNE) / 100.0;
            let g = db(self.z(zi, zone::LEVEL)) * vel;
            let (pl, pr) = crate::dsp::pan(self.z(zi, zone::PAN));
            let slot = (self.voices.iter().position(|v| !v.on))
                .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap_or(0));
            let mut env = Env::IDLE;
            env.start();
            self.voices[slot] = Voice {
                on: true,
                zone: zi,
                channel,
                note,
                held: true,
                player,
                shift,
                gain: (g * pl, g * pr),
                env,
                filter: [SVF_ZERO; 2],
                age: self.clock,
            };
        }
    }

    fn release(&mut self, channel: u8, note: u8) {
        for v in &mut self.voices {
            if v.on && v.held && v.channel == channel && v.note == note {
                v.held = false;
                if !self.pedal {
                    v.env.release();
                }
            }
        }
    }

    fn event(&mut self, e: &Event) {
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.note_on(e.channel, pitch, velocity),
            Kind::NoteOff { pitch, .. } => self.release(e.channel, pitch),
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number: 64, value } => {
                self.pedal = value >= 64;
                if !self.pedal {
                    for v in self.voices.iter_mut().filter(|v| v.on && !v.held) {
                        v.env.release();
                    }
                }
            }
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => {
                self.pedal = false;
                for v in &mut self.voices {
                    v.held = false;
                    v.env.release();
                }
            }
            _ => {}
        }
    }

    fn run(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let mut at = from;
        while at < to {
            let end = (at + CHUNK).min(to);
            self.chunk(at, end, left, right);
            at = end;
        }
    }

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let p = &self.params;
        let r = self.rate;
        let adsr = Adsr::new(
            p.read(at::ATTACK),
            p.read(at::DECAY),
            p.read(at::SUSTAIN),
            p.read(at::RELEASE),
            r,
        );
        let mode = Mode::from_index(p.read(at::FILTER));
        let cutoff = p.read(at::CUTOFF);
        let res = p.read(at::RESONANCE);
        let filtering = !(mode == Mode::Low && cutoff >= 19_999.0);
        let bend = self.bend * p.read(at::BEND);
        let gain = db(p.read(at::LEVEL));
        for v in &mut self.voices {
            if !v.on {
                continue;
            }
            let Some(s) = self.zones.get(v.zone).and_then(|z| z.as_deref()) else {
                v.on = false;
                continue;
            };
            v.player.step = ((v.shift + bend) as f64 / 12.0).exp2();
            if filtering {
                for f in &mut v.filter {
                    f.set(cutoff, res, r);
                }
            }
            for i in from..to {
                let e = v.env.next(&adsr);
                if !v.env.active() || v.player.done {
                    v.on = false;
                    break;
                }
                let (mut a, mut b) = v.player.next(s);
                if filtering {
                    a = v.filter[0].run(a, mode);
                    b = v.filter[1].run(b, mode);
                }
                left[i] += a * v.gain.0 * e * gain;
                right[i] += b * v.gain.1 * e * gain;
            }
        }
    }
}

impl Instrument for Sampler {
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

    fn tone(hz: f32, rate: u32, secs: f32) -> Arc<Sample> {
        let n = (rate as f32 * secs) as usize;
        Arc::new(Sample::mono(
            (0..n)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() * 0.5)
                .collect(),
            rate,
        ))
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn names_give_notes_and_roots_spread_over_the_keys() {
        assert_eq!(note_in_name("Piano C4"), Some(60));
        assert_eq!(note_in_name("bass_F#2.wav"), Some(42));
        assert_eq!(note_in_name("pad-Bb3"), Some(58));
        assert_eq!(note_in_name("Strings A-1"), Some(9));
        assert_eq!(note_in_name("Cello"), None);
        assert_eq!(note_in_name("Take 12"), None);
        assert_eq!(spread(&[60, 48, 72]), [(54, 65), (0, 53), (66, 127)]);
        assert_eq!(spread(&[60]), [(0, 127)]);
    }

    #[test]
    fn a_note_plays_its_zone_at_its_pitch() {
        let rate = 48_000;
        let p = Arc::new(Params::new(params(2)));
        // Zone 0: 440 Hz at A4 for the keys up to B4; zone 1 silent above.
        p.set(at::ZONES + zone::ROOT, 69.0);
        p.set(at::ZONES + zone::HI, 71.0);
        p.set(at::ZONES + ZONE + zone::LO, 72.0);
        let mut s = Sampler::new(rate, p, vec![Some(tone(440.0, rate, 2.0)), None]);
        let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
        // An octave below the root plays at half speed: 220 Hz.
        s.process(&[Event::note_on(0, 0, 57, 127, 0)], &mut l, &mut r);
        let c = crossings(&l[4800..]) as f32 / 2.0 / 0.4;
        assert!((215.0..225.0).contains(&c), "{c}");
        // A key in the empty zone plays nothing.
        let mut s2 = Sampler::new(rate, s.params.clone(), s.zones.clone());
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        s2.process(&[Event::note_on(0, 0, 80, 127, 0)], &mut l, &mut r);
        assert!(l.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn velocity_ranges_pick_the_layer_and_loops_hold_the_note() {
        let rate = 48_000;
        let p = Arc::new(Params::new(params(2)));
        p.set(at::ZONES + zone::VEL_HI, 64.0);
        p.set(at::ZONES + ZONE + zone::VEL_LO, 65.0);
        p.set(at::ZONES + ZONE + zone::LOOP, 1.0);
        p.set(at::ZONES + ZONE + zone::LOOP_START, 0.25);
        p.set(at::ZONES + ZONE + zone::LOOP_END, 0.75);
        let short = tone(100.0, rate, 0.1);
        let mut s = Sampler::new(rate, p, vec![Some(short.clone()), Some(short)]);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        s.process(&[Event::note_on(0, 0, 60, 30, 0)], &mut l, &mut r);
        assert_eq!(
            s.voices
                .iter()
                .filter(|v| v.on)
                .map(|v| v.zone)
                .collect::<Vec<_>>(),
            [0]
        );
        s.process(&[Event::note_on(0, 0, 62, 100, 1)], &mut l, &mut r);
        // A second later the unlooped one has ended and the looped one plays on.
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        s.process(&[], &mut l, &mut r);
        let on: Vec<usize> = s.voices.iter().filter(|v| v.on).map(|v| v.zone).collect();
        assert_eq!(on, [1]);
        assert!(l[47_000..].iter().any(|v| v.abs() > 0.1));
    }
}
