// SPDX-License-Identifier: GPL-3.0-or-later
//! A synth of our own design (roadmap 3.4): two oscillators and a sub
//! oscillator, noise, up to four detuned copies of each voice spread
//! across the stereo field, a resonant filter with its own envelope and
//! key tracking, and an LFO on pitch, filter and level. Sixteen voices,
//! or one that glides from note to note.

use std::sync::Arc;

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Adsr, Env, Mode, Noise, Svf, Wave, db, hz, osc};
use crate::params::{Param, Params, Unit};

const LFO_WAVES: &[&str] = &["Sine", "Triangle", "Square", "Random"];
const FILTERS: &[&str] = &["Low-pass", "Band-pass", "High-pass"];
const PLAY: &[&str] = &["Chords", "One note, gliding"];

/// Settings by index; the order of [`params`].
mod at {
    pub const OSC1: usize = 0;
    pub const OSC2: usize = 1;
    pub const OSC2_SEMIS: usize = 2;
    pub const OSC2_CENTS: usize = 3;
    pub const OSC_MIX: usize = 4;
    pub const WIDTH: usize = 5;
    pub const SUB: usize = 6;
    pub const NOISE: usize = 7;
    pub const UNISON: usize = 8;
    pub const DETUNE: usize = 9;
    pub const SPREAD: usize = 10;
    pub const FILTER: usize = 11;
    pub const CUTOFF: usize = 12;
    pub const RESONANCE: usize = 13;
    pub const KEY_TRACK: usize = 14;
    pub const FILTER_ENV: usize = 15;
    pub const F_ATTACK: usize = 16;
    pub const F_DECAY: usize = 17;
    pub const F_SUSTAIN: usize = 18;
    pub const F_RELEASE: usize = 19;
    pub const ATTACK: usize = 20;
    pub const DECAY: usize = 21;
    pub const SUSTAIN: usize = 22;
    pub const RELEASE: usize = 23;
    pub const LFO_WAVE: usize = 24;
    pub const LFO_RATE: usize = 25;
    pub const LFO_PITCH: usize = 26;
    pub const LFO_CUTOFF: usize = 27;
    pub const LFO_LEVEL: usize = 28;
    pub const PLAY: usize = 29;
    pub const GLIDE: usize = 30;
    pub const VELOCITY: usize = 31;
    pub const BEND: usize = 32;
    pub const LEVEL: usize = 33;
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    vec![
        Param::new("osc1", "Oscillator 1", 0.0, 4.0, 2.0, Choice).choices(Wave::NAMES),
        Param::new("osc2", "Oscillator 2", 0.0, 4.0, 2.0, Choice).choices(Wave::NAMES),
        Param::new("osc2_semis", "Osc 2 pitch", -24.0, 24.0, 0.0, Semitones),
        Param::new("osc2_cents", "Osc 2 fine", -50.0, 50.0, 7.0, Cents),
        Param::new("osc_mix", "Osc 1 / 2 mix", 0.0, 1.0, 0.5, Amount),
        Param::new("width", "Pulse width", 0.05, 0.95, 0.5, Amount),
        Param::new("sub", "Sub oscillator", 0.0, 1.0, 0.0, Amount),
        Param::new("noise", "Noise", 0.0, 1.0, 0.0, Amount),
        Param::new("unison", "Unison voices", 1.0, 4.0, 1.0, Count),
        Param::new("detune", "Unison detune", 0.0, 50.0, 12.0, Cents),
        Param::new("spread", "Stereo spread", 0.0, 1.0, 0.6, Amount),
        Param::new("filter", "Filter", 0.0, 2.0, 0.0, Choice).choices(FILTERS),
        Param::new("cutoff", "Cutoff", 30.0, 18_000.0, 2_500.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.2, Amount),
        Param::new("key_track", "Key tracking", 0.0, 1.0, 0.5, Amount),
        Param::new("filter_env", "Envelope amount", -4.0, 4.0, 1.5, Octaves),
        Param::new("f_attack", "Filter attack", 0.001, 4.0, 0.005, Seconds).log(),
        Param::new("f_decay", "Filter decay", 0.01, 8.0, 0.6, Seconds).log(),
        Param::new("f_sustain", "Filter sustain", 0.0, 1.0, 0.3, Amount),
        Param::new("f_release", "Filter release", 0.01, 8.0, 0.4, Seconds).log(),
        Param::new("attack", "Attack", 0.001, 4.0, 0.005, Seconds).log(),
        Param::new("decay", "Decay", 0.01, 8.0, 0.5, Seconds).log(),
        Param::new("sustain", "Sustain", 0.0, 1.0, 0.7, Amount),
        Param::new("release", "Release", 0.01, 8.0, 0.35, Seconds).log(),
        Param::new("lfo_wave", "LFO shape", 0.0, 3.0, 0.0, Choice).choices(LFO_WAVES),
        Param::new("lfo_rate", "LFO rate", 0.05, 20.0, 5.0, Hz).log(),
        Param::new("lfo_pitch", "LFO to pitch", 0.0, 2.0, 0.0, Semitones),
        Param::new("lfo_cutoff", "LFO to filter", 0.0, 4.0, 0.0, Octaves),
        Param::new("lfo_level", "LFO to level", 0.0, 1.0, 0.0, Amount),
        Param::new("play", "Play", 0.0, 1.0, 0.0, Choice).choices(PLAY),
        Param::new("glide", "Glide", 0.0, 2.0, 0.0, Seconds),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.7, Amount),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("level", "Level", -36.0, 6.0, -10.0, Db),
    ]
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Oscillators", at::OSC1, at::SPREAD),
    ("Filter", at::FILTER, at::F_RELEASE),
    ("Level", at::ATTACK, at::RELEASE),
    ("LFO", at::LFO_WAVE, at::LFO_LEVEL),
    ("Playing", at::PLAY, at::LEVEL),
];

