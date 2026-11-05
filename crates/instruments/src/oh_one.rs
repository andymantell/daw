// SPDX-License-Identifier: GPL-3.0-or-later
//! Oh-One: the one-oscillator bass and lead monosynth of the early
//! 1980s (plan `docs/plans/classic-synths.md`, S3). One oscillator
//! giving a saw and a pulse at once, the pulse's width moved by hand,
//! by the LFO or by the envelope; a sub-oscillator divided down from it
//! (an octave down, two down, or two down as a narrow pulse); noise; a
//! source mixer that overdrives the filter when pushed; the 24 dB
//! transconductance cascade filter, singing on its own at the top of
//! its resonance, with the envelope, the LFO, the keyboard and the
//! bender on its cutoff; one ADSR envelope with analogue curves; an
//! amplifier that follows the envelope or just the gate; an LFO with
//! triangle, square, random and noise shapes; and portamento that can
//! be on, off, or only between overlapping notes.
//!
//! Everything nonlinear is where the circuit has it: the mixer and the
//! filter's input and feedback saturate (the toolkit's [`Cascade`], run
//! at twice the sample rate), and the amplifier soft-limits. The
//! oscillator is free-running, with a slow drift set by **Vintage
//! drift**.

use daw_engine::midi::{Event, Instrument, Kind};

use crate::dsp::{Noise, db, hz};
use crate::params::{Live, Param, Unit};
use crate::vintage::{
    Action, Allocator, Analog, Cascade, Drift, Glide, GlideMode, Osc, Poles, Priority, Shape,
    Steps, Sub, SubShape, VoiceSettings, Wave, soft,
};

const RANGES: &[&str] = &[
    "16' (an octave down)",
    "8'",
    "4' (an octave up)",
    "2' (two up)",
];
const PWM_SOURCES: &[&str] = &["By hand", "LFO", "Envelope"];
const AMP_MODES: &[&str] = &["Envelope", "Gate"];
const LFO_WAVES: &[&str] = &["Triangle", "Square", "Random", "Noise"];
const PORTAMENTO: &[&str] = &["Off", "On", "Overlapping notes only"];

/// Settings by index; the order of [`params`].
pub mod at {
    pub const RANGE: usize = 0;
    pub const LFO_PITCH: usize = 1;
    pub const WIDTH: usize = 2;
    pub const PWM_SOURCE: usize = 3;
    pub const PWM: usize = 4;
    pub const SAW: usize = 5;
    pub const PULSE: usize = 6;
    pub const SUB: usize = 7;
    pub const SUB_SHAPE: usize = 8;
    pub const NOISE: usize = 9;
    pub const CUTOFF: usize = 10;
    pub const RESONANCE: usize = 11;
    pub const FILTER_ENV: usize = 12;
    pub const FILTER_LFO: usize = 13;
    pub const KEY_TRACK: usize = 14;
    pub const AMP: usize = 15;
    pub const ATTACK: usize = 16;
    pub const DECAY: usize = 17;
    pub const SUSTAIN: usize = 18;
    pub const RELEASE: usize = 19;
    pub const LFO_WAVE: usize = 20;
    pub const LFO_RATE: usize = 21;
    pub const PORTAMENTO: usize = 22;
    pub const GLIDE: usize = 23;
    pub const PRIORITY: usize = 24;
    pub const LEGATO: usize = 25;
    pub const BEND: usize = 26;
    pub const BEND_FILTER: usize = 27;
    pub const VELOCITY: usize = 28;
    pub const LEVEL: usize = 29;
    pub const DRIVE: usize = 30;
    pub const VINTAGE: usize = 31;
}