const VOICES: usize = 16;
const UNISON: usize = 4;
/// Settings are read, and the filters set, this often.
const CHUNK: usize = 16;

#[derive(Clone, Copy)]
struct Voice {
    on: bool,
    channel: u8,
    note: u8,
    /// The pitch now, gliding towards `note`.
    pitch: f32,
    velocity: f32,
    phase: [[f32; 2]; UNISON],
    sub: f32,
    amp: Env,
    fenv: Env,
    filter: [Svf; 2],
    age: u64,
    held: bool,
}

impl Voice {
    const OFF: Voice = Voice {
        on: false,
        channel: 0,
        note: 0,
        pitch: 60.0,
        velocity: 0.0,
        phase: [[0.0; 2]; UNISON],
        sub: 0.0,
        amp: Env::IDLE,
        fenv: Env::IDLE,
        filter: [crate::dsp::SVF_ZERO; 2],
        age: 0,
        held: false,
    };
}

/// The synth on the audio thread.
pub struct Synth {
    params: Arc<Params>,
    rate: f32,
    voices: [Voice; VOICES],
    bend: f32,
    pedal: bool,
    clock: u64,
    lfo_phase: f32,
    lfo_held: f32,
    noise: Noise,
    /// Notes held down in mono mode, newest last, to go back to.
    stack: [u8; 16],
    stack_len: usize,
}

impl Synth {
    pub fn new(rate: u32, params: Arc<Params>) -> Synth {
        Synth {
            params,
            rate: rate.max(1) as f32,
            voices: [Voice::OFF; VOICES],
            bend: 0.0,
            pedal: false,
            clock: 0,
            lfo_phase: 0.0,
            lfo_held: 0.0,
            noise: Noise::new(0x1234_5678),
            stack: [0; 16],
            stack_len: 0,
        }
    }

    fn mono(&self) -> bool {
        self.params.read(at::PLAY) >= 0.5
    }

    fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        self.clock += 1;
        let sens = self.params.read(at::VELOCITY);
        let vel = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
        if self.mono() {
            if self.stack_len < self.stack.len() {
                self.stack[self.stack_len] = note;
                self.stack_len += 1;
            }
            let v = &mut self.voices[0];
            let legato = v.on && v.held;
            v.note = note;
            v.channel = channel;
            v.held = true;
            v.velocity = vel;
            if !v.on {
                v.pitch = note as f32;
            }
            if !legato || self.params.read(at::GLIDE) <= 0.0 {
                // Without gliding, a new note jumps and starts again.
                if !legato {
                    v.amp.start();
                    v.fenv.start();
                }
                if self.params.read(at::GLIDE) <= 0.0 {
                    v.pitch = note as f32;
                }
            }
            v.on = true;
            v.age = self.clock;
            return;
        }
        let slot = (self.voices.iter().position(|v| !v.on))
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap_or(0));
        let v = &mut self.voices[slot];
        let fresh = !v.on;
        v.on = true;
        v.channel = channel;
        v.note = note;
        if fresh || self.params.read(at::GLIDE) <= 0.0 {
            v.pitch = note as f32;
        }
        v.velocity = vel;
        if fresh {
            // Phases spread so unison copies don't start in step.
            for (k, p) in v.phase.iter_mut().enumerate() {
                *p = [k as f32 * 0.37 % 1.0, k as f32 * 0.61 % 1.0];
            }
            v.filter = [crate::dsp::SVF_ZERO; 2];
        }
        v.amp.start();
        v.fenv.start();
        v.age = self.clock;
        v.held = true;
    }

    fn note_off(&mut self, channel: u8, note: u8) {
        if self.mono() {
            // Back to the last note still held, if there is one.
            if let Some(i) = self.stack[..self.stack_len]
                .iter()
                .rposition(|&n| n == note)
            {
                self.stack.copy_within(i + 1..self.stack_len, i);
                self.stack_len -= 1;
            }
            let v = &mut self.voices[0];
            if v.note != note {
                return;
            }
            if self.stack_len > 0 {
                v.note = self.stack[self.stack_len - 1];
                if self.params.read(at::GLIDE) <= 0.0 {
                    v.pitch = v.note as f32;
                }
                return;
            }
            v.held = false;
            if !self.pedal {
                v.amp.release();
                v.fenv.release();
            }
            return;
        }
        for v in &mut self.voices {
            if v.on && v.held && v.channel == channel && v.note == note {
                v.held = false;
                if !self.pedal {
                    v.amp.release();
                    v.fenv.release();
                }
            }
        }
    }

    fn event(&mut self, e: &Event) {
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => self.note_on(e.channel, pitch, velocity),
            Kind::NoteOff { pitch, .. } => self.note_off(e.channel, pitch),
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number: 64, value } => {
                self.pedal = value >= 64;
                if !self.pedal {
                    for v in &mut self.voices {
                        if v.on && !v.held {
                            v.amp.release();
                            v.fenv.release();
                        }
                    }
                }
            }
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => {
                for v in &mut self.voices {
                    v.held = false;
                    v.amp.release();
                    v.fenv.release();
                }
                self.pedal = false;
                self.stack_len = 0;
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
        let n = (to - from) as f32;
        // The LFO, once a chunk.
        let lfo_rate = p.read(at::LFO_RATE);
        let lfo = match p.read(at::LFO_WAVE) as u32 {
            1 => 1.0 - 4.0 * (self.lfo_phase - 0.5).abs(),
            2 => {
                if self.lfo_phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => self.lfo_held,
            _ => (self.lfo_phase * std::f32::consts::TAU).sin(),
        };
        self.lfo_phase += lfo_rate * n / r;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
            self.lfo_held = self.noise.tick();
        }
        let w1 = Wave::from_index(p.read(at::OSC1));
        let w2 = Wave::from_index(p.read(at::OSC2));
        let semis2 = p.read(at::OSC2_SEMIS) + p.read(at::OSC2_CENTS) / 100.0;
        let mix = p.read(at::OSC_MIX);
        let width = p.read(at::WIDTH);
        let sub = p.read(at::SUB);
        let noise = p.read(at::NOISE);
        let unison = (p.read(at::UNISON) as usize).clamp(1, UNISON);
        let detune = p.read(at::DETUNE) / 100.0;
        let spread = p.read(at::SPREAD);
        let mode = Mode::from_index(p.read(at::FILTER));
        let cutoff = p.read(at::CUTOFF);
        let res = p.read(at::RESONANCE);
        let track = p.read(at::KEY_TRACK);
        let fenv_amt = p.read(at::FILTER_ENV);
        let fadsr = Adsr::new(
            p.read(at::F_ATTACK),
            p.read(at::F_DECAY),
            p.read(at::F_SUSTAIN),
            p.read(at::F_RELEASE),
            r,
        );
        let adsr = Adsr::new(
            p.read(at::ATTACK),
            p.read(at::DECAY),
            p.read(at::SUSTAIN),
            p.read(at::RELEASE),
            r,
        );
        let lfo_pitch = p.read(at::LFO_PITCH) * lfo;
        let lfo_cut = p.read(at::LFO_CUTOFF) * lfo;
        let lfo_level = 1.0 - p.read(at::LFO_LEVEL) * (0.5 - 0.5 * lfo);
        let glide = p.read(at::GLIDE);
        let bend = self.bend * p.read(at::BEND);
        let gain = db(p.read(at::LEVEL)) * lfo_level;
        // Unison copies: detune and pan for each.
        let mut offs = [0.0f32; UNISON];
        let mut pans = [(1.0f32, 1.0f32); UNISON];
        for k in 0..unison {
            let x = if unison == 1 {
                0.0
            } else {
                k as f32 / (unison - 1) as f32 * 2.0 - 1.0
            };
            offs[k] = x * detune;
            pans[k] = crate::dsp::pan(x * spread);
        }
        let norm = 1.0 / (unison as f32).sqrt();
        let glide_k = if glide > 0.0 {
            1.0 - (-n / (glide * r * 0.25)).exp()
        } else {
            1.0
        };
        for v in &mut self.voices {
            if !v.on {
                continue;
            }
            v.pitch += (v.note as f32 - v.pitch) * glide_k;
            let pitch = v.pitch + bend + lfo_pitch;
            // The filter's cutoff, from the knob, the key, the envelope
            // (at the chunk's start) and the LFO, in octaves.
            let octaves = track * (pitch - 60.0) / 12.0 + fenv_amt * v.fenv.level + lfo_cut;
            let fc = cutoff * octaves.exp2();
            for f in &mut v.filter {
                f.set(fc, res, r);
            }
            let mut dts = [[0.0f32; 2]; UNISON];
            for (k, dt) in dts.iter_mut().enumerate().take(unison) {
                *dt = [hz(pitch + offs[k]) / r, hz(pitch + semis2 + offs[k]) / r];
            }
            let sub_dt = hz(pitch - 12.0) / r;
            for i in from..to {
                let a = v.amp.next(&adsr);
                v.fenv.next(&fadsr);
                if !v.amp.active() {
                    v.on = false;
                    break;
                }
                let (mut l, mut rr) = (0.0, 0.0);
                for k in 0..unison {
                    let [p1, p2] = &mut v.phase[k];
                    let [d1, d2] = dts[k];
                    let s = (1.0 - mix) * osc(w1, *p1, d1, width) + mix * osc(w2, *p2, d2, width);
                    *p1 = (*p1 + d1).fract();
                    *p2 = (*p2 + d2).fract();
                    l += s * pans[k].0;
                    rr += s * pans[k].1;
                }
                let mut common = 0.0;
                if sub > 0.0 {
                    common += sub * osc(Wave::Square, v.sub, sub_dt, 0.5);
                }
                v.sub = (v.sub + sub_dt).fract();
                if noise > 0.0 {
                    common += noise * self.noise.tick();
                }
                let l = v.filter[0].run(l * norm + common, mode);
                let rr = v.filter[1].run(rr * norm + common, mode);
                let g = a * v.velocity * gain;
                left[i] += l * g;
                right[i] += rr * g;
            }
        }
    }
}