/// The synth's settings, in the order of [`at`]. Grouped for showing by
/// [`GROUPS`].
pub fn params() -> Vec<Param> {
    use Unit::*;
    vec![
        Param::new("range", "Range", 0.0, 3.0, 1.0, Choice).choices(RANGES),
        Param::new("lfo_pitch", "LFO to pitch", 0.0, 12.0, 0.0, Semitones),
        Param::new("width", "Pulse width", 0.05, 0.95, 0.5, Amount),
        Param::new("pwm_source", "Width moved by", 0.0, 2.0, 0.0, Choice).choices(PWM_SOURCES),
        Param::new("pwm", "Width movement", 0.0, 1.0, 0.5, Amount),
        Param::new("saw", "Saw", 0.0, 1.0, 1.0, Amount),
        Param::new("pulse", "Pulse", 0.0, 1.0, 0.0, Amount),
        Param::new("sub", "Sub oscillator", 0.0, 1.0, 0.5, Amount),
        Param::new("sub_shape", "Sub shape", 0.0, 2.0, 0.0, Choice).choices(SubShape::NAMES),
        Param::new("noise", "Noise", 0.0, 1.0, 0.0, Amount),
        Param::new("cutoff", "Cutoff", 10.0, 20_000.0, 1_000.0, Hz).log(),
        Param::new("resonance", "Resonance", 0.0, 1.0, 0.2, Amount),
        Param::new("filter_env", "Envelope to filter", 0.0, 6.0, 2.5, Octaves),
        Param::new("filter_lfo", "LFO to filter", 0.0, 4.0, 0.0, Octaves),
        Param::new("key_track", "Keyboard to filter", 0.0, 1.0, 0.5, Amount),
        Param::new("amp", "Amplifier follows", 0.0, 1.0, 0.0, Choice).choices(AMP_MODES),
        Param::new("attack", "Attack", 0.0015, 3.0, 0.002, Seconds).log(),
        Param::new("decay", "Decay", 0.002, 12.0, 0.3, Seconds).log(),
        Param::new("sustain", "Sustain", 0.0, 1.0, 0.0, Amount),
        Param::new("release", "Release", 0.002, 12.0, 0.2, Seconds).log(),
        Param::new("lfo_wave", "LFO shape", 0.0, 3.0, 0.0, Choice).choices(LFO_WAVES),
        Param::new("lfo_rate", "LFO rate", 0.1, 30.0, 5.0, Hz).log(),
        Param::new("portamento", "Portamento", 0.0, 2.0, 0.0, Choice).choices(PORTAMENTO),
        Param::new("glide", "Portamento time", 0.005, 3.0, 0.1, Seconds).log(),
        Param::new("priority", "Note priority", 0.0, 2.0, 2.0, Choice).choices(Priority::NAMES),
        Param::new("legato", "Legato", 0.0, 1.0, 0.0, Switch),
        Param::new("bend", "Bend range", 1.0, 24.0, 2.0, Semitones),
        Param::new("bend_filter", "Bend to filter", 0.0, 4.0, 0.0, Octaves),
        Param::new("velocity", "Velocity", 0.0, 1.0, 0.5, Amount),
        Param::new("level", "Level", -36.0, 6.0, -6.0, Db),
        Param::new("drive", "Drive", 0.0, 1.0, 0.0, Amount),
        Param::new("vintage", "Vintage drift", 0.0, 1.0, 0.3, Amount),
    ]
}

/// The settings in groups, for the controls: (heading, first, last).
pub const GROUPS: &[(&str, usize, usize)] = &[
    ("Oscillator and mixer", at::RANGE, at::NOISE),
    ("Filter", at::CUTOFF, at::KEY_TRACK),
    ("Envelope and amplifier", at::AMP, at::RELEASE),
    ("LFO", at::LFO_WAVE, at::LFO_RATE),
    ("Playing", at::PORTAMENTO, at::LEVEL),
    ("Character", at::DRIVE, at::VINTAGE),
];