impl Instrument for Synth {
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
    use daw_engine::midi::Event;

    fn synth() -> Synth {
        Synth::new(48_000, Arc::new(Params::new(params())))
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn a_note_sounds_then_dies_away() {
        let mut s = synth();
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        s.process(&[Event::note_on(100, 0, 60, 100, 0)], &mut l, &mut r);
        assert_eq!(peak(&l[..100]), 0.0);
        assert!(peak(&l[100..]) > 0.05);
        let (mut l, mut r) = (vec![0.0; 96_000], vec![0.0; 96_000]);
        s.process(&[Event::note_off(0, 0, 60, 0)], &mut l, &mut r);
        assert_eq!(peak(&l[90_000..]), 0.0);
        assert!(s.voices.iter().all(|v| !v.on));
    }

    #[test]
    fn its_pitch_follows_the_note() {
        let mut s = synth();
        let p = &s.params;
        p.set(at::OSC1, 0.0);
        p.set(at::OSC2, 0.0);
        p.set(at::OSC2_CENTS, 0.0);
        p.set(at::CUTOFF, 18_000.0);
        p.set(at::FILTER_ENV, 0.0);
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        s.process(&[Event::note_on(0, 0, 69, 127, 0)], &mut l, &mut r);
        let c = l[24_000..]
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((215..=225).contains(&(c / 2)), "{c}");
    }

    #[test]
    fn unison_spreads_the_voice_across_the_sides() {
        let mut s = synth();
        s.params.set(at::UNISON, 4.0);
        s.params.set(at::SPREAD, 1.0);
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        s.process(&[Event::note_on(0, 0, 57, 100, 0)], &mut l, &mut r);
        assert!(l.iter().zip(&r).any(|(a, b)| (a - b).abs() > 0.01));
    }

    #[test]
    fn one_note_mode_goes_back_to_the_note_still_held() {
        let mut s = synth();
        s.params.set(at::PLAY, 1.0);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        s.process(
            &[
                Event::note_on(0, 0, 60, 100, 0),
                Event::note_on(10, 0, 64, 100, 1),
                Event::note_off(20, 0, 64, 1),
            ],
            &mut l,
            &mut r,
        );
        assert_eq!(s.voices.iter().filter(|v| v.on).count(), 1);
        assert_eq!(s.voices[0].note, 60);
        assert!(s.voices[0].held);
    }
}