/// Settings are read, and the filter set, this often: a third of a
/// millisecond, so a 2 ms attack still sweeps the filter in steps too
/// small to hear.
/// Sounds made here, as settings over the defaults, so the panel's
/// header and the assistant can step through them.
pub fn sounds() -> Vec<crate::Sound> {
    use crate::Sound;
    vec![
        Sound {
            name: "Sub bass",
            settings: &[
                ("range", 0.0),
                ("saw", 0.6),
                ("sub", 0.9),
                ("cutoff", 400.0),
                ("resonance", 0.1),
                ("filter_env", 1.5),
                ("decay", 0.5),
                ("sustain", 0.6),
                ("release", 0.1),
            ],
        },
        Sound {
            name: "Plucked bass",
            settings: &[
                ("range", 0.0),
                ("saw", 1.0),
                ("sub", 0.5),
                ("cutoff", 250.0),
                ("resonance", 0.35),
                ("filter_env", 3.5),
                ("decay", 0.25),
                ("sustain", 0.0),
                ("release", 0.15),
            ],
        },
        Sound {
            name: "Wobbly pulse lead",
            settings: &[
                ("saw", 0.0),
                ("pulse", 1.0),
                ("pwm_source", 1.0),
                ("pwm", 0.6),
                ("sub", 0.0),
                ("cutoff", 2_500.0),
                ("resonance", 0.3),
                ("filter_env", 1.0),
                ("sustain", 0.7),
                ("lfo_rate", 3.0),
                ("portamento", 2.0),
                ("glide", 0.08),
            ],
        },
        Sound {
            name: "Resonant sweep",
            settings: &[
                ("saw", 1.0),
                ("sub", 0.3),
                ("cutoff", 300.0),
                ("resonance", 0.7),
                ("filter_env", 4.0),
                ("attack", 0.3),
                ("decay", 1.5),
                ("sustain", 0.3),
                ("release", 0.6),
            ],
        },
    ]
}

const CHUNK: usize = 8;

/// The gate's amplifier shape: on and off in a few milliseconds, so it
/// doesn't click.
fn gate_shape() -> Shape {
    Shape::adsr(0.002, 0.001, 1.0, 0.004)
}

/// The synth on the audio thread.
pub struct OhOne {
    params: Live,
    rate: f32,
    voices: Allocator,
    saw: Osc,
    pulse: Osc,
    sub: Sub,
    noise: Noise,
    filter: Cascade,
    env: Analog,
    gate: Analog,
    drift: Drift,
    /// The note the voice is on, if it is sounding.
    on: bool,
    /// The pitch now, gliding towards `target`.
    pitch: f32,
    target: f32,
    velocity: f32,
    bend: f32,
    /// Keys down, to tell an overlapping note from a new one.
    keys_down: u32,
    lfo_phase: f32,
    /// The random shape's held value.
    lfo_held: f32,
    lfo_noise: Noise,
    actions: Vec<Action>,
}

impl OhOne {
    pub fn new(rate: u32, params: impl Into<Live>) -> OhOne {
        let mut sub = Sub::new(SubShape::OctaveDown);
        sub.reset();
        OhOne {
            params: params.into(),
            rate: rate.max(1) as f32,
            voices: Allocator::new(1),
            saw: Osc::new(Wave::Saw),
            pulse: Osc::new(Wave::Pulse),
            sub,
            noise: Noise::new(0x0101_0101),
            filter: Cascade::new(),
            env: Analog::IDLE,
            gate: Analog::IDLE,
            drift: Drift::new(101),
            on: false,
            pitch: 60.0,
            target: 60.0,
            velocity: 1.0,
            bend: 0.0,
            keys_down: 0,
            lfo_phase: 0.0,
            lfo_held: 0.0,
            lfo_noise: Noise::new(0x5151_5151),
            actions: Vec::with_capacity(8),
        }
    }

    fn settings(&self) -> VoiceSettings {
        VoiceSettings {
            notes: 1,
            unison: 1,
            priority: Priority::from_index(self.params.read(at::PRIORITY)),
            legato: self.params.read(at::LEGATO) >= 0.5,
        }
    }

    /// Whether a move to a new note glides, given whether another key
    /// was held as it came.
    fn glides(&self, overlapping: bool) -> bool {
        match self.params.read(at::PORTAMENTO) as u32 {
            1 => true,
            2 => overlapping,
            _ => false,
        }
    }

    fn apply(&mut self, overlapping: bool) {
        let sens = self.params.read(at::VELOCITY);
        for i in 0..self.actions.len() {
            match self.actions[i] {
                Action::Start {
                    note,
                    velocity,
                    fresh,
                    ..
                } => {
                    self.target = note as f32;
                    if fresh || !self.glides(overlapping) {
                        self.pitch = self.target;
                    }
                    if fresh {
                        // The filter starts clean; the oscillator runs
                        // free, as the circuit's does.
                        self.filter.reset();
                        self.sub.reset();
                    }
                    self.velocity = 1.0 - sens + sens * velocity.clamp(1, 127) as f32 / 127.0;
                    self.env.trigger();
                    self.gate.trigger();
                    self.on = true;
                }
                Action::Retune { note, .. } => {
                    self.target = note as f32;
                    if !self.glides(true) {
                        self.pitch = self.target;
                    }
                }
                Action::Release { .. } => {
                    self.env.release();
                    self.gate.release();
                }
            }
        }
        self.actions.clear();
    }

    fn event(&mut self, e: &Event) {
        let s = self.settings();
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => {
                let overlapping = self.keys_down > 0;
                self.keys_down += 1;
                self.voices.note_on(pitch, velocity, &s, &mut self.actions);
                self.apply(overlapping);
            }
            Kind::NoteOff { pitch, .. } => {
                self.keys_down = self.keys_down.saturating_sub(1);
                self.voices.note_off(pitch, &s, &mut self.actions);
                // Going back to a note still held is an overlap.
                self.apply(true);
            }
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number: 64, value } => {
                self.voices.pedal(value >= 64, &mut self.actions);
                self.apply(false);
            }
            Kind::Control { number: 120, .. } | Kind::AllNotesOff => {
                self.keys_down = 0;
                self.voices.all_off(&mut self.actions);
                self.apply(false);
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

    /// The LFO's value now, −1 to 1, by its shape.
    #[inline]
    fn lfo(&mut self, shape: u32) -> f32 {
        match shape {
            1 => {
                if self.lfo_phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            2 => self.lfo_held,
            3 => self.lfo_noise.tick(),
            _ => 1.0 - 4.0 * (self.lfo_phase - 0.5).abs(),
        }
    }

    #[inline]
    fn lfo_advance(&mut self, dt: f32) {
        self.lfo_phase += dt;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
            self.lfo_held = self.lfo_noise.tick();
        }
    }

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let p = &self.params;
        let r = self.rate;
        let n = (to - from) as f32;
        let lfo_shape = p.read(at::LFO_WAVE) as u32;
        let lfo_dt = p.read(at::LFO_RATE) / r;
        if !self.on {
            // The LFO runs free while nothing plays, as the circuit's does.
            self.lfo_advance(lfo_dt * n);
            return;
        }
        let range = match p.read(at::RANGE) as u32 {
            0 => -12.0,
            2 => 12.0,
            3 => 24.0,
            _ => 0.0,
        };
        let width = p.read(at::WIDTH);
        let pwm_source = p.read(at::PWM_SOURCE) as u32;
        let pwm = p.read(at::PWM);
        let saw = p.read(at::SAW);
        let pulse = p.read(at::PULSE);
        let sub = p.read(at::SUB);
        self.sub.shape = SubShape::from_index(p.read(at::SUB_SHAPE));
        let noise = p.read(at::NOISE);
        let cutoff = p.read(at::CUTOFF);
        let res = p.read(at::RESONANCE);
        let fenv = p.read(at::FILTER_ENV);
        let flfo = p.read(at::FILTER_LFO);
        let track = p.read(at::KEY_TRACK);
        let gate_mode = p.read(at::AMP) >= 0.5;
        let steps = Steps::new(
            Shape::adsr(
                p.read(at::ATTACK),
                p.read(at::DECAY),
                p.read(at::SUSTAIN),
                p.read(at::RELEASE),
            ),
            r,
        );
        let gate_steps = Steps::new(gate_shape(), r);
        let glide = Glide {
            mode: GlideMode::Time,
            seconds: p.read(at::GLIDE),
        };
        let bend = self.bend * p.read(at::BEND);
        let bend_filter = self.bend * p.read(at::BEND_FILTER);
        let lfo_pitch = p.read(at::LFO_PITCH);
        let gain = db(p.read(at::LEVEL));
        let drive = 1.0 + 3.0 * p.read(at::DRIVE);
        let vintage = p.read(at::VINTAGE);

        // Once a chunk: the pitch, with its glide and drift, and the
        // filter's cutoff from the key, the envelope, the LFO and the
        // bender.
        let lfo_now = self.lfo(lfo_shape);
        self.pitch = glide.step(self.pitch, self.target, n, r);
        let cents = self.drift.next(vintage, n, r);
        let pitch = self.pitch + range + bend + lfo_pitch * lfo_now + cents / 100.0;
        let f = hz(pitch);
        self.saw.set_hz(f, r);
        self.pulse.set_hz(f, r);
        let octaves =
            track * (pitch - 60.0) / 12.0 + fenv * self.env.level + flfo * lfo_now + bend_filter;
        self.filter
            .set(Poles::Four, cutoff * octaves.exp2(), res, drive, 0.3, r);
        // The sources sum into the filter at a level that leaves room:
        // a saw and a sub together just start to warm its input.
        let mix_gain = 0.5;
        let vel_gain = self.velocity * gain;

        for i in from..to {
            let e = self.env.next(&steps);
            let g = self.gate.next(&gate_steps);
            let lfo = if pwm_source == 1 {
                self.lfo(lfo_shape)
            } else {
                0.0
            };
            self.lfo_advance(lfo_dt);
            self.pulse.width = match pwm_source {
                1 => (0.5 + 0.45 * pwm * lfo).clamp(0.05, 0.95),
                2 => (0.5 + 0.45 * pwm * e).clamp(0.05, 0.95),
                _ => width,
            };
            let s = self.saw.tick(None);
            let q = self.pulse.tick(None);
            let b = self.sub.tick(self.saw.wrapped);
            let mut x = saw * s + pulse * q + sub * b;
            if noise > 0.0 {
                x += noise * self.noise.tick();
            }
            let y = self.filter.run(x * mix_gain);
            let amp = if gate_mode { g } else { e };
            // The amplifier limits softly when pushed.
            let out = soft(y * amp * 2.0) * 0.5 * vel_gain;
            left[i] += out;
            right[i] += out;
            let done = if gate_mode {
                !self.gate.active()
            } else {
                !self.env.active()
            };
            if done {
                self.on = false;
                self.voices.free(0);
                // Carry the LFO over the rest of the chunk.
                self.lfo_advance(lfo_dt * (to - i - 1) as f32);
                break;
            }
        }
    }
}

impl Instrument for OhOne {
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
    use crate::params::Params;
    use std::sync::Arc;

    const RATE: f32 = 48_000.0;

    fn synth() -> OhOne {
        OhOne::new(RATE as u32, Arc::new(Params::new(params())))
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// The level at `hz` in `x`, in dB.
    fn db_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let p = std::f64::consts::TAU * hz as f64 * i as f64 / RATE as f64;
            re += v as f64 * p.cos();
            im += v as f64 * p.sin();
        }
        let a = (re * re + im * im).sqrt() * 2.0 / x.len() as f64;
        20.0 * a.max(1e-9).log10() as f32
    }

    fn render(s: &mut OhOne, events: &[Event], n: usize) -> Vec<f32> {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.process(events, &mut l, &mut r);
        l
    }

    #[test]
    fn a_note_sounds_then_dies_away_and_the_voice_is_freed() {
        let mut s = synth();
        let x = render(&mut s, &[Event::note_on(100, 0, 45, 100, 0)], 4800);
        assert_eq!(peak(&x[..100]), 0.0);
        assert!(peak(&x[100..]) > 0.05, "{}", peak(&x[100..]));
        let x = render(&mut s, &[Event::note_off(0, 0, 45, 0)], 96_000);
        assert_eq!(peak(&x[90_000..]), 0.0);
        assert!(!s.on);
        assert_eq!(s.voices.note_of(0), None);
    }

    #[test]
    fn the_pitch_follows_the_note_and_the_range_switch() {
        let crossings = |range: f64| {
            let mut s = synth();
            s.params.set(at::RANGE, range);
            s.params.set(at::SUB, 0.0);
            s.params.set(at::CUTOFF, 20_000.0);
            s.params.set(at::FILTER_ENV, 0.0);
            s.params.set(at::VINTAGE, 0.0);
            s.params.set(at::SUSTAIN, 1.0);
            let x = render(&mut s, &[Event::note_on(0, 0, 69, 127, 0)], 48_000);
            x[24_000..]
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
                / 2
        };
        // Half a second of a saw: one crossing a cycle after halving.
        assert!((215..=225).contains(&crossings(1.0)), "{}", crossings(1.0));
        assert!((105..=115).contains(&crossings(0.0)), "{}", crossings(0.0));
        assert!((435..=445).contains(&crossings(2.0)), "{}", crossings(2.0));
    }

    #[test]
    fn the_sub_adds_the_octave_below_and_noise_fills_between_harmonics() {
        let spectrum = |sub: f64, noise: f64| {
            let mut s = synth();
            s.params.set(at::SUB, sub);
            s.params.set(at::NOISE, noise);
            s.params.set(at::CUTOFF, 20_000.0);
            s.params.set(at::RESONANCE, 0.0);
            s.params.set(at::FILTER_ENV, 0.0);
            s.params.set(at::VINTAGE, 0.0);
            s.params.set(at::SUSTAIN, 1.0);
            // 100 Hz divides the sample rate, so harmonics land on bins.
            s.params.set(at::RANGE, 1.0);
            let x = render(&mut s, &[Event::note_on(0, 0, 43, 127, 0)], 96_000);
            x[48_000..].to_vec()
        };
        let f0 = hz(43.0); // 97.999 Hz
        let plain = spectrum(0.0, 0.0);
        let with_sub = spectrum(1.0, 0.0);
        assert!(db_at(&with_sub, f0 / 2.0) > db_at(&plain, f0 / 2.0) + 20.0);
        let between = f0 * 1.5;
        let noisy = spectrum(0.0, 1.0);
        assert!(db_at(&noisy, between) > db_at(&plain, between) + 10.0);
    }

    #[test]
    fn the_filter_closes_and_sings_on_its_own() {
        let bright = |cutoff: f64| {
            let mut s = synth();
            s.params.set(at::CUTOFF, cutoff);
            s.params.set(at::FILTER_ENV, 0.0);
            s.params.set(at::KEY_TRACK, 0.0);
            s.params.set(at::SUSTAIN, 1.0);
            s.params.set(at::VINTAGE, 0.0);
            let x = render(&mut s, &[Event::note_on(0, 0, 45, 100, 0)], 48_000);
            db_at(&x[24_000..], hz(45.0) * 8.0)
        };
        assert!(bright(10_000.0) > bright(300.0) + 30.0);
        // No source, resonance full: the filter whistles at its cutoff.
        let mut s = synth();
        s.params.set(at::SAW, 0.0);
        s.params.set(at::SUB, 0.0);
        s.params.set(at::NOISE, 0.02);
        s.params.set(at::CUTOFF, 1000.0);
        s.params.set(at::RESONANCE, 1.0);
        s.params.set(at::FILTER_ENV, 0.0);
        s.params.set(at::KEY_TRACK, 0.0);
        s.params.set(at::SUSTAIN, 1.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 60, 100, 0)], 96_000);
        let hz = crate::vintage::measure::ring_hz(&x, RATE);
        assert!((hz / 1000.0 - 1.0).abs() < 0.05, "{hz}");
        assert!(peak(&x[48_000..]) > 0.02);
    }

    #[test]
    fn pulse_width_moves_with_the_lfo_and_the_envelope() {
        let mean_width = |source: f64| {
            let mut s = synth();
            s.params.set(at::SAW, 0.0);
            s.params.set(at::SUB, 0.0);
            s.params.set(at::PULSE, 1.0);
            s.params.set(at::PWM_SOURCE, source);
            s.params.set(at::PWM, 1.0);
            s.params.set(at::LFO_RATE, 0.5);
            s.params.set(at::SUSTAIN, 1.0);
            s.params.set(at::ATTACK, 1.0);
            s.params.set(at::CUTOFF, 20_000.0);
            s.params.set(at::FILTER_ENV, 0.0);
            render(&mut s, &[Event::note_on(0, 0, 57, 100, 0)], 48_000);
            s.pulse.width
        };
        assert!((mean_width(0.0) - 0.5).abs() < 1e-6);
        // A second into a 0.5 Hz triangle LFO, the width is at its widest.
        let mut s = synth();
        s.params.set(at::PWM_SOURCE, 1.0);
        s.params.set(at::PWM, 1.0);
        s.params.set(at::LFO_RATE, 0.5);
        s.params.set(at::SUSTAIN, 1.0);
        render(&mut s, &[Event::note_on(0, 0, 57, 100, 0)], 48_000);
        assert!((s.pulse.width - 0.95).abs() < 0.02, "{}", s.pulse.width);
        // The envelope, a second into its attack, has it most of the way.
        assert!(mean_width(2.0) > 0.85, "{}", mean_width(2.0));
    }

    #[test]
    fn gate_mode_holds_full_level_until_the_key_lifts() {
        let mut s = synth();
        s.params.set(at::AMP, 1.0);
        s.params.set(at::SUSTAIN, 0.0);
        s.params.set(at::DECAY, 0.01);
        s.params.set(at::FILTER_ENV, 0.0);
        s.params.set(at::CUTOFF, 5000.0);
        let x = render(&mut s, &[Event::note_on(0, 0, 45, 100, 0)], 48_000);
        // With the envelope the sound would be gone; the gate holds it.
        assert!(peak(&x[40_000..]) > 0.1, "{}", peak(&x[40_000..]));
        let x = render(&mut s, &[Event::note_off(0, 0, 45, 0)], 4800);
        assert_eq!(peak(&x[2400..]), 0.0);
        assert!(!s.on);
    }

    #[test]
    fn portamento_glides_only_when_asked() {
        let pitch_after = |mode: f64, overlap: bool| {
            let mut s = synth();
            s.params.set(at::PORTAMENTO, mode);
            s.params.set(at::GLIDE, 1.0);
            s.params.set(at::SUSTAIN, 1.0);
            let mut events = vec![Event::note_on(0, 0, 48, 100, 0)];
            if !overlap {
                events.push(Event::note_off(500, 0, 48, 0));
            }
            events.push(Event::note_on(1000, 0, 60, 100, 1));
            render(&mut s, &events, 1100);
            s.pitch
        };
        assert_eq!(pitch_after(0.0, true), 60.0);
        assert!(pitch_after(1.0, true) < 50.0);
        assert!(pitch_after(1.0, false) < 50.0);
        assert!(pitch_after(2.0, true) < 50.0);
        assert_eq!(pitch_after(2.0, false), 60.0);
    }

    #[test]
    fn priority_and_legato_follow_the_settings() {
        let mut s = synth();
        s.params.set(at::PRIORITY, 0.0);
        s.params.set(at::SUSTAIN, 1.0);
        render(
            &mut s,
            &[
                Event::note_on(0, 0, 60, 100, 0),
                Event::note_on(10, 0, 55, 100, 1),
                Event::note_on(20, 0, 64, 100, 2),
            ],
            64,
        );
        assert_eq!(s.target, 55.0, "the lowest held note sounds");
        render(&mut s, &[Event::note_off(0, 0, 55, 1)], 64);
        assert_eq!(s.target, 60.0, "back to the lowest still held");
        // Legato: a second note while one is held leaves the envelope
        // where it is; without it the attack starts again.
        let level_after = |legato: f64| {
            let mut s = synth();
            s.params.set(at::LEGATO, legato);
            s.params.set(at::ATTACK, 0.5);
            s.params.set(at::SUSTAIN, 1.0);
            render(
                &mut s,
                &[
                    Event::note_on(0, 0, 48, 100, 0),
                    Event::note_on(24_000, 0, 55, 100, 1),
                ],
                24_016,
            );
            s.env.level
        };
        assert!(level_after(1.0) > 0.9, "{}", level_after(1.0));
        // Retriggering starts the attack from where it is, so the level
        // keeps climbing rather than dropping: the same here.
        assert!(level_after(0.0) > 0.9);
    }

    /// The golden render: a fixed bass patch has a known spectrum, so a
    /// later change to the toolkit can't silently change the sound.
    #[test]
    fn the_golden_bass_renders_the_same_spectrum() {
        let mut s = synth();
        s.params.set(at::SUB, 0.8);
        s.params.set(at::PULSE, 0.3);
        s.params.set(at::CUTOFF, 400.0);
        s.params.set(at::RESONANCE, 0.5);
        s.params.set(at::FILTER_ENV, 2.0);
        s.params.set(at::DECAY, 0.4);
        s.params.set(at::VINTAGE, 0.0);
        s.params.set(at::DRIVE, 0.3);
        let x = render(&mut s, &[Event::note_on(0, 0, 41, 110, 0)], 48_000);
        let f0 = hz(41.0);
        let got: Vec<f32> = [0.5, 1.0, 2.0, 3.0, 4.0]
            .iter()
            .map(|h| db_at(&x[..24_000], f0 * h))
            .collect();
        // Measured when the synth was built; see the review notes.
        let want = GOLDEN;
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 0.5, "got {got:.1?}, want {want:?}");
        }
    }

    /// Levels in dB at half, one, two, three and four times the
    /// fundamental of the golden bass.
    const GOLDEN: [f32; 5] = [-34.4, -51.1, -48.9, -60.4, -58.8];

    /// The plan's CPU budget, for one voice at 2× oversampling: well
    /// under a percent of one core at 48 kHz; the bound is loose because
    /// test machines differ.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timed in release builds only")]
    fn one_voice_fits_the_budget() {
        let mut s = synth();
        s.params.set(at::SUSTAIN, 1.0);
        s.params.set(at::NOISE, 0.2);
        s.params.set(at::PWM_SOURCE, 1.0);
        let seconds = 2.0;
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let start = std::time::Instant::now();
        s.process(&[Event::note_on(0, 0, 40, 100, 0)], &mut l, &mut r);
        for _ in 0..(48_000.0 * seconds / 512.0) as usize {
            s.process(&[], &mut l, &mut r);
        }
        let used = start.elapsed().as_secs_f64() / seconds;
        assert!(used < 0.05, "{:.2}% of one core", used * 100.0);
    }

    #[test]
    fn every_sound_made_in_code_names_real_settings_and_plays() {
        let keys: Vec<String> = params().into_iter().map(|p| p.key).collect();
        assert!(sounds().len() >= 4);
        for sound in sounds() {
            let p = Arc::new(Params::new(params()));
            for (k, v) in sound.settings {
                let i = keys.iter().position(|key| key == k);
                assert!(i.is_some(), "{}: no setting {k}", sound.name);
                let param = &params()[i.unwrap()];
                assert!(
                    (param.min..=param.max).contains(v),
                    "{}: {k} = {v} is out of range",
                    sound.name
                );
                p.set(i.unwrap(), *v);
            }
            let mut s = OhOne::new(RATE as u32, p);
            let x = render(&mut s, &[Event::note_on(0, 0, 45, 100, 0)], 48_000);
            assert!(peak(&x) > 0.02, "{} is silent", sound.name);
            assert!(peak(&x) <= 1.0, "{} clips", sound.name);
        }
    }
}
